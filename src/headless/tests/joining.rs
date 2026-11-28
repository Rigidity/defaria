//! The game told to play on a server over the network (`--connect`, or a
//! server picked on the Multiplayer screen) joins it from the menu and
//! plays there as in single player: the world around it drawn, with its
//! textures, and the player carrying what the server says, its hotbar
//! showing it.

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use bevy::{math::DVec3, prelude::*};
use defaria_client::{
    hotbar_ui::HotbarSlot,
    session::{GameState, StartAtMenu},
    ui::{icon::ItemIcons, slot::ItemSlot},
    voxel_material::BlockTiles,
};
use defaria_core::{
    content::Content,
    hotbar::{ActiveSlot, Hotbar},
    inventory::Inventory,
    physics::Body,
    player::{EYE_HEIGHT, LocalPlayer, START_POSITION},
    save::DataFolder,
    streaming::{StreamingSettings, StreamingStatus},
    testing::temp_folder,
    world::{ChunkCoord, ChunkMap},
};

use crate::{
    connecting::ConnectOnStart,
    dedicated::take_clients_from,
    headless::{
        FRAME_TIME, HeadlessPlugin, Multiplayer, Script, default_plugins, test_server_with,
    },
    identity::Identity,
    mod_transfer::Offer,
    quic,
};

/// A server on the network, and the game set to join it as it starts.
fn joining() -> Multiplayer {
    let mut game = Multiplayer::serving(test_server_with(None, None));
    let content = game.server.world().resource::<Content>().clone();
    let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener =
        quic::listen(local, Offer::new(&content).unwrap(), &Identity::generate()).unwrap();
    let address = listener.address.to_string();
    take_clients_from(&mut game.server, listener);

    let mut client = App::new();
    client
        .insert_resource(ConnectOnStart {
            address,
            exit_on_failure: false,
        })
        .insert_resource(StartAtMenu)
        .insert_resource(DataFolder(temp_folder("joining")))
        .add_plugins((
            default_plugins(false).disable::<bevy::log::LogPlugin>(),
            crate::GamePlugin,
        ))
        .insert_resource(StreamingSettings {
            radius: 4,
            ..default()
        });
    client.finish();
    client.cleanup();
    game.clients.push(client);
    game
}

/// Updates until `done`, giving the network a moment a frame.
fn until(game: &mut Multiplayer, what: &str, mut done: impl FnMut(&mut App) -> bool) {
    for _ in 0..6000 {
        if done(&mut game.clients[0]) {
            return;
        }
        game.update();
        thread::sleep(Duration::from_millis(1));
    }
    panic!("never {what}");
}

#[test]
fn a_client_that_joins_from_the_menu_plays_there() {
    let mut game = joining();
    until(&mut game, "played", |client| {
        *client.world().resource::<State<GameState>>().get() == GameState::Playing
    });
    until(&mut game, "settled", |client| {
        client.world().resource::<StreamingStatus>().settled
    });
    for _ in 0..5 {
        game.update();
    }
    let client = &mut game.clients[0];

    // The player carries what the server gave it, and its hotbar shows
    // it. (The server's first word on the player came before the player
    // was there to take it.)
    let (inventory, hotbar) = client
        .world_mut()
        .query_filtered::<(&Inventory, &Hotbar), With<LocalPlayer>>()
        .single(client.world())
        .map(|(inventory, hotbar)| (inventory.clone(), hotbar.clone()))
        .expect("the server's state of the player was applied");
    let first = hotbar
        .item(ActiveSlot(0))
        .expect("a hotbar with something in it");
    assert!(inventory.count(first) > 0);
    let shown: Vec<ItemSlot> = client
        .world_mut()
        .query::<(&HotbarSlot, &ItemSlot)>()
        .iter(client.world())
        .filter(|(slot, _)| slot.0 == 0)
        .map(|(_, item)| *item)
        .collect();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].item, Some(first));
    assert!(shown[0].count > 0);

    // The block texture was built from the server's content, and the
    // icons drawn from it.
    assert!(!client.world().resource::<BlockTiles>().0.is_empty());
    let icon = client.world().resource::<ItemIcons>().get(first).unwrap();
    assert!(
        client
            .world()
            .resource::<Assets<Image>>()
            .get(icon)
            .is_some()
    );

    // The chunks around the player were meshed, with faces to draw.
    let mut chunks = client
        .world_mut()
        .query_filtered::<&Mesh3d, With<ChunkCoord>>();
    let meshes = client.world().resource::<Assets<Mesh>>();
    let vertices: usize = chunks
        .iter(client.world())
        .filter_map(|mesh| meshes.get(mesh))
        .map(Mesh::count_vertices)
        .sum();
    assert!(vertices > 0, "nothing drawn");
}

