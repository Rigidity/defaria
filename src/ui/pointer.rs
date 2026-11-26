//! The mouse pointer over the UI, through Bevy's picking.
//!
//! `bevy_picking`'s UI backend finds the nodes under the pointer and sends
//! them its pointer events ([`PointerPress`](bevy::picking::events::PointerPress),
//! `PointerClick`, `PointerDrag`, `PointerDragDrop`, `PointerScroll`, …),
//! which bubble up through their ancestors. Widgets and features observe the
//! ones they care about; a node that wants to know whether it's hovered
//! carries picking's [`Hovered`](bevy::picking::hover::Hovered).
//!
//! - The UI only takes the pointer while a screen is open. While playing
//!   the cursor is locked and the mouse belongs to the game, so nothing is
//!   picked: [`UiPickingSettings::require_markers`] is turned on, and no
//!   camera has the marker it then asks for. The HUD isn't hovered or
//!   clicked by a cursor that's been locked over it.
//! - Headless runs have no OS window for Bevy's mouse input to read, so the
//!   runner writes [`PointerInput`](bevy::picking::pointer::PointerInput)
//!   itself, aimed at the image it renders into (see `crate::headless`).
//! - Nodes that only show something over others, such as the tooltip, carry
//!   [`Pickable::IGNORE`] so they don't hide what's under them.

use bevy::{
    ecs::system::SystemParam,
    picking::pointer::{PointerId, PointerLocation},
    prelude::*,
    ui::picking_backend::UiPickingSettings,
};

use super::screen::{UiScreen, playing};

pub struct PointerPlugin;

impl Plugin for PointerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            pick_only_in_screens.before(bevy::picking::PickingSystems::Backend),
        );
    }
}

/// While playing, UI picking looks only at cameras marked
/// [`UiPickingCamera`](bevy::ui::picking_backend::UiPickingCamera), and none are, so nothing
/// is under the pointer; with a screen open it looks at every camera.
fn pick_only_in_screens(
    screens: Query<(), With<UiScreen>>,
    settings: Option<ResMut<UiPickingSettings>>,
) {
    let Some(mut settings) = settings else {
        return;
    };
    let require_markers = playing(screens);
    if settings.require_markers != require_markers {
        settings.require_markers = require_markers;
    }
}

/// The mouse pointer's position. What it's over is picking's: observe its
/// events, or give a node [`Hovered`](bevy::picking::hover::Hovered).
#[derive(SystemParam)]
pub struct UiPointer<'w, 's> {
    pointers: Query<'w, 's, (&'static PointerId, &'static PointerLocation)>,
}

impl UiPointer<'_, '_> {
    /// Where the mouse is, in logical pixels of what it points at (the
    /// window, or the headless runner's image). `None` before it first
    /// moves.
    pub fn position(&self) -> Option<Vec2> {
        self.pointers
            .iter()
            .find(|(id, _)| id.is_mouse())
            .and_then(|(_, location)| location.location())
            .map(|location| location.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_picked_while_playing() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PointerPlugin))
            .init_resource::<UiPickingSettings>();
        let require = |app: &App| app.world().resource::<UiPickingSettings>().require_markers;
        app.update();
        assert!(require(&app));
        let screen = app.world_mut().spawn(UiScreen).id();
        app.update();
        assert!(!require(&app));
        app.world_mut().despawn(screen);
        app.update();
        assert!(require(&app));
    }
}
