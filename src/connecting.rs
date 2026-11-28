//! Joining a server, in the background: the Multiplayer screen asks
//! (`defaria_client::connecting::Connect`), or `--connect` does
//! ([`ConnectOnStart`]), and a thread of its own connects, takes the
//! server's mods (fetching those the game lacks into the data folder's
//! cache) and builds the content from them, saying how far it's got in
//! the [`ConnectStatus`] the screen shows. The window goes on drawing
//! meanwhile. Once it's done the client plays there: the content is
//! swapped in, the link to the server put in place, and play starts
//! ([`RemoteServer`]: no server starts in the game's own process). A
//! [`CancelConnect`] (or another server asked for) calls it off: the
//! connection is closed and whatever it made is dropped; a flag it checks
//! between steps that can't be interrupted (building content, saving the
//! server's key) keeps it from joining or remembering anything after.
//!
//! A failure says why in the status (and `--connect` in a headless run
//! exits with it). Leaving the server returns to the Multiplayer screen;
//! a server that goes while the player's there says why
//! ([`ConnectStatus::Lost`]).

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use bevy::{ecs::message::MessageCursor, prelude::*};
use defaria_client::{
    connecting::{
        CancelConnect, Connect, ConnectRequestSystems, ConnectStatus, ConnectStep, ServerTarget,
        short_fingerprint,
    },
    content_swap::swap_content,
    net::ServerConnection,
    session::GameState,
};
use defaria_core::{
    content::Content,
    identity::parse_hex,
    save::DataFolder,
    server_list::{Fingerprint, normalize_address},
};

use crate::{
    identity::local_identity,
    integrated::{SinglePlayerContent, connect_remembering_with},
    mod_transfer::Library,
    quic::{CANCELLED, CloseReason, ConnectError, ConnectOptions, Connected, HANDSHAKE_TIMEOUT},
};

pub struct BackgroundConnectPlugin;

impl Plugin for BackgroundConnectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConnectSettings>()
            .add_systems(Update, drive_connecting.after(ConnectRequestSystems));
    }
}

/// How connecting goes.
#[derive(Resource, Debug, Clone)]
pub struct ConnectSettings {
    /// How long a server has to answer before it's given up on (the mods
    /// it sends have longer).
    pub timeout: Duration,
}

impl Default for ConnectSettings {
    fn default() -> Self {
        Self {
            timeout: HANDSHAKE_TIMEOUT,
        }
    }
}

/// Connect to this server as the game starts (`--connect`), rather than
/// waiting at the menu. With `exit_on_failure` (a headless run, which has
/// nobody to read the error), failing exits the game with an error.
#[derive(Resource, Debug, Clone)]
pub struct ConnectOnStart {
    pub address: String,
    pub exit_on_failure: bool,
}

/// The server the client plays on, over the network, instead of one in
/// the game's own process.
#[derive(Resource, Debug, Clone)]
pub struct RemoteServer {
    pub server: ServerTarget,
    /// Why the connection ended, once it has.
    pub closed: CloseReason,
}

