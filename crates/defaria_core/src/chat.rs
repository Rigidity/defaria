//! Chat: what players say to everyone on the server, and what the server
//! answers a command with. A client sends a line ([`ClientMessage::Chat`]
//! or, for a line starting with `/`, [`ClientMessage::Command`]); the
//! server cleans it ([`clean_text`]), holds each player to
//! [`MAX_MESSAGE`] characters and a rate ([`CHAT_BURST`],
//! [`CHAT_PER_SECOND`]), and sends every player a [`ChatLine`]
//! ([`ServerMessage::Chat`]). Clients are untrusted: the server checks
//! everything here again.
//!
//! [`ClientMessage::Chat`]: crate::net::ClientMessage::Chat
//! [`ClientMessage::Command`]: crate::net::ClientMessage::Command
//! [`ServerMessage::Chat`]: crate::net::ServerMessage::Chat

use crate::identity::is_format;

/// The longest a chat message or command may be, in characters.
pub const MAX_MESSAGE: usize = 256;
/// How many messages a player may send at once, after a quiet spell.
pub const CHAT_BURST: f32 = 8.0;
/// How many a second they may keep sending after that.
pub const CHAT_PER_SECOND: f32 = 1.0;

/// A line of chat, as the server sends it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChatLine {
    pub kind: ChatKind,
    /// Who said it: a player's name, or `None` for the server.
    pub from: Option<String>,
    pub text: String,
}

/// What a [`ChatLine`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChatKind {
    /// A player (or the console, `say`) said it to everyone.
    Said,
    /// The server says something happened: a player joined or left.
    Notice,
    /// What a command the player ran answered.
    Output,
    /// A command the player ran failed or was refused, or a message
    /// wasn't sent: why.
    Refused,
}

/// What the server answers a command with: one reply each, carrying the
/// id the client gave the command, so whatever sent it (the chat line,
/// a scenario) knows the reply is its own.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CommandReply {
    /// The command's id, as the client sent it.
    pub id: u32,
    /// Whether it ran.
    pub ok: bool,
    /// What it says: its output, or why it didn't run.
    pub text: String,
    /// For a `fill`, how many voxels it changed and how many nanoseconds
    /// setting them took (for a scenario's `timing`).
    pub filled: Option<(u64, u64)>,
}

impl CommandReply {
    /// A command that ran, saying `text`.
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            id: 0,
            ok: true,
            text: text.into(),
            filled: None,
        }
    }

    /// A command that didn't run, and why.
    pub fn refused(text: impl Into<String>) -> Self {
        Self {
            id: 0,
            ok: false,
            text: text.into(),
            filled: None,
        }
    }

    /// The reply as a line of chat.
    pub fn line(&self) -> ChatLine {
        ChatLine {
            kind: if self.ok {
                ChatKind::Output
            } else {
                ChatKind::Refused
            },
            from: None,
            text: self.text.clone(),
        }
    }
}

/// `text` as it's sent on: without control or format characters (as
/// names are, see [`crate::identity::clean_name`]), trimmed. Empty if
/// nothing's left.
pub fn clean_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() && !is_format(*c))
        .collect::<String>()
        .trim()
        .to_string()
}

/// How many messages a player may still send: [`CHAT_BURST`] at most,
/// refilling at [`CHAT_PER_SECOND`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Allowance(f32);

impl Default for Allowance {
    fn default() -> Self {
        Self(CHAT_BURST)
    }
}

impl Allowance {
    /// `seconds` have gone by.
    pub fn refill(&mut self, seconds: f32) {
        self.0 = (self.0 + seconds * CHAT_PER_SECOND).min(CHAT_BURST);
    }

    /// Takes one message's worth, if there's one left.
    pub fn take(&mut self) -> bool {
        if self.0 >= 1.0 {
            self.0 -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_cleaned_of_control_and_format_characters() {
        assert_eq!(clean_text("  hello \n"), "hello");
        assert_eq!(clean_text("a\u{7}b\u{202E}c\u{200B}"), "abc");
        assert_eq!(clean_text("Zoë 日本"), "Zoë 日本");
        assert_eq!(clean_text("\u{1b}[31m"), "[31m");
        assert_eq!(clean_text(" \t\u{FEFF} "), "");
    }

    #[test]
    fn the_allowance_bursts_then_refills_slowly() {
        let mut allowance = Allowance::default();
        let sent = (0..20).filter(|_| allowance.take()).count();
        assert_eq!(sent, CHAT_BURST as usize);
        allowance.refill(0.5);
        assert!(!allowance.take());
        allowance.refill(0.5);
        assert!(allowance.take());
        allowance.refill(1000.0);
        assert_eq!(allowance, Allowance(CHAT_BURST));
    }
}
