//! The UI toolkit: the pieces screens and the HUD are built from.
//!
//! - [`theme`]: the shared sizes and colors, [`UiTheme`].
//! - [`screen`]: the [`UiScreen`] marker on an open screen's root, which
//!   takes the mouse and keyboard from the game, and the [`playing`] run
//!   condition for gameplay input.
//! - [`pointer`]: the mouse over the UI, through Bevy's picking
//!   ([`UiPointer`]); widgets observe picking's pointer events.
//! - [`scroll`]: wheel scrolling and scrollbars ([`scroll_panel`]).
//! - [`icon`]: item icons drawn from the block textures ([`ItemIcon`]).
//! - [`slot`]: the item slot widget ([`item_slot`]).
//! - [`button`]: buttons ([`button`](button::button)), on Bevy's.
//! - [`text_field`]: single-line text fields ([`text_field`]), on Bevy's
//!   text input, and the [`typing`] run condition.
//! - [`font`]: the pixel font every text uses.
//! - [`tooltip`]: text beside the pointer ([`Tooltip`]).
//!
//! Widgets are bundle-returning functions (or, where one part needs
//! another's entity, functions that spawn and return it, like
//! [`scroll_panel`]) plus the components and systems that keep them up to
//! date. They don't decide what a click does: the code that spawns one
//! observes picking's events on it (`PointerPress`, `PointerClick`,
//! `PointerDragDrop`, …), which bubble up from the node under the pointer
//! to its ancestors. Features build their own UI out
//! of these in their own modules (`hotbar`, `inventory`).
//!
//! The widgets are meant to stay small and data-driven, so that a later
//! layout format (UI described in mod files) can map onto them one to one.

pub mod button;
pub mod font;
#[cfg(feature = "dev")]
pub mod gallery;
pub mod icon;
pub mod pointer;
pub mod screen;
pub mod scroll;
pub mod slot;
pub mod text_field;
pub mod theme;
pub mod tooltip;

use bevy::prelude::*;

pub use button::{UiButton, button};
pub use icon::ItemIcon;
pub use pointer::{DRAG_DISTANCE, PointerTravel, UiPointer};
pub use screen::{UiScreen, playing};
pub use scroll::scroll_panel;
pub use slot::{ItemSlot, item_slot};
pub use text_field::{TextField, text_field, typing};
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
                font::FontPlugin,
                screen::ScreenPlugin,
                pointer::PointerPlugin,
                button::ButtonPlugin,
                text_field::TextFieldPlugin,
                scroll::ScrollPlugin,
                icon::IconPlugin,
                slot::SlotPlugin,
                tooltip::TooltipPlugin,
            ));
        #[cfg(feature = "dev")]
        app.add_plugins(gallery::GalleryPlugin);
    }
}
