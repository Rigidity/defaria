//! Joining a server, as the menus see it: the Multiplayer screen asks with
//! a [`Connect`] (or calls it off with a [`CancelConnect`]) and shows the
//! [`ConnectStatus`]. Connecting itself (the network, the server's mods,
//! building content from them) is the game's (`defaria::connecting`),
//! which does it in the background and keeps the status up to date, and
//! starts play once it's done; the window never waits on it.

use bevy::prelude::*;

pub struct ConnectingPlugin;

impl Plugin for ConnectingPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Connect>()
            .add_message::<CancelConnect>()
            .init_resource::<ConnectStatus>();
    }
}

/// A server, as the menus name it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerTarget {
    /// What it's called on the list (its address, without a name).
    pub name: String,
    /// Where it is: `host:port`.
    pub address: String,
}

/// Connect to this server (instead of any other being connected to), and
/// play there once it's done.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub struct Connect(pub ServerTarget);

/// Stop connecting.
#[derive(Message, Debug, Clone, Copy, Default)]
pub struct CancelConnect;

/// How far connecting has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
#[derive(Resource, Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectStatus {
    /// Not on a server, nor trying.
    #[default]
    Idle,
    Connecting {
        server: ServerTarget,
        step: ConnectStep,
    },
    /// Connecting failed: why. A server whose key isn't the one
    /// remembered says so (`changed_key`), so the screen can offer to
    /// trust the new one.
    Failed {
        server: ServerTarget,
        error: String,
        changed_key: bool,
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
    fn steps_say_how_far_they_are() {
        assert_eq!(
            ConnectStep::Fetching { done: 1, total: 5 }.describe(),
            "Downloading mods (2 of 5)..."
        );
        assert_eq!(ConnectStep::Connecting.describe(), "Connecting...");
    }
}
