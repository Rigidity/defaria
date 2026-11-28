//! Test helpers: the whole game with no GPU, driven by scripts.

use bevy::prelude::*;

use super::{HeadlessPlugin, SETTLE_TIMEOUT_FRAMES, Script, default_plugins, runner::ScriptRunner};
use defaria_core::{
    light::{ChunkLight, Light},
    player::{LocalPlayer, Look, Player, eyes},
    world::{ChunkCoord, ChunkMap},
};

/// The whole game with no GPU, running `script`, loading a smaller world
/// than the game does so tests are quick.
pub fn test_game(script: Script) -> App {
    let mut app = App::new();
    // Before the game's plugins, so its scripts load with them: what a
    // script may do then doesn't depend on how busy the machine is.
    app.insert_resource(defaria_core::script::ScriptSettings::untimed())
        .insert_resource(defaria_server::script::hooks::HookSettings::untimed());
    app.add_plugins((
        default_plugins(false).disable::<bevy::log::LogPlugin>(),
        crate::GamePlugin,
        HeadlessPlugin {
            script,
            rendering: false,
        },
    ))
    .insert_resource(defaria_core::streaming::StreamingSettings {
        radius: 4,
        ..default()
    });
    app.finish();
    app.cleanup();
    app
}

/// Runs `script` on a [`test_game`] app that has already finished its own,
/// updating it until this one has finished too, so a test can look at the
/// game between steps.
pub fn run_more(app: &mut App, script: &str) {
    let script = Script::parse(script).unwrap();
    app.world_mut()
        .resource_mut::<ScriptRunner>()
        .commands
        .extend(script.commands);
    for _ in 0..SETTLE_TIMEOUT_FRAMES {
        app.update();
        if app.world().resource::<ScriptRunner>().finished() {
            return;
        }
    }
    panic!("the script never finished");
}

pub use defaria_server::testing::settle;

/// Runs the whole game with no GPU until the script ends, and returns
/// how it exited.
pub(super) fn run_game(script: &str) -> (App, AppExit) {
    let mut app = test_game(Script::parse(script).unwrap());
    for _ in 0..SETTLE_TIMEOUT_FRAMES + 200 {
        app.update();
        if let Some(exit) = app.should_exit() {
            return (app, exit);
        }
    }
    panic!("script never finished");
}

pub(super) fn light_at(app: &mut App, pos: IVec3) -> Light {
    let (coord, local) = ChunkCoord::split(pos);
    let entity = app.world().resource::<ChunkMap>().get(coord).unwrap();
    app.world().get::<ChunkLight>(entity).unwrap().at(local)
}

/// Runs `script` in the test game until it ends.
pub(super) fn play(script: &str) -> App {
    let mut app = test_game(Script::parse(script).unwrap());
    let mut frames = 0;
    let exit = loop {
        app.update();
        frames += 1;
        if let Some(exit) = app.should_exit() {
            break exit;
        }
        assert!(frames < SETTLE_TIMEOUT_FRAMES, "script never finished");
    };
    assert_eq!(exit, AppExit::Success);
    app
}

/// The local player's look, and its eyes.
pub(super) fn camera(app: &mut App) -> (Look, Transform) {
    let (transform, player, look) = app
        .world_mut()
        .query_filtered::<(&Transform, &Player, &Look), With<LocalPlayer>>()
        .single(app.world())
        .unwrap();
    (*look, eyes(transform, player, look))
}

/// The world of the server a [`test_game`] plays on: where the world's
/// state is decided (block entities, networks, fluid flow, scripts). The
/// client's world has copies of the chunks and its player.
pub fn server(app: &mut App) -> &mut World {
    app.world_mut()
        .non_send_mut::<crate::IntegratedServer>()
        .into_inner()
        .app
        .world_mut()
}

/// [`server`], to read.
pub fn server_world(app: &App) -> &World {
    app.world()
        .non_send::<crate::IntegratedServer>()
        .app
        .world()
}

