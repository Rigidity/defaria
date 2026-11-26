//! The UI's pixel font, Pixel Operator (CC0; see `assets/fonts/SOURCES.md`).
//!
//! It takes the place of Bevy's default font, so every [`TextFont`] that
//! doesn't name a font uses it, Bevy's own widgets included. Titles use its
//! bold cut, [`UiTheme::title_font`]. Both are built into the game, so text
//! has its font from the first frame, and in tests.
//!
//! The font is drawn on a 16-pixel grid: sizes that are multiples of 16
//! keep its pixels whole, and [`UiTheme::text_font`] turns off smoothing,
//! which would blur them.

use bevy::{prelude::*, text::FontSmoothing};

use super::UiTheme;

const REGULAR: &[u8] = include_bytes!("../../assets/fonts/PixelOperator.ttf");
const BOLD: &[u8] = include_bytes!("../../assets/fonts/PixelOperator-Bold.ttf");

pub struct FontPlugin;

impl Plugin for FontPlugin {
    fn build(&self, app: &mut App) {
        // Without the text plugin (tests of single systems) there's no text.
        let Some(mut fonts) = app.world_mut().get_resource_mut::<Assets<Font>>() else {
            return;
        };
        if let Err(error) = fonts.insert(AssetId::default(), Font::from_bytes(REGULAR.to_vec())) {
            error!("can't replace the default font: {error}");
        }
        let bold = fonts.add(Font::from_bytes(BOLD.to_vec()));
        app.world_mut().resource_mut::<UiTheme>().bold = bold;
    }
}

impl UiTheme {
    /// Body text, at [`UiTheme::font_size`], unsmoothed.
    pub fn text_font(&self) -> TextFont {
        TextFont {
            font_smoothing: FontSmoothing::None,
            ..TextFont::from_font_size(self.font_size)
        }
    }

    /// Titles: the bold cut, at [`UiTheme::title_font_size`].
    pub fn title_font(&self) -> TextFont {
        TextFont {
            font: self.bold.clone().into(),
            font_size: self.title_font_size.into(),
            ..self.text_font()
        }
    }
}
