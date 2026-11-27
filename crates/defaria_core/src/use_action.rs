//! Using things: right click, as a [`UseAction`] the player takes.
//!
//! One system reads right click (`input::using`): while playing, with
//! the cursor locked (the click that locks it uses nothing), it gives the
//! local player a [`Use`] for the frame, with whether Shift is held (it
//! sneaks past what a block or item does when used, to place) and the
//! block under the crosshair. Gameplay acts on the use, never on the
//! mouse, so a player that isn't this game's (a server's) uses things
//! the same way.
//!
//! What a use does is decided in one place, the order of [`UseStep`]:
//! each step runs only while the use is unclaimed, and claims it
//! ([`UseAction::claim`]) if it's the step's, so nothing after it acts on
//! it too. In order, the wrench (an item's action, in
//! hand), a fluid container in hand, a block's script's `on_used`, a
//! machine's screen, an item's script's `on_use`, and last placing the
//! block in hand.

use bevy::prelude::*;

use crate::block_edit::Hit;

pub struct UseActionPlugin;

impl Plugin for UseActionPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<UseAction>().configure_sets(
            Update,
            // Each step after the input only while a use is unclaimed,
            // checked as the step comes up, so none runs after a claim.
            (
                UseStep::Input,
                UseStep::Wrench.run_if(pending),
                UseStep::Container.run_if(pending),
                UseStep::Block.run_if(pending),
                UseStep::Screen.run_if(pending),
                UseStep::Item.run_if(pending),
                UseStep::Place.run_if(pending),
            )
                .chain_ignore_deferred()
                .in_set(UseSystems),
        );
    }
}

/// Everything that reads or acts on [`UseAction`]s.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct UseSystems;

/// The steps of [`UseSystems`], in order of precedence: each acts on a
/// use only if no step before it claimed it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UseStep {
    /// Right click becomes a [`Use`] (`input::using`).
    Input,
    /// The wrench in hand (`wrench`): every use with it in hand is its.
    Wrench,
    /// A fluid container in hand (`fluid_container`).
    Container,
    /// A block whose script has `on_used` (`script::using`).
    Block,
    /// A machine's screen opens (`machine::screen`).
    Screen,
    /// An item whose script has `on_use` (`script::using`).
    Item,
    /// The block in hand is placed (`block_edit`). It claims every use,
    /// so none outlasts its frame.
    Place,
}

/// A player using what's under its crosshair or in its hand this frame,
/// until a [`UseStep`] claims it.
#[derive(Component, Reflect, Debug, Default, Clone, Copy, PartialEq)]
#[reflect(Component)]
pub struct UseAction(pub Option<Use>);

impl UseAction {
    /// A use this frame that no step has claimed yet.
    pub fn pending(&self) -> Option<Use> {
        self.0
    }

    /// Takes the use, so no later step acts on it.
    pub fn claim(&mut self) {
        self.0 = None;
    }
}

/// One use: a right click, as it was made.
#[derive(Reflect, Debug, Clone, Copy, PartialEq)]
pub struct Use {
    /// The player's eyes: where it aims from.
    pub eyes: Transform,
    /// The block under the crosshair (`block_edit::crosshair_target`), if
    /// any.
    pub target: Option<Hit>,
    /// Shift is held: the player sneaks past what a block does when used
    /// (a machine's screen, a script's `on_used`) or pours rather than
    /// takes with a fluid container.
    pub sneak: bool,
}

/// Whether some player has a use no step has claimed yet.
fn pending(actions: Query<&UseAction>) -> bool {
    actions.iter().any(|action| action.pending().is_some())
}
