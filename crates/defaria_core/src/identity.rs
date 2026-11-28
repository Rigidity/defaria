//! Who a player is: [`PlayerId`], the hash of their install's public key.
//! Each install makes a key pair once and presents it when it connects
//! (as a TLS client certificate, over the network), so the server learns
//! the id from the connection, never from what a client says; single
//! player uses the install's own id. Making and keeping keys, and hashing
//! them, is the game's (`defaria::identity`); this is only the id.
//!
//! An id is written as 64 lowercase hex digits: in a world's `players`
//! folder (`players/<id>.json5`) and as `player.id` in scripts.

use std::{fmt, str::FromStr};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// A player's id: the SHA-256 hash of their install's public key (its
/// DER `SubjectPublicKeyInfo`). The same install has the same id on
/// every server; the player's name is only shown.
#[derive(
    Component, Reflect, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[reflect(Component)]
pub struct PlayerId(pub [u8; 32]);

impl fmt::Display for PlayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

impl fmt::Debug for PlayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PlayerId({self})")
    }
}

impl FromStr for PlayerId {
    type Err = String;

    /// 64 hex digits, either case.
    fn from_str(text: &str) -> Result<Self, String> {
        parse_hex(text)
            .map(Self)
            .ok_or_else(|| format!("`{text}` isn't a player id (64 hex digits)"))
    }
}

/// `bytes` as lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    use fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// 32 bytes from 64 hex digits, either case, or `None`.
pub fn parse_hex(text: &str) -> Option<[u8; 32]> {
    let text = text.as_bytes();
    if text.len() != 64 {
        return None;
    }
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut bytes = [0; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = digit(text[2 * i])? << 4 | digit(text[2 * i + 1])?;
    }
    Some(bytes)
}

/// The longest a player's name may be, in characters.
pub const MAX_NAME: usize = 32;
/// The name of a player who didn't give one.
pub const DEFAULT_NAME: &str = "Player";

/// Whether `c` is a Unicode format character (general category Cf): one
/// that shows nothing itself but changes how text around it shows, such as
/// bidirectional overrides and zero-width characters, which could make
/// one name look like another.
pub fn is_format(c: char) -> bool {
    matches!(
        c as u32,
        0xAD | 0x600..=0x605
            | 0x61C
            | 0x6DD
            | 0x70F
            | 0x890..=0x891
            | 0x8E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

/// `name` as it's shown: trimmed, without control or format characters
/// ([`is_format`]), at most [`MAX_NAME`] characters, and [`DEFAULT_NAME`]
/// if nothing's left. Names come from clients, so the server cleans them
/// too.
pub fn clean_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| !c.is_control() && !is_format(*c))
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_NAME)
        .collect();
    match clean.trim() {
        "" => DEFAULT_NAME.to_string(),
        clean => clean.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_written_as_hex_and_read_back() {
        let mut bytes = [0; 32];
        bytes[0] = 0xab;
        bytes[31] = 0x07;
        let id = PlayerId(bytes);
        let text = id.to_string();
        assert_eq!(text.len(), 64);
        assert!(text.starts_with("ab00"), "{text}");
        assert!(text.ends_with("07"), "{text}");
        assert_eq!(text.parse::<PlayerId>(), Ok(id));
        assert_eq!(text.to_uppercase().parse::<PlayerId>(), Ok(id));
        assert!("ab".parse::<PlayerId>().is_err());
        assert!("g".repeat(64).parse::<PlayerId>().is_err());
        // Not ASCII, but 64 bytes long.
        assert!("é".repeat(32).parse::<PlayerId>().is_err());
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  Ada \n"), "Ada");
        assert_eq!(clean_name("a\u{7}b"), "ab");
        // Bidi overrides, zero-width characters and the like.
        assert_eq!(clean_name("Ad\u{202E}a\u{200B}\u{2066}\u{FEFF}"), "Ada");
        assert_eq!(clean_name("\u{200D}\u{E0041}"), DEFAULT_NAME);
        assert_eq!(clean_name("Zoë 日本"), "Zoë 日本");
        assert_eq!(clean_name(""), DEFAULT_NAME);
        assert_eq!(clean_name(" \t "), DEFAULT_NAME);
        assert_eq!(clean_name(&"x".repeat(100)).chars().count(), MAX_NAME);
    }
}
