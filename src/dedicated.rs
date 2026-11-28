//! A dedicated server: `defaria server [--port N] [--world DIR] [--radius
//! N] [--data DIR]`. The server app on its own, with no window, sixty
//! frames a second, listening for clients over the network (`quic`). It
//! plays the world in `--world`, or the `server-world` folder in the
//! game's data folder, and saves it as the game does: as chunks unload,
//! every so often, and when it stops, its players with it (each as they
//! leave, too). Typing `stop` (and Enter), or Ctrl-C, stops it.
//!
//! Its key is the world's `server.pem`, made the first time, so clients
//! that remember it know it's the same server after a restart.
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
use defaria_server::server::{Incoming, ReceiveSystems};

use crate::{
    identity::{Identity, SERVER_KEY_FILE},
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
    let content = app.world().resource::<Content>().fingerprint();
    let address = SocketAddr::from(([0, 0, 0, 0], settings.port.unwrap_or(DEFAULT_PORT)));
    let listener = match quic::listen(address, content, &identity) {
        Ok(listener) => listener,
        Err(error) => {
            error!("can't listen on {address}: {error}");
            return AppExit::error();
        }
    };
    take_clients_from(&mut app, listener);
    app.insert_resource(Console(Mutex::new(console())))
        .add_systems(PreUpdate, read_console);
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

/// `stop` stops the server, saving the world.
fn read_console(console: Res<Console>, mut exit: MessageWriter<AppExit>) {
    let lines = console.0.lock().unwrap_or_else(|e| e.into_inner());
    for line in lines.try_iter() {
        match line.trim() {
            "stop" => {
                info!("stopping");
                exit.write(AppExit::Success);
            }
            "" => {}
            other => info!("unknown command: {other} (stop stops the server)"),
        }
    }
}
