//! A server's mods reach a client that has none of them, over the network:
//! it builds the same block and item states from them and plays with
//! them, the server's `server` mods never leave it, and what the client
//! fetched once is in its cache the next time.

use std::{net::SocketAddr, thread, time::Duration};

use bevy::{ecs::system::RunSystemOnce, prelude::*};
use defaria_client::{content_swap::swap_content, net::ServerConnection, session::GameState};
use defaria_core::{
    content::{BlockState, Content, load::ModFiles},
    script::ScriptRuntime,
    streaming::StreamingStatus,
    testing::{temp_folder, test_mods},
    world::Voxels,
};

use crate::{
    dedicated::take_clients_from,
    headless::{Multiplayer, test_client, test_server_with},
    identity::Identity,
    mod_transfer::{CACHE_DIR, Library, Offer},
    quic,
};

/// Mod `test`'s block with three states, so every block after it would be
/// numbered differently without it.
const TINTED: (&str, &str) = (
    "blocks/tinted.json5",
    r#"{ properties: { tint: ["a", "b", "c"] },
         model: "defaria:cube", textures: { all: "defaria:red_lamp" } }"#,
);

/// The server's mods: the base game, mod `test` (a block, and a script
/// for both sides), and `admin`, a `server` mod.
fn server_content() -> Content {
    let mut mods = test_mods(&[TINTED, ("scripts/greet.luau", "-- on both sides")]);
    mods.push(ModFiles::new(
        "mods/admin",
        r#"{ id: "admin", name: "Admin", side: "server", dependencies: ["defaria"] }"#,
        &[("scripts/admin.luau", "-- the server's alone")],
    ));
    let content = Content::from_mods(mods);
    assert!(content.errors.is_empty(), "{:#?}", content.errors);
    content
}

/// Every block state's and item state's name, in index order.
fn state_table(content: &Content) -> Vec<String> {
    let blocks = content.states.iter().map(|state| {
        format!(
            "{}{:?}",
            content.blocks[state.block].id,
            state.values.iter().collect::<Vec<_>>()
        )
    });
    let items = content.item_states.iter().map(|state| state.name.clone());
    blocks.chain(items).collect()
}

fn voxel(app: &mut App, pos: IVec3) -> Option<BlockState> {
    app.world_mut()
        .run_system_once(move |voxels: Voxels| voxels.get(pos))
        .unwrap()
}

/// Updates until `done`, giving the network a moment a frame.
fn until(game: &mut Multiplayer, mut done: impl FnMut(&mut Multiplayer) -> bool) {
    for _ in 0..6000 {
        if done(game) {
            return;
        }
        game.update();
        thread::sleep(Duration::from_millis(1));
    }
    panic!("it never happened");
}

fn scripts(app: &App) -> Vec<String> {
    let runtime = app.world().resource::<ScriptRuntime>();
    runtime.sources().sources.keys().cloned().collect()
}

#[test]
fn a_client_with_no_mods_plays_with_the_servers() {
    let content = server_content();
    let offer = Offer::new(&content).unwrap();
    let offered: Vec<&str> = offer.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        offered,
        ["defaria", "test"],
        "a `server` mod is never offered"
    );

    let mut game = Multiplayer::serving(test_server_with(None, Some(content.clone())));
    let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = quic::listen(local, offer.clone(), &Identity::generate()).unwrap();
    let address = listener.address.to_string();
    take_clients_from(&mut game.server, listener);
    let data = temp_folder("mod-transfer-install");
    let me = Identity::generate();

    // A client with no mods of its own fetches every one offered.
    let connected =
        quic::connect(&address, Library::empty(Some(&data)), &me, "Tester", None).unwrap();
    let mut fetched = connected.fetched.clone();
    fetched.sort();
    let mut all: Vec<_> = offer.entries.iter().map(|e| e.hash).collect();
    all.sort();
    assert_eq!(fetched, all);
    assert_eq!(state_table(&connected.content), state_table(&content));
    assert_eq!(connected.content.fingerprint(), content.fingerprint());
    let mods: Vec<&str> = connected
        .content
        .mods
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(mods, ["defaria", "test"]);
    for entry in &offer.entries {
        let cached = data.join(CACHE_DIR).join(entry.hash.to_string());
        assert!(cached.join("mod.json5").is_file(), "{} is cached", entry.id);
    }

    // It plays with them: the scripts of both sides' mods load, the
    // server's own don't, and a block of the test mod's reaches it as
    // the same state.
    let mut client = test_client();
    swap_content(client.world_mut(), connected.content);
    client.insert_resource(ServerConnection(connected.link));
    game.clients.push(client);
    until(&mut game, |game| {
        game.clients[0]
            .world()
            .resource::<StreamingStatus>()
            .settled
    });
    let on_client = scripts(&game.clients[0]);
    assert!(
        on_client.contains(&"test:greet".to_string()),
        "{on_client:?}"
    );
    assert!(
        !on_client.iter().any(|id| id.starts_with("admin:")),
        "{on_client:?}"
    );
    assert!(scripts(&game.server).contains(&"admin:admin".to_string()));

    let tinted = content
        .block_state("test:tinted", &[("tint", "c")])
        .unwrap();
    let at = IVec3::new(64, 60, 88);
    game.server
        .world_mut()
        .run_system_once(move |mut voxels: Voxels| voxels.set(at, tinted))
        .unwrap();
    until(&mut game, |game| {
        voxel(&mut game.clients[0], at) == Some(tinted)
    });
    let client_content = game.clients[0].world().resource::<Content>();
    assert_eq!(client_content.block_of(tinted).id, "test:tinted");

    // Joining again, it has every mod in its cache.
    let again = quic::connect(&address, Library::empty(Some(&data)), &me, "Tester", None).unwrap();
    assert!(again.fetched.is_empty(), "{:?}", again.fetched);
    assert_eq!(again.content.fingerprint(), content.fingerprint());
}

