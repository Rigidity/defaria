//! The game told to play on a server over the network (`--connect`, or a
//! server picked on the Multiplayer screen) joins it from the menu and
//! plays there as in single player: the world around it drawn, with its
//! textures, and the player carrying what the server says, its hotbar
//! showing it.

use std::{net::SocketAddr, thread, time::Duration};

use bevy::prelude::*;
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
    player::LocalPlayer,
    save::DataFolder,
    streaming::{StreamingSettings, StreamingStatus},
    testing::temp_folder,
    world::ChunkCoord,
};

use crate::{
    connecting::ConnectOnStart,
    dedicated::take_clients_from,
    headless::{Multiplayer, default_plugins, test_server_with},
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
