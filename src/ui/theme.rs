//! Sizes and colors every widget shares, so screens look alike and a
//! restyle is one place.

use bevy::prelude::*;

/// The look of the UI. Widgets read it when they're spawned.
#[derive(Resource, Debug, Clone)]
pub struct UiTheme {
    /// Width and height of an item slot, border included, in logical pixels.
    pub slot_size: f32,
    /// The item icon inside a slot.
    pub icon_size: f32,
    pub slot_border: f32,
    /// Space between neighbouring slots.
    pub slot_gap: f32,
    pub panel_padding: f32,
    pub font_size: f32,
    pub title_font_size: f32,

    pub panel: Color,
    pub panel_border: Color,
    /// Behind a screen, over the world.
    pub backdrop: Color,
    pub slot: Color,
    pub slot_edge: Color,
    pub slot_hovered: Color,
    pub slot_selected: Color,
    pub text: Color,
    pub text_dim: Color,
    pub tooltip: Color,
    pub scrollbar_track: Color,
    pub scrollbar_thumb: Color,
}

impl Default for UiTheme {
    fn default() -> Self {
        Self {
            slot_size: 52.0,
            icon_size: 40.0,
            slot_border: 2.0,
            slot_gap: 4.0,
            panel_padding: 14.0,
            font_size: 16.0,
            title_font_size: 20.0,

            panel: Color::srgba(0.11, 0.11, 0.13, 0.94),
            panel_border: Color::srgba(1.0, 1.0, 1.0, 0.12),
            backdrop: Color::srgba(0.0, 0.0, 0.0, 0.35),
            slot: Color::srgba(0.0, 0.0, 0.0, 0.45),
            slot_edge: Color::srgba(1.0, 1.0, 1.0, 0.10),
            slot_hovered: Color::srgba(1.0, 1.0, 1.0, 0.55),
            slot_selected: Color::srgb(0.95, 0.85, 0.35),
            text: Color::srgb(0.92, 0.92, 0.92),
            text_dim: Color::srgb(0.6, 0.6, 0.62),
            tooltip: Color::srgba(0.05, 0.05, 0.07, 0.95),
            scrollbar_track: Color::srgba(1.0, 1.0, 1.0, 0.06),
            scrollbar_thumb: Color::srgba(1.0, 1.0, 1.0, 0.35),
        }
    }
}