/// A server on the network that runs on its own thread in real time, as
/// a dedicated server does, until `stop` is set; it stalls for a moment
/// (as a busy server can) whenever `stall` is set.
struct RealTimeServer {
    address: String,
    stop: Arc<AtomicBool>,
    stall: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}

/// How long a [`RealTimeServer`] stalls.
const STALL: Duration = Duration::from_millis(1500);

fn real_time_server() -> RealTimeServer {
    let (address_tx, address_rx) = std::sync::mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stall = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let stalling = stall.clone();
    let thread = thread::spawn(move || {
        let mut server = test_server_with(None, None);
        let content = server.world().resource::<Content>().clone();
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener =
            quic::listen(local, Offer::new(&content).unwrap(), &Identity::generate()).unwrap();
        address_tx.send(listener.address.to_string()).unwrap();
        take_clients_from(&mut server, listener);
        while !stopping.load(Ordering::Relaxed) {
            if stalling.swap(false, Ordering::Relaxed) {
                thread::sleep(STALL);
            }
            let start = Instant::now();
            server.update();
            if let Some(rest) = FRAME_TIME.checked_sub(start.elapsed()) {
                thread::sleep(rest);
            }
        }
    });
    RealTimeServer {
        address: address_rx.recv().unwrap(),
        stop,
        stall,
        thread,
    }
}

/// The original failure, end to end: a headless run on a server over the
/// network jumps far from where it starts (`camera`) and `settle`s. It
/// settles there, with the player where it jumped to and the world around
/// it loaded, not around where it started. The client runs its frames as
/// a headless run does, back to back, while the server steps in real
/// time. The server stalls just as the player jumps, so for a while all
/// it has said is that the world is loaded around where the player was:
/// the client mustn't take that for settled.
#[test]
fn a_camera_jump_on_a_server_settles_where_it_went() {
    let server = real_time_server();
    let script = Script::parse("camera 600 80 600 0 -30\nsettle\n").unwrap();
    let mut client = App::new();
    client
        .insert_resource(ConnectOnStart {
            address: server.address.clone(),
            exit_on_failure: true,
        })
        .insert_resource(StartAtMenu)
        .insert_resource(DataFolder(temp_folder("camera-jump")))
        .add_plugins((
            default_plugins(false).disable::<bevy::log::LogPlugin>(),
            crate::GamePlugin,
            HeadlessPlugin {
                script,
                rendering: false,
            },
        ))
        .insert_resource(StreamingSettings {
            radius: 4,
            ..default()
        });
    client.finish();
    client.cleanup();
    let started = Instant::now();
    let mut jumped = false;
    let exit = loop {
        client.update();
        if let Some(exit) = client.should_exit() {
            break exit;
        }
        if !jumped && local_feet(&mut client).x > 500.0 {
            jumped = true;
            server.stall.store(true, Ordering::Relaxed);
        }
        assert!(started.elapsed() < Duration::from_secs(120), "never ended");
    };
    server.stop.store(true, Ordering::Relaxed);
    server.thread.join().unwrap();
    assert!(jumped);
    assert_eq!(exit, AppExit::Success);

    let feet = local_feet(&mut client);
    let world = client.world_mut();
    let jumped = DVec3::new(600.0, 80.0 - EYE_HEIGHT, 600.0);
    assert!(feet.distance(jumped) < 1.0, "the player is at {feet}");
    let there = ChunkCoord::split(IVec3::new(600, 80, 600)).0.0;
    let settings = world.resource::<StreamingSettings>().clone();
    let map = world.resource::<ChunkMap>();
    let missing = (settings.window(there))
        .filter(|&coord| map.get(ChunkCoord(coord)).is_none())
        .count();
    assert_eq!(missing, 0, "chunks around where it went still missing");
    let start = ChunkCoord::split(START_POSITION.floor().as_ivec3()).0;
    assert!(map.get(start).is_none(), "still has where it started");
}

/// Where the local player's feet are, or the origin before there's one.
fn local_feet(client: &mut App) -> DVec3 {
    let world = client.world_mut();
    world
        .query_filtered::<&Body, With<LocalPlayer>>()
        .single(world)
        .map_or(DVec3::ZERO, |body| body.position)
}
