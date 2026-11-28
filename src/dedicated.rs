//! A dedicated server: `defaria server [--port N] [--world DIR] [--radius
//! N] [--data DIR]`. The server app on its own, with no window, sixty
//! frames a second, listening for clients over the network (`quic`). It
//! plays the world in `--world`, or the `server-world` folder in the
//! game's data folder, and saves it as the game does: as chunks unload,
//! every so often, and when it stops, its players with it (each as they
//! leave, too). Ctrl-C stops it.
//!
//! Its standard input is a console: each line is a server command
//! (`defaria_server::command`; a `/` before it is fine), run as the
//! console, which may run any, and what it answers is printed. `stop`
//! saves the world and stops it; `say` talks to the players; `op` lets a
//! player run commands too (the world's `ops.json5`).
//!
//! Its key is the world's `server.pem`, made the first time, so clients
//! that remember it know it's the same server after a restart.
//!
//! It sends its `both` mods to the clients that join (`mod_transfer`); a
//! mod that can't be sent (a file too big, or named oddly) stops it from
//! starting.
//!
//! It never pauses: unlike the server in the game's own process, it
//! doesn't pause with its players (`server::PauseWithPlayers`).

use std::{
    io::BufRead,
    net::SocketAddr,
    path::PathBuf,
    sync::{Mutex, mpsc},
};

use bevy::{app::TerminalCtrlCHandlerPlugin, log::LogPlugin, prelude::*};
use defaria_core::{content::Content, save::WorldFolder, streaming::StreamingSettings};
use defaria_server::{
    command,
    server::{Incoming, ReceiveSystems},
};

use crate::{
    identity::{Identity, SERVER_KEY_FILE},
    mod_transfer::Offer,
    quic::{self, DEFAULT_PORT, Listener},
};

/// How the server was asked to run.
pub struct DedicatedSettings {
    pub port: Option<u16>,
    pub world: Option<PathBuf>,
    pub radius: Option<i32>,
    /// The game's data folder, where the default world is.
    pub data: Option<PathBuf>,
}

/// Runs a dedicated server until it's stopped.
pub fn run(settings: DedicatedSettings) -> AppExit {
    let mut app = App::new();
    app.add_plugins((LogPlugin::default(), TerminalCtrlCHandlerPlugin));
    let folder = settings
        .world
        .or_else(|| Some(settings.data?.join("server-world")));
    let identity = match &folder {
        Some(folder) => {
            info!("the world is {}", folder.display());
            app.insert_resource(WorldFolder(folder.clone()));
            match server_identity(Some(folder)) {
                Ok(identity) => identity,
                Err(error) => {
                    error!("{error}");
                    return AppExit::error();
                }
            }
        }
        None => {
            warn!(
                "there's no folder to save the world in; it's kept in memory, and the \
                 server's key is new"
            );
            Identity::generate()
        }
    };
    if let Some(radius) = settings.radius {
        app.insert_resource(StreamingSettings {
            radius,
            ..default()
        });
    }
    crate::server::build_server(&mut app);
    // What clients are sent: every `both` mod. One that can't be sent
    // (too big, a file with an odd name) stops the server, rather than
    // turn every client away.
    let offer = match Offer::new(app.world().resource::<Content>()) {
        Ok(offer) => offer,
        Err(error) => {
            error!("{error}");
            return AppExit::error();
        }
    };
    let address = SocketAddr::from(([0, 0, 0, 0], settings.port.unwrap_or(DEFAULT_PORT)));
    let listener = match quic::listen(address, offer, &identity) {
        Ok(listener) => listener,
        Err(error) => {
            error!("can't listen on {address}: {error}");
            return AppExit::error();
        }
    };
    take_clients_from(&mut app, listener);
    app.insert_resource(Console(Mutex::new(console())))
        .add_systems(PreUpdate, read_console)
        .add_systems(Last, print_console_replies);
    let exit = app.run();
    // The world's saved; tell the players.
    if let Some(network) = app.world_mut().remove_resource::<Network>() {
        network.close();
    }
    exit
}

/// The key of the server for the world in `folder`: its `server.pem`,
/// made the first time. A world in memory has a new one each time.
pub fn server_identity(folder: Option<&std::path::Path>) -> Result<Identity, String> {
    match folder {
        Some(folder) => Identity::load_or_create(&folder.join(SERVER_KEY_FILE)),
        None => Ok(Identity::generate()),
    }
}

/// Has the server app `app` take the clients that connect to `listener` as
/// players.
pub fn take_clients_from(app: &mut App, listener: Listener) {
    app.insert_resource(Network(Mutex::new(listener)))
        .add_systems(PreUpdate, accept_clients.before(ReceiveSystems));
}

/// Where clients connect.
#[derive(Resource)]
pub(crate) struct Network(Mutex<Listener>);

impl Network {
    /// The address it listens at.
    #[cfg(test)]
    pub(crate) fn address(&self) -> SocketAddr {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).address
    }

    /// Closes every connection (see [`Listener::close`]), and returns the
    /// address it listened at.
    pub(crate) fn close(self) -> SocketAddr {
        let listener = self.0.into_inner().unwrap_or_else(|e| e.into_inner());
        listener.close();
        listener.address
    }
}

/// Lines typed at the server's console.
#[derive(Resource)]
struct Console(Mutex<mpsc::Receiver<String>>);

/// Reads the console's lines on a thread of their own.
fn console() -> mpsc::Receiver<String> {
    let (send, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if send.send(line).is_err() {
                break;
            }
        }
    });
    lines
}

/// Clients that connected join, as players.
fn accept_clients(network: Res<Network>, mut incoming: ResMut<Incoming>) {
    let listener = network.0.lock().unwrap_or_else(|e| e.into_inner());
    incoming.0.extend(listener.accept());
}

/// Hands the lines typed at the console to the server's commands, which
/// run them this frame.
fn read_console(console: Res<Console>, mut commands: ResMut<command::Console>) {
    let lines = console.0.lock().unwrap_or_else(|e| e.into_inner());
    for line in lines.try_iter() {
        if !line.trim().is_empty() {
            commands.lines.push(line);
        }
    }
}

/// Prints what the console's commands answered.
fn print_console_replies(mut commands: ResMut<command::Console>) {
    for reply in commands.replies.drain(..) {
        if reply.text.is_empty() {
            continue;
        }
        if reply.ok {
            println!("{}", reply.text);
        } else {
            eprintln!("{}", reply.text);
        }
    }
}