/// Joining a server, on a thread of its own: how far it's got, and what it
/// came to once it's done. Dropping it calls it off.
#[derive(Resource)]
struct Attempt {
    server: ServerTarget,
    step: Arc<Mutex<ConnectStep>>,
    result: Arc<Mutex<Option<Result<Connected, ConnectError>>>>,
    exit_on_failure: bool,
    /// Calls it off when it goes.
    _cancel: smol::channel::Sender<()>,
    /// Set when it's called off (dropped).
    cancelled: Arc<AtomicBool>,
    /// The thread, to tell one that ended without saying how.
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Attempt {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl Attempt {
    /// Starts joining `server` from `world`'s client: as its player, with
    /// its own mods where they're the server's, and its data folder's
    /// saved servers and cache.
    fn start(
        world: &mut World,
        server: ServerTarget,
        trust_key: Option<Fingerprint>,
        exit_on_failure: bool,
    ) -> Self {
        let local = local_identity(world);
        let folder = world.get_resource::<DataFolder>().map(|f| f.0.clone());
        let own = world
            .get_resource::<SinglePlayerContent>()
            .map(|own| &own.0)
            .unwrap_or_else(|| world.resource::<Content>());
        let library = Library::new(own, folder.as_deref());
        let step = Arc::new(Mutex::new(ConnectStep::Connecting));
        let result = Arc::new(Mutex::new(None));
        let (cancel_send, cancel) = smol::channel::bounded(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let options = ConnectOptions {
            timeout: Some(world.resource::<ConnectSettings>().timeout),
            step: Some(step.clone()),
            cancel: Some(cancel),
            cancelled: Some(cancelled.clone()),
        };
        info!("connecting to {}", server.address);
        let address = server.address.clone();
        let done = result.clone();
        let spawned = std::thread::Builder::new()
            .name("connecting".into())
            .spawn(move || {
                let outcome = guarded(|| {
                    connect_remembering_with(
                        &address,
                        library,
                        &local,
                        folder.as_deref(),
                        trust_key,
                        options,
                    )
                });
                *done.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
            });
        let thread = match spawned {
            Ok(thread) => Some(thread),
            Err(error) => {
                *result.lock().unwrap_or_else(|e| e.into_inner()) = Some(Err(
                    ConnectError::Failed(format!("can't start connecting: {error}")),
                ));
                None
            }
        };
        Self {
            server,
            step,
            result,
            exit_on_failure,
            _cancel: cancel_send,
            cancelled,
            thread,
        }
    }

    fn step(&self) -> ConnectStep {
        *self.step.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What it came to, once it's done (a thread that ended without
    /// saying, an error).
    fn take(&self) -> Option<Result<Connected, ConnectError>> {
        let ended = self
            .thread
            .as_ref()
            .is_some_and(|thread| thread.is_finished());
        let outcome = self.result.lock().unwrap_or_else(|e| e.into_inner()).take();
        match outcome {
            None if ended => Some(Err(ConnectError::Failed(
                "connecting stopped without saying why".into(),
            ))),
            outcome => outcome,
        }
    }
}

/// What `work` comes to, or, if it panics, an error saying so: a
/// connection that fails that way must still end, not leave the screen
/// connecting for ever.
fn guarded(
    work: impl FnOnce() -> Result<Connected, ConnectError>,
) -> Result<Connected, ConnectError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|panic| {
        let what = (panic.downcast_ref::<&str>().map(|s| s.to_string()))
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "no message".to_string());
        Err(ConnectError::Failed(format!(
            "connecting failed unexpectedly (a bug: {what})"
        )))
    })
}

/// What the menus asked for since last frame: the last server to join, and
/// whether to stop.
#[derive(Default)]
struct Asked {
    connects: MessageCursor<Connect>,
    cancels: MessageCursor<CancelConnect>,
}

/// Starts, calls off and finishes joining a server, and keeps the status
/// up to date as it goes.
fn drive_connecting(world: &mut World, mut asked: Local<Asked>) {
    let cancelled = world
        .get_resource::<Messages<CancelConnect>>()
        .is_some_and(|messages| asked.cancels.read(messages).count() > 0);
    let wanted = world
        .get_resource::<Messages<Connect>>()
        .and_then(|messages| asked.connects.read(messages).last().cloned());
    if cancelled && world.remove_resource::<Attempt>().is_some() {
        info!("stopped connecting");
        set_status(world, ConnectStatus::Idle);
    }
    let start = world
        .remove_resource::<ConnectOnStart>()
        .map(|start| {
            let server = ServerTarget {
                name: start.address.clone(),
                address: start.address,
            };
            (server, None, start.exit_on_failure)
        })
        .or(wanted.map(|connect| (connect.server, connect.trust_key, false)));
    if let Some((mut server, trust_key, exit_on_failure)) = start {
        let trust_key = match trust_key.map(|key| parse_hex(&key).map(Fingerprint)) {
            Some(None) => Err("that key isn't a fingerprint".to_string()),
            Some(key) => Ok(key),
            None => Ok(None),
        };
        match trust_key.and_then(|key| Ok((normalize_address(&server.address)?, key))) {
            Ok((address, trust_key)) => {
                if server.name == server.address {
                    server.name.clone_from(&address);
                }
                server.address = address;
                // Another under way is called off.
                world.remove_resource::<Attempt>();
                let attempt = Attempt::start(world, server.clone(), trust_key, exit_on_failure);
                set_status(
                    world,
                    ConnectStatus::Connecting {
                        server,
                        step: attempt.step(),
                    },
                );
                world.insert_resource(attempt);
            }
            Err(error) => {
                fail(world, server, ConnectError::Failed(error), exit_on_failure);
            }
        }
    }
    let Some(attempt) = world.get_resource::<Attempt>() else {
        return;
    };
    let Some(outcome) = attempt.take() else {
        let status = ConnectStatus::Connecting {
            server: attempt.server.clone(),
            step: attempt.step(),
        };
        set_status(world, status);
        return;
    };
    let attempt = world.remove_resource::<Attempt>().expect("it's there");
    match outcome {
        Ok(connected) => play_there(world, attempt.server.clone(), connected),
        Err(error) => fail(
            world,
            attempt.server.clone(),
            error,
            attempt.exit_on_failure,
        ),
    }
}

/// Sets the status, if it's changed (so the screens see a change only when
/// there is one).
fn set_status(world: &mut World, status: ConnectStatus) {
    if let Some(mut current) = world.get_resource_mut::<ConnectStatus>() {
        current.set_if_neq(status);
    }
}

/// Joined: plays with the server's content, over its link.
fn play_there(world: &mut World, server: ServerTarget, connected: Connected) {
    info!(
        "playing on {}, with its {} mods ({} fetched)",
        server.address,
        connected.content.mods.len(),
        connected.fetched.len()
    );
    if let Some(own) = swap_content(world, connected.content)
        && !world.contains_resource::<SinglePlayerContent>()
    {
        world.insert_resource(SinglePlayerContent(own));
    }
    world.insert_resource(ServerConnection(connected.link));
    world.insert_resource(RemoteServer {
        server: server.clone(),
        closed: connected.closed,
    });
    set_status(world, ConnectStatus::Playing { server });
    if let Some(mut next) = world.get_resource_mut::<NextState<GameState>>() {
        next.set(GameState::Playing);
    }
}

/// Couldn't join: says why.
fn fail(world: &mut World, server: ServerTarget, error: ConnectError, exit: bool) {
    error!("can't connect to {}: {error}", server.address);
    if exit {
        world.write_message(AppExit::error());
    }
    let new_key = match &error {
        ConnectError::ChangedKey { found, .. } => Some(found.to_string()),
        ConnectError::Failed(_) => None,
    };
    let error = match error {
        ConnectError::Failed(reason) if reason == CANCELLED => return,
        // The log has both fingerprints whole; the screen, what to do.
        ConnectError::ChangedKey { expected, .. } => format!(
            "its key isn't the one saved for it ({}), so it may be someone else's server. \
             If its owner says they changed its key, trust the new one",
            short_fingerprint(&expected.to_string())
        ),
        error => error.to_string(),
    };
    set_status(
        world,
        ConnectStatus::Failed {
            server,
            error,
            new_key,
        },
    );
}

/// Leaving a server's world: the status says whether the server went (and
/// why) or the player left.
pub(crate) fn leave_the_server(world: &mut World) {
    let Some(remote) = world.remove_resource::<RemoteServer>() else {
        return;
    };
    let went = world
        .get_resource::<ServerConnection>()
        .is_some_and(|connection| connection.0.is_closed());
    if went {
        let reason = remote
            .closed
            .get()
            .unwrap_or_else(|| "the connection to the server was lost".to_string());
        warn!("{} went: {reason}", remote.server.address);
        set_status(
            world,
            ConnectStatus::Lost {
                server: remote.server,
                reason,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connecting thread that panics comes to an error, which the
    /// screen shows, rather than never finishing.
    #[test]
    fn a_panic_while_connecting_is_an_error() {
        let outcome = guarded(|| panic!("injected"));
        let Err(error) = outcome else {
            panic!("it connected");
        };
        assert!(error.to_string().contains("injected"), "{error}");
    }
}
