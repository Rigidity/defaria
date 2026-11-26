//! Simulated input: the keys and characters a script's buttons and
//! `type` stand for, and what the mouse pointer points at.

use bevy::{
    camera::{NormalizedRenderTarget, RenderTarget},
    input::keyboard::{Key, NativeKey},
    prelude::*,
    window::WindowRef,
};

use super::{
    capture::HeadlessTarget,
    script::{Button, parse_button},
};

/// What a key means on a US layout, and the text it types, as winit
/// would report it, so text fields and shortcuts see real keys.
pub(super) fn logical_key(key_code: KeyCode) -> (Key, Option<String>) {
    let named = match key_code {
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Enter | KeyCode::NumpadEnter => Key::Enter,
        KeyCode::Escape => Key::Escape,
        KeyCode::Tab => Key::Tab,
        KeyCode::ArrowLeft => Key::ArrowLeft,
        KeyCode::ArrowRight => Key::ArrowRight,
        KeyCode::ArrowUp => Key::ArrowUp,
        KeyCode::ArrowDown => Key::ArrowDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Key::Shift,
        KeyCode::ControlLeft | KeyCode::ControlRight => Key::Control,
        KeyCode::AltLeft | KeyCode::AltRight => Key::Alt,
        KeyCode::SuperLeft | KeyCode::SuperRight => Key::Super,
        KeyCode::Space => return (Key::Space, Some(" ".into())),
        _ => {
            return match character_for(key_code) {
                Some(character) => {
                    let text = character.to_string();
                    (Key::Character(text.clone().into()), Some(text))
                }
                None => (Key::Unidentified(NativeKey::Unidentified), None),
            };
        }
    };
    (named, None)
}

/// The letter or digit a key types, unshifted.
fn character_for(key_code: KeyCode) -> Option<char> {
    let name = format!("{key_code:?}");
    let character = name
        .strip_prefix("Key")
        .or_else(|| name.strip_prefix("Digit"))
        .filter(|rest| rest.len() == 1)?
        .chars()
        .next()?;
    Some(character.to_ascii_lowercase())
}

/// The key that types `character`, for the letters and digits; others
/// are unidentified.
pub(super) fn key_for(character: char) -> KeyCode {
    use bevy::input::keyboard::NativeKeyCode;
    let name = if character.is_ascii_alphabetic() {
        format!("Key{}", character.to_ascii_uppercase())
    } else if character.is_ascii_digit() {
        format!("Digit{character}")
    } else if character == ' ' {
        return KeyCode::Space;
    } else {
        return KeyCode::Unidentified(NativeKeyCode::Unidentified);
    };
    parse_button(&name)
        .ok()
        .and_then(|button| match button {
            Button::Key(key) => Some(key),
            Button::Mouse(_) => None,
        })
        .unwrap_or(KeyCode::Unidentified(NativeKeyCode::Unidentified))
}

/// What the mouse pointer points at: the image the cameras render into,
/// or with no renderer the primary window.
pub(super) fn pointer_target(
    image: Option<&HeadlessTarget>,
    window: Entity,
) -> Option<NormalizedRenderTarget> {
    match image {
        Some(image) => RenderTarget::Image(image.0.clone().into()).normalize(None),
        None => RenderTarget::Window(WindowRef::Primary).normalize(Some(window)),
    }
}