/// A client with the base game of its own fetches only what it lacks.
#[test]
fn a_client_fetches_only_what_it_lacks() {
    let content = server_content();
    let offer = Offer::new(&content).unwrap();
    let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = quic::listen(local, offer.clone(), &Identity::generate()).unwrap();
    let own = defaria_core::content::base();
    let connected = quic::connect(
        &listener.address.to_string(),
        Library::new(own, None),
        &Identity::generate(),
        "Tester",
        None,
    )
    .unwrap();
    let test = offer.entries.iter().find(|e| e.id == "test").unwrap();
    assert_eq!(connected.fetched, [test.hash]);
    assert_eq!(connected.content.fingerprint(), content.fingerprint());
    // The base game is its own copy, on disk.
    assert_eq!(connected.content.mods[0].dir, "mods/defaria");
}

/// The game, told to play on a server (`--connect`), connects in the
/// background, plays with the content of that server's mods once it has,
/// and with its own again once it leaves.
#[test]
fn the_game_swaps_in_a_servers_content_and_back() {
    let content = server_content();
    let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener =
        quic::listen(local, Offer::new(&content).unwrap(), &Identity::generate()).unwrap();
    let data = temp_folder("mod-transfer-game");
    let mut app = App::new();
    app.insert_resource(crate::connecting::ConnectOnStart {
        address: listener.address.to_string(),
        exit_on_failure: false,
    })
    .insert_resource(defaria_client::session::StartAtMenu)
    .insert_resource(defaria_core::save::DataFolder(data.clone()))
    .add_plugins((
        crate::headless::default_plugins(false).disable::<bevy::log::LogPlugin>(),
        crate::GamePlugin,
    ));
    app.finish();
    app.cleanup();
    let own = app.world().resource::<Content>().fingerprint();
    assert_ne!(own, content.fingerprint());

    // It connects, and takes the server's mods, while the menu's frames go
    // on; then plays.
    app.update();
    assert_eq!(
        *app.world().resource::<State<GameState>>().get(),
        GameState::Menu
    );
    for _ in 0..5000 {
        if *app.world().resource::<State<GameState>>().get() == GameState::Playing {
            break;
        }
        app.update();
        thread::sleep(Duration::from_millis(1));
    }
    let playing = app.world().resource::<Content>();
    assert_eq!(playing.fingerprint(), content.fingerprint());
    assert!(playing.block("test:tinted").is_some());
    assert!(app.world().contains_resource::<ServerConnection>());
    assert!(scripts(&app).contains(&"test:greet".to_string()));

    // Leaving, it has its own again.
    app.world_mut()
        .resource_mut::<NextState<GameState>>()
        .set(GameState::Menu);
    app.update();
    let back = app.world().resource::<Content>();
    assert_eq!(back.fingerprint(), own);
    assert!(back.block("test:tinted").is_none());
    assert!(!scripts(&app).contains(&"test:greet".to_string()));
    drop(listener);
}
