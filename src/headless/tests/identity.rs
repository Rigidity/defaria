//! Players are who their installs' keys say, over the network: the same
//! install is the same player, with what they had, after leaving and
//! after the server restarts; another install is someone else; and a
//! server whose key changed is refused.

use std::{net::SocketAddr, path::Path, thread, time::Duration};

use bevy::prelude::*;
use defaria_client::net::ServerConnection;
use defaria_core::{
    content::Content,
    game_mode::GameMode,
    identity::PlayerId,
    physics::Body,
    player::{LocalPlayer, START_POSITION},
    streaming::StreamingStatus,
    testing::temp_folder,
};
use defaria_server::{save::player as file, server::Connection};

use crate::{
    dedicated::{server_identity, take_clients_from},
    headless::{Multiplayer, test_client, test_server},
    identity::LocalIdentity,
    integrated::connect_remembering,
    mod_transfer::{Library, Offer},
    quic::{self, ConnectError, Listener},
};

/// A server for the world in `world` (in memory without), with its key,
/// listening at `address`.
fn start(world: Option<&Path>, address: SocketAddr) -> (Multiplayer, Listener) {
    let game = Multiplayer::serving(test_server(world.map(Path::to_path_buf)));
    let offer = Offer::new(game.server.world().resource::<Content>()).unwrap();
    let key = server_identity(world).unwrap();
    // The address may take a moment to be free again after a server there
    // stops.
    for _ in 0..100 {
        match quic::listen(address, offer.clone(), &key) {
            Ok(listener) => return (game, listener),
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    }
    panic!("can't listen on {address}");
}

/// Joins `game` at `address` as the install in `data`, and waits for its
/// world to load.
fn join(game: &mut Multiplayer, address: &str, data: &Path) -> Result<(), ConnectError> {
    let library = Library::new(game.server.world().resource::<Content>(), Some(data));
    let local = LocalIdentity::load(Some(data));
    let link = connect_remembering(address, library, &local, Some(data))?.link;
    let mut client = test_client();
    client.insert_resource(ServerConnection(link));
    game.clients.push(client);
    let joined = game.clients.len() - 1;
    for _ in 0..6000 {
        game.update();
        thread::sleep(Duration::from_millis(1));
        if game.clients[joined]
            .world()
            .resource::<StreamingStatus>()
            .settled
        {
            // A moment more, for where the player is to arrive.
            for _ in 0..30 {
                game.update();
                thread::sleep(Duration::from_millis(1));
            }
            return Ok(());
        }
    }
    panic!("never settled over the network");
}

/// Runs `script` on client `client`, giving the network a moment a frame.
fn run(game: &mut Multiplayer, client: usize, script: &str) {
    game.run_watching(client, script, |_| thread::sleep(Duration::from_millis(1)));
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

/// The server's player with id `id`: its game mode and feet.
fn on_server(game: &mut Multiplayer, id: PlayerId) -> Option<(GameMode, bevy::math::DVec3)> {
    let world = game.server.world_mut();
    let mut players = world.query::<(&PlayerId, &GameMode, &Body)>();
    players
        .iter(world)
        .find(|(playing, ..)| **playing == id)
        .map(|(_, mode, body)| (*mode, body.position))
}

#[test]
fn an_install_is_the_same_player_across_reconnects_and_restarts() {
    let world = temp_folder("identity-world");
    let data = temp_folder("identity-install");
    let other = temp_folder("identity-other-install");
    let id = LocalIdentity::load(Some(&data)).identity.id;

    let (mut game, listener) = start(Some(&world), "127.0.0.1:0".parse().unwrap());
    let address = listener.address.to_string();
    take_clients_from(&mut game.server, listener);
    join(&mut game, &address, &data).unwrap();
    assert!(on_server(&mut game, id).is_some(), "joined as its key's id");

    // Somewhere else, in survival.
    run(
        &mut game,
        0,
        "camera 80.5 70 100.5 0 0\nwait 20\nhold MouseLeft 1\nwait 1\nhold KeyC 1\nwait 20",
    );
    let (mode, _) = on_server(&mut game, id).unwrap();
    assert_eq!(mode, GameMode::Survival);

    // It leaves, and is saved as it goes.
    let left_at = on_server(&mut game, id).unwrap().1;
    game.clients.pop();
    until(&mut game, |game| {
        let world = game.server.world_mut();
        world.query::<&Connection>().iter(world).next().is_none()
    });
    let saved = file::read(&world, id).unwrap().expect("saved on leaving");
    assert_eq!(saved.game_mode, GameMode::Survival);
    let feet = saved.position.unwrap().feet;
    assert!((feet[0] - 80.5).abs() < 0.01 && (feet[2] - 100.5).abs() < 0.01);
    assert!(left_at.distance(bevy::math::DVec3::from_array(feet)) < 1.0);

    // The server stops, and starts again at the same address, with the same
    // key: the install trusts it, and is who it was, where it was.
    game.server.world_mut().write_message(AppExit::Success);
    game.server.update();
    let network = game
        .server
        .world_mut()
        .remove_resource::<crate::dedicated::Network>()
        .unwrap();
    let address_was = network.close();
    drop(game);
    let (mut game, listener) = start(Some(&world), address_was);
    take_clients_from(&mut game.server, listener);
    join(&mut game, &address, &data).unwrap();
    let (mode, at) = on_server(&mut game, id).unwrap();
    assert_eq!(mode, GameMode::Survival);
    assert_eq!(at.to_array(), feet, "back where it left");
    let client = game.clients[0].world_mut();
    let mut local = client.query_filtered::<(&Body, &GameMode), With<LocalPlayer>>();
    let (body, mode) = local.single(client).unwrap();
    assert_eq!(*mode, GameMode::Survival);
    assert!(
        (body.position.x - 80.5).abs() < 0.01 && (body.position.z - 100.5).abs() < 0.01,
        "the client's player is where the server put it: {}",
        body.position
    );

    // Another install is someone else, new here.
    join(&mut game, &address, &other).unwrap();
    let other_id = LocalIdentity::load(Some(&other)).identity.id;
    assert_ne!(other_id, id);
    let (mode, at) = on_server(&mut game, other_id).unwrap();
    assert_eq!(mode, GameMode::Creative);
    assert!((at.x - START_POSITION.x as f64).abs() < 0.01, "{at}");
}

/// A server at an address the install knows, whose key isn't the one it
/// remembers, is refused; the install still trusts the one it knew.
#[test]
fn a_server_whose_key_changed_is_refused() {
    let data = temp_folder("identity-changed-install");
    let (mut game, listener) = start(None, "127.0.0.1:0".parse().unwrap());
    let address = listener.address.to_string();
    let bound = listener.address;
    take_clients_from(&mut game.server, listener);
    join(&mut game, &address, &data).unwrap();
    let remembered = defaria_core::server_list::ServerList::load(Some(&data))
        .unwrap()
        .fingerprint(&address)
        .expect("remembered the first time");
    game.clients.pop();
    let network = game
        .server
        .world_mut()
        .remove_resource::<crate::dedicated::Network>()
        .unwrap();
    network.close();
    drop(game);

    // Another server (a world in memory makes a new key) at the same address.
    let (mut game, listener) = start(None, bound);
    take_clients_from(&mut game.server, listener);
    let error = join(&mut game, &address, &data).unwrap_err();
    assert!(
        matches!(&error, ConnectError::ChangedKey { expected, .. } if *expected == remembered),
        "{error}"
    );
    assert!(error.to_string().contains("servers.json5"), "{error}");
    let still = defaria_core::server_list::ServerList::load(Some(&data)).unwrap();
    assert_eq!(still.fingerprint(&address), Some(remembered));
}

/// An install whose key can't be read doesn't join a server as a stand-in
/// (a stray player there), and says why; nothing is remembered.
#[test]
fn a_damaged_key_joins_no_server() {
    let data = temp_folder("identity-damaged-join");
    std::fs::write(data.join(crate::identity::KEY_FILE), "not a key").unwrap();
    let (mut game, listener) = start(None, "127.0.0.1:0".parse().unwrap());
    let address = listener.address.to_string();
    take_clients_from(&mut game.server, listener);
    let error = join(&mut game, &address, &data).unwrap_err();
    assert!(
        error.to_string().contains("key couldn't be read"),
        "{error}"
    );
    assert!(!data.join(defaria_core::server_list::SERVERS_FILE).exists());
}

/// A first visit whose key can't be saved doesn't join: otherwise every
/// visit would be a first, trusting any key.
#[cfg(unix)]
#[test]
fn a_key_that_cant_be_saved_joins_no_server() {
    use std::os::unix::fs::PermissionsExt;
    let data = temp_folder("identity-unsaved-key");
    // The install's own files first, then a folder nothing can be written to.
    LocalIdentity::load(Some(&data));
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o500)).unwrap();
    let (mut game, listener) = start(None, "127.0.0.1:0".parse().unwrap());
    let address = listener.address.to_string();
    take_clients_from(&mut game.server, listener);
    let error = join(&mut game, &address, &data);
    std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    let error = error.unwrap_err();
    assert!(error.to_string().contains("couldn't save"), "{error}");
}