/// A server and clients in one process, connected by links in memory, and
/// stepped together a frame at a time (the clients, then the server, as
/// the server in the game's own process is): multiplayer, for tests.
pub struct Multiplayer {
    pub server: App,
    pub clients: Vec<App>,
    /// What's between each client and the server.
    relays: Vec<Relay>,
}

/// How the links between a [`Multiplayer`]'s clients and server behave,
/// to test the game under a bad connection, the same way every run.
#[derive(Debug, Clone, Copy, Default)]
pub struct Conditions {
    /// How many frames each message takes, each way.
    pub delay: u32,
    /// The share of the messages that may be lost (not `reliable()`) that
    /// are, each way.
    pub loss: f64,
}

/// What's between a client and the server: messages each way, encoded
/// and decoded as over the network, held back and lost as the
/// [`Conditions`] say.
struct Relay {
    /// The client's side: what it sends comes out here.
    client: defaria_core::net::ServerLink,
    /// The server's side: what it sends comes out here.
    server: defaria_core::net::ClientLink,
    to_server: std::collections::VecDeque<(u64, defaria_core::net::ClientMessage)>,
    to_client: std::collections::VecDeque<(u64, defaria_core::net::ServerMessage)>,
    conditions: Conditions,
    frame: u64,
    /// For losing messages: the same losses every run.
    random: u64,
}

impl Relay {
    /// Whether the next message that may be lost is.
    fn lose(&mut self) -> bool {
        // xorshift64
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        let unit = (self.random >> 11) as f64 / (1u64 << 53) as f64;
        unit < self.conditions.loss
    }

    /// Takes what each side sent, and passes on what's due.
    fn pump(&mut self) {
        use defaria_core::net::{decode, encode};
        let due = self.frame + self.conditions.delay as u64;
        for message in self.client.receive() {
            if !message.reliable() && self.lose() {
                continue;
            }
            self.to_server
                .push_back((due, decode(&encode(&message)).unwrap()));
        }
        for message in self.server.receive() {
            if !message.reliable() && self.lose() {
                continue;
            }
            self.to_client
                .push_back((due, decode(&encode(&message)).unwrap()));
        }
        while self
            .to_server
            .front()
            .is_some_and(|(at, _)| *at <= self.frame)
        {
            let (_, message) = self.to_server.pop_front().unwrap();
            self.server.send(message);
        }
        while self
            .to_client
            .front()
            .is_some_and(|(at, _)| *at <= self.frame)
        {
            let (_, message) = self.to_client.pop_front().unwrap();
            self.client.send(message);
        }
    }
}

/// A client with no server of its own and no script yet, loading a small
/// world: give it a `ServerConnection` to play.
pub fn test_client() -> App {
    let mut client = App::new();
    client
        .add_plugins((
            default_plugins(false).disable::<bevy::log::LogPlugin>(),
            crate::ClientPlugin,
            HeadlessPlugin {
                script: Script::default(),
                rendering: false,
            },
        ))
        .insert_resource(defaria_core::streaming::StreamingSettings {
            radius: 4,
            ..default()
        });
    client.finish();
    client.cleanup();
    client
}

/// The id of a [`Multiplayer`]'s client `index`: a made-up one, the same
/// every run, as if each were its own install.
pub fn test_player_id(index: usize) -> defaria_core::identity::PlayerId {
    defaria_core::identity::PlayerId([index as u8 + 1; 32])
}

/// A server app for a [`Multiplayer`], loading a small world: the one in
/// `world`, or one in memory.
pub fn test_server(world: Option<std::path::PathBuf>) -> App {
    test_server_with(world, None)
}

