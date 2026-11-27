//! Right click on scripted blocks and with scripted items, as uses of them
//! (see `use_action`): a block whose script defines `on_used` is used
//! instead of placing against it (unless the player sneaks with something
//! in hand), and an item whose script defines `on_use` is used instead of
//! placed. Each claims the use, so nothing after it acts on it, and sends
//! a [`BlockUsed`] or [`ItemUsed`] for [`hooks`](super::hooks) to call the
//! hook on the next tick.
//!
//! Using a block is [`UseStep::Block`], after the wrench and before
//! machine screens (so a scripted machine's `on_used` replaces its
//! screen); using an item is [`UseStep::Item`], after every block use, so
//! a block that does something when used (a machine's screen) comes
//! before the item in hand.

use bevy::prelude::*;

use super::{BlockUsed, ItemUsed, ScriptIndex, api::PlayerView};
use crate::{
    content::{Content, Direction},
    game_mode::GameMode,
    hotbar::{ActiveSlot, Hotbar},
    player::Player,
    use_action::{Use, UseAction, UseStep},
    world::Voxels,
};

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            use_scripted_block.in_set(UseStep::Block),
            use_scripted_item.in_set(UseStep::Item),
        )
            // Left out in apps with no content, such as the scripts' tests.
            .run_if(resource_exists::<Content>),
    );
}

/// What a script sees of the player making `using`.
fn view(using: Use, mode: Option<&GameMode>, walker: Option<&Player>) -> PlayerView {
    PlayerView {
        pos: using.eyes.translation,
        look: *using.eyes.forward(),
        mode: mode.copied().unwrap_or_default(),
        sneaking: walker.is_some_and(|walker| walker.sneaking),
        target: using
            .target
            .map(|hit| (hit.block, Direction::from_normal(hit.normal))),
    }
}

/// Uses the block a player uses if its script defines `on_used`,
/// claiming the use; sneaking with something in hand passes it by, to
/// place against it.
pub fn use_scripted_block(
    mut players: Query<(
        &mut UseAction,
        &Hotbar,
        &ActiveSlot,
        Option<&GameMode>,
        Option<&Player>,
    )>,
    content: Res<Content>,
    scripts: Option<Res<ScriptIndex>>,
    voxels: Voxels,
    mut uses: MessageWriter<BlockUsed>,
) {
    let Some(scripts) = scripts else {
        return;
    };
    for (mut action, hotbar, active, mode, walker) in &mut players {
        let Some(using) = action.pending() else {
            continue;
        };
        let Some(hit) = using.target else {
            continue;
        };
        if using.sneak && hotbar.item(*active).is_some() {
            continue;
        }
        if !voxels
            .get(hit.block)
            .is_some_and(|state| scripts.is_usable(&content, state))
        {
            continue;
        }
        action.claim();
        uses.write(BlockUsed {
            pos: hit.block,
            player: Some(view(using, mode, walker)),
        });
    }
}

/// Uses the item in a player's hand if its script defines `on_use`,
/// claiming the use, when nothing used the block under the crosshair.
pub fn use_scripted_item(
    mut players: Query<(
        Entity,
        &mut UseAction,
        &Hotbar,
        &ActiveSlot,
        Option<&GameMode>,
        Option<&Player>,
    )>,
    content: Res<Content>,
    scripts: Option<Res<ScriptIndex>>,
    mut uses: MessageWriter<ItemUsed>,
) {
    let Some(scripts) = scripts else {
        return;
    };
    for (entity, mut action, hotbar, active, mode, walker) in &mut players {
        let Some(using) = action.pending() else {
            continue;
        };
        let Some(item) = hotbar.item(*active) else {
            continue;
        };
        if scripts.item_use(&content, item).is_none() {
            continue;
        }
        action.claim();
        uses.write(ItemUsed {
            player: entity,
            item,
            view: view(using, mode, walker),
        });
    }
}
