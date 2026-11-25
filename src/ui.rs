//! The UI toolkit: the pieces screens and the HUD are built from.
//!
//! - [`theme`]: the shared sizes and colors, [`UiTheme`].
//! - [`screen`]: the [`UiScreen`] state, which screen (if any) has the mouse
//!   and keyboard, and the [`playing`] run condition for gameplay input.
//! - [`pointer`]: hovering and clicking nodes ([`Hoverable`], [`Clicked`]).
//! - [`scroll`]: wheel scrolling and scrollbars ([`scroll_panel`]).
//! - [`icon`]: item icons drawn from the block textures ([`ItemIcon`]).
//! - [`slot`]: the item slot widget ([`item_slot`]).
//! - [`tooltip`]: text beside the pointer ([`Tooltip`]).
//!
//! Widgets are bundle-returning functions plus the components and systems
//! that keep them up to date. They don't decide what a click does: the code
//! that spawns one observes its [`Clicked`]. Features build their own UI out
//! of these in their own modules (`hotbar`, `inventory`).
//!
//! The widgets are meant to stay small and data-driven, so that a later
//! layout format (UI described in mod files) can map onto them one to one.

pub mod icon;
pub mod pointer;
pub mod screen;
pub mod scroll;
pub mod slot;
pub mod theme;
pub mod tooltip;

use bevy::prelude::*;

pub use icon::ItemIcon;
pub use pointer::{Clicked, Hoverable, Hovered, UiPointer, UiPointerSystems};
pub use screen::{UiScreen, playing};
pub use scroll::{Scrollable, scroll_panel};
pub use slot::{ItemSlot, item_slot};
pub use theme::UiTheme;
pub use tooltip::Tooltip;

pub struct UiPlugin;

/// Systems that bring widgets in line with their data ([`ItemSlot`] to its
/// icon and frame, [`ItemIcon`] to its image), in `PostUpdate` before
/// layout, so whatever `Update` changed shows on the same frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct UiWidgetSystems;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiTheme>()
            .configure_sets(
                PostUpdate,
                UiWidgetSystems.before(bevy::ui::UiSystems::Prepare),
            )
            .add_plugins((
                screen::ScreenPlugin,
                pointer::PointerPlugin,
                scroll::ScrollPlugin,
                icon::IconPlugin,
                slot::SlotPlugin,
                tooltip::TooltipPlugin,
            ));
    }
}
