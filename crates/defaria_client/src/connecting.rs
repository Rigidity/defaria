//! Joining a server, as the menus see it: the Multiplayer screen asks with
//! a [`Connect`] (or calls it off with a [`CancelConnect`]) and shows the
//! [`ConnectStatus`]. Connecting itself (the network, the server's mods,
//! building content from them) is the game's (`defaria::connecting`),
//! which does it in the background and keeps the status up to date, and
//! starts play once it's done; the window never waits on it.

use bevy::prelude::*;

pub struct ConnectStatusPlugin;

impl Plugin for ConnectStatusPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Connect>()
            .add_message::<CancelConnect>()
            .register_type::<ConnectStatus>()
            .init_resource::<ConnectStatus>();
    }
}

/// Where the menus ask to connect or stop (Escape on the connecting
/// screen): before the game acts on what's asked, so a request is never a
/// frame late for it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConnectRequestSystems;

/// A server, as the menus name it.
#[derive(Reflect, Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerTarget {
    /// What it's called on the list (its address, without a name).
    pub name: String,
    /// Where it is: `host:port`.
    pub address: String,
}

/// Connect to this server (instead of any other being connected to), and
/// play there once it's done.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub struct Connect {
    pub server: ServerTarget,
    /// The key (its fingerprint, in hex) the player chose to trust in
    /// place of the one saved for this server: the server must show
    /// exactly this one, and once it has, it replaces the saved one.
    pub trust_key: Option<String>,
}

impl Connect {
    pub fn to(server: ServerTarget) -> Self {
        Self {
            server,
            trust_key: None,
        }
    }
}

/// Stop connecting.
#[derive(Message, Debug, Clone, Copy, Default)]
pub struct CancelConnect;

/// A key's fingerprint (hex) as the screens show it: its first 16 digits,
/// in groups of four, enough to tell keys apart by eye.
pub fn short_fingerprint(fingerprint: &str) -> String {
    let head: Vec<String> = (fingerprint.as_bytes().chunks(4).take(4))
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect();
    format!("{}...", head.join(" "))
}

/// How far connecting has got.
#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectStep {
    /// Reaching the server, and waiting for its list of mods.
    #[default]
    Connecting,
    /// Downloading the mods the game doesn't have: `done` of `total`.
    Fetching { done: usize, total: usize },
    /// Building the blocks and items from the server's mods.
    Loading,
    /// Waiting for the server to let the player in.
    Joining,
}

impl ConnectStep {
    /// What the connecting screen says.
    pub fn describe(self) -> String {
        match self {
            Self::Connecting => "Connecting...".into(),
            Self::Fetching { done, total } => {
                format!("Downloading mods ({} of {total})...", done + 1)
            }
            Self::Loading => "Loading the server's mods...".into(),
            Self::Joining => "Joining...".into(),
        }
    }
}

/// Where joining a server is.
#[derive(Resource, Reflect, Debug, Clone, PartialEq, Eq, Default)]
#[reflect(Resource)]
pub enum ConnectStatus {
    /// Not on a server, nor trying.
    #[default]
    Idle,
    Connecting {
        server: ServerTarget,
        step: ConnectStep,
    },
    /// Connecting failed: why. A server whose key isn't the one
    /// remembered says which it showed (`new_key`, its fingerprint in
    /// hex), so the screen can offer to trust exactly that one.
    Failed {
        server: ServerTarget,
        error: String,
        new_key: Option<String>,
    },
    /// Playing there.
    Playing { server: ServerTarget },
    /// The server went while the player was there: why.
    Lost {
        server: ServerTarget,
        reason: String,
    },
}

impl ConnectStatus {
    pub fn is_connecting(&self) -> bool {
        matches!(self, Self::Connecting { .. })
    }

    /// Whether the player is playing on a server, not alone.
    pub fn is_on_a_server(&self) -> bool {
        matches!(self, Self::Playing { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_are_shortened() {
        assert_eq!(
            short_fingerprint(&"ab12".repeat(16)),
            "ab12 ab12 ab12 ab12..."
        );
    }

    #[test]
    fn steps_say_how_far_they_are() {
        assert_eq!(
            ConnectStep::Fetching { done: 1, total: 5 }.describe(),
            "Downloading mods (2 of 5)..."
        );
        assert_eq!(ConnectStep::Connecting.describe(), "Connecting...");
    }
}
