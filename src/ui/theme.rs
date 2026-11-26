//! Sizes and colors every widget shares, so screens look alike and a
//! restyle is one place.

use bevy::prelude::*;

/// The look of the UI. Widgets read it when they're spawned; the colours
/// that change with state (a slot's frame) are read as they change.
#[derive(Resource, Debug, Clone)]
pub struct UiTheme {
    /// Width and height of an item slot, border included, in logical pixels.
    pub slot_size: f32,
    /// The item icon inside a slot. A multiple of 16, so flat 16-pixel
    /// item icons scale up by a whole number.
    pub icon_size: f32,
    pub slot_border: f32,
    /// Space between neighbouring slots.
    pub slot_gap: f32,
    pub panel_padding: f32,
    /// Inside a button, left and right; half that above and below.
    pub button_padding: f32,
    /// Inside a text field.
    pub field_padding: f32,
    pub scrollbar_width: f32,
    /// Between a scrolled area and its scrollbar.
    pub scrollbar_gap: f32,
    /// The shortest a scrollbar's thumb gets, however long the content.
    pub scrollbar_min_thumb: f32,
    /// Multiples of 16, for the pixel font (see [`super::font`]).
    pub font_size: f32,
    pub title_font_size: f32,
    /// The pixel font's bold cut, for titles. Set when the font loads.
    pub bold: Handle<Font>,

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
    pub button: Color,
    pub button_edge: Color,
    pub button_hovered: Color,
    pub button_pressed: Color,
    pub field: Color,
    /// A text field's frame while it has the keyboard.
    pub field_focused: Color,
}

impl Default for UiTheme {
    fn default() -> Self {
        Self {
            slot_size: 56.0,
            icon_size: 48.0,
            slot_border: 2.0,
            slot_gap: 4.0,
            panel_padding: 14.0,
            button_padding: 12.0,
            field_padding: 6.0,
            scrollbar_width: 6.0,
            scrollbar_gap: 4.0,
            scrollbar_min_thumb: 24.0,
            font_size: 16.0,
            title_font_size: 16.0,
            bold: Handle::default(),

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
            button: Color::srgba(1.0, 1.0, 1.0, 0.08),
            button_edge: Color::srgba(1.0, 1.0, 1.0, 0.18),
            button_hovered: Color::srgba(1.0, 1.0, 1.0, 0.16),
            button_pressed: Color::srgba(1.0, 1.0, 1.0, 0.04),
            field: Color::srgba(0.0, 0.0, 0.0, 0.45),
            field_focused: Color::srgb(0.95, 0.85, 0.35),
        }
    }
}