/// [`test_server`], playing with `content` rather than the mods on disk.
pub fn test_server_with(
    world: Option<std::path::PathBuf>,
    content: Option<defaria_core::content::Content>,
) -> App {
    let mut server = App::new();
    if let Some(content) = content {
        server.insert_resource(content);
    }
    server
        .insert_resource(defaria_core::script::ScriptSettings::untimed())
        .insert_resource(defaria_server::script::hooks::HookSettings::untimed())
        .insert_resource(defaria_core::streaming::StreamingSettings {
            radius: 4,
            ..default()
        });
    if let Some(world) = world {
        server.insert_resource(defaria_core::save::WorldFolder(world));
    }
    crate::server::build_server(&mut server);
    server.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        super::FRAME_TIME,
    ));
    server
}

/// A [`Multiplayer`] with `clients` clients, each with no script of its
/// own yet (see [`Multiplayer::run`]), loading a small world.
pub fn test_multiplayer(clients: usize) -> Multiplayer {
    test_multiplayer_over(clients, Conditions::default())
}

/// [`test_multiplayer`], each client's messages delayed and lost as
/// `conditions` say.
pub fn test_multiplayer_over(clients: usize, conditions: Conditions) -> Multiplayer {
    let mut server = test_server(None);
    let (clients, relays) = (0..clients)
        .map(|index| {
            let mut client = test_client();
            let (to_server, client_side) = defaria_core::net::link();
            let (server_side, at_server) = defaria_core::net::link();
            client.insert_resource(defaria_client::net::ServerConnection(to_server));
            server
                .world_mut()
                .resource_mut::<defaria_server::server::Incoming>()
                .0
                .push(defaria_server::server::Joining {
                    id: test_player_id(index),
                    name: format!("Player {}", index + 1),
                    link: at_server,
                });
            let relay = Relay {
                client: client_side,
                server: server_side,
                to_server: default(),
                to_client: default(),
                conditions,
                frame: 0,
                random: 0x9E37_79B9_7F4A_7C15 ^ index as u64,
            };
            (client, relay)
        })
        .unzip();
    Multiplayer {
        server,
        clients,
        relays,
    }
}

impl Multiplayer {
    /// `server`, with no clients yet: they join over the network (add each
    /// to `clients`).
    pub fn serving(server: App) -> Self {
        Self {
            server,
            clients: Vec::new(),
            relays: Vec::new(),
        }
    }

    /// One frame: each client's, then the server's.
    pub fn update(&mut self) {
        for client in &mut self.clients {
            client.update();
        }
        self.pump();
        self.server.update();
        self.pump();
        for relay in &mut self.relays {
            relay.frame += 1;
        }
    }

    /// Passes on what's due between the clients and the server.
    fn pump(&mut self) {
        for relay in &mut self.relays {
            relay.pump();
        }
    }

    /// Updates until every client has loaded what's around its player.
    pub fn settle(&mut self) {
        for _ in 0..SETTLE_TIMEOUT_FRAMES {
            self.update();
            let settled = self.clients.iter().all(|client| {
                client
                    .world()
                    .resource::<defaria_core::streaming::StreamingStatus>()
                    .settled
            });
            if settled {
                // A few more, for the meshes and what's sent last.
                for _ in 0..5 {
                    self.update();
                }
                return;
            }
        }
        panic!("the clients never settled");
    }

    /// Runs `script` on client `client`, updating everything until it's
    /// done.
    pub fn run(&mut self, client: usize, script: &str) {
        self.run_watching(client, script, |_| {});
    }

    /// [`Multiplayer::run`], calling `watch` after every frame.
    pub fn run_watching(
        &mut self,
        client: usize,
        script: &str,
        mut watch: impl FnMut(&mut Multiplayer),
    ) {
        let script = Script::parse(script).unwrap();
        self.clients[client]
            .world_mut()
            .resource_mut::<ScriptRunner>()
            .commands
            .extend(script.commands);
        for _ in 0..SETTLE_TIMEOUT_FRAMES {
            self.update();
            watch(self);
            if self.clients[client]
                .world()
                .resource::<ScriptRunner>()
                .finished()
            {
                return;
            }
        }
        panic!("the script never finished");
    }
}
