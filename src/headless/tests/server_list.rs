//! The Multiplayer screen, in the game: servers added, edited and removed
//! through it; joining one over the network, in the background, and
//! leaving back to it; a failed join and a server that goes saying why;
//! and a changed key trusted again.

use std::{
    net::SocketAddr,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    thread,
    time::Duration,
};

use bevy::prelude::*;
use defaria_client::{
    connecting::{Connect, ConnectStatus, ServerTarget},
    menu::{
        MenuError, MenuPage,
        multiplayer::{AddressField, ConnectErrorText, NewKeyText, ServerNameField},
    },
    session::GameState,
    ui::TextField,
};
use defaria_core::{
    content::Content,
    identity::parse_hex,
    save::DataFolder,
    server_list::{Fingerprint, SavedServer, ServerList},
    streaming::StreamingStatus,
    testing::temp_folder,
};

use super::menus::{click, page, state, type_into};
use crate::{
    connecting::{ConnectOnStart, ConnectSettings},
    dedicated::{Network, take_clients_from},
    headless::{Script, run_more, settle, test_game, test_server},
    identity::{Identity, LocalIdentity},
    integrated::connect_remembering_with,
    mod_transfer::{Library, Offer},
    quic::{self, ConnectOptions},
};

/// A game playing alone, with a data folder of its own, gone to the
/// Multiplayer screen.
fn at_the_servers(name: &str) -> (App, std::path::PathBuf) {
    let data = temp_folder(name);
    let mut app = test_game(Script::parse("wait 1").unwrap());
    app.insert_resource(DataFolder(data.clone()));
    settle(&mut app);
    run_more(&mut app, "hold MouseLeft 1\nwait 1\nhold Escape 1\nwait 2");
    click(&mut app, "Main menu");
    click(&mut app, "Multiplayer");
    assert_eq!(page(&mut app), Some(MenuPage::Servers));
    (app, data)
}

fn saved(data: &Path) -> Vec<SavedServer> {
    ServerList::load(Some(data)).unwrap().servers().to_vec()
}

fn status(app: &App) -> ConnectStatus {
    app.world().resource::<ConnectStatus>().clone()
}

fn texts<T: Component>(app: &mut App) -> Vec<String> {
    let mut texts = app.world_mut().query_filtered::<&Text, With<T>>();
    texts.iter(app.world()).map(|text| text.0.clone()).collect()
}

fn field<T: Component>(app: &mut App) -> String {
    let mut fields = app.world_mut().query_filtered::<&TextField, With<T>>();
    fields.single(app.world()).unwrap().value.clone()
}

/// Updates the game (and `server`, if there is one) until `done`, giving
/// the network a moment a frame.
fn until(app: &mut App, mut server: Option<&mut App>, mut done: impl FnMut(&mut App) -> bool) {
    for _ in 0..8000 {
        if done(app) {
            return;
        }
        app.update();
        if let Some(server) = server.as_deref_mut() {
            server.update();
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("it never happened");
}

/// A server playing a world in memory with a key of its own, listening at
/// `address`.
fn serve(address: SocketAddr) -> App {
    let mut server = test_server(None);
    let offer = Offer::new(server.world().resource::<Content>()).unwrap();
    // The address may take a moment to be free again after a server there
    // stops.
    for _ in 0..100 {
        if let Ok(listener) = quic::listen(address, offer.clone(), &Identity::generate()) {
            take_clients_from(&mut server, listener);
            return server;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("can't listen on {address}");
}

fn address_of(server: &App) -> SocketAddr {
    server.world().resource::<Network>().address()
}

#[test]
fn servers_are_added_edited_and_removed() {
    let (mut app, data) = at_the_servers("server-list-ui");
    assert!(saved(&data).is_empty());

    click(&mut app, "Add server");
    assert_eq!(page(&mut app), Some(MenuPage::AddServer));
    type_into::<ServerNameField>(&mut app, "Home");
    type_into::<AddressField>(&mut app, "localhost");
    click(&mut app, "Add");
    assert_eq!(page(&mut app), Some(MenuPage::Servers));
    click(&mut app, "Add server");
    type_into::<ServerNameField>(&mut app, "Far");
    type_into::<AddressField>(&mut app, "example.org:7000");
    click(&mut app, "Add");
    let shown: Vec<_> = saved(&data).iter().map(|s| s.address.clone()).collect();
    assert_eq!(shown, ["localhost:25575", "example.org:7000"]);

    // The same address again: refused, saying why, and nothing added.
    click(&mut app, "Add server");
    type_into::<AddressField>(&mut app, "localhost:25575");
    click(&mut app, "Add");
    assert_eq!(page(&mut app), Some(MenuPage::AddServer));
    let error = texts::<MenuError>(&mut app).concat();
    assert!(error.contains("already on the list, as Home"), "{error}");
    assert_eq!(saved(&data).len(), 2);
    click(&mut app, "Back");

    // Edited: it shows what it has, and keeps its place.
    click(&mut app, "Edit: Home");
    assert_eq!(page(&mut app), Some(MenuPage::EditServer));
    assert_eq!(field::<ServerNameField>(&mut app), "Home");
    assert_eq!(field::<AddressField>(&mut app), "localhost:25575");
    type_into::<ServerNameField>(&mut app, "stead");
    click(&mut app, "Save");
    assert_eq!(page(&mut app), Some(MenuPage::Servers));
    assert_eq!(saved(&data)[0].name, "Homestead");
    assert_eq!(saved(&data)[0].address, "localhost:25575");

    // Removed.
    click(&mut app, "Edit: Far");
    click(&mut app, "Remove");
    assert_eq!(page(&mut app), Some(MenuPage::Servers));
    let left: Vec<_> = saved(&data).iter().map(|s| s.name.clone()).collect();
    assert_eq!(left, ["Homestead"]);
    click(&mut app, "Server: Homestead");
    // (Nothing listens there; connecting is called off.)
    run_more(&mut app, "wait 2\nhold Escape 1\nwait 2");
    assert_eq!(page(&mut app), Some(MenuPage::Servers));
}

/// A server that doesn't answer: the screen shows it connecting, while
/// the game goes on, then why it failed, with the way back; Cancel and
/// Escape call connecting off.
#[test]
fn a_failed_join_says_why() {
    let (mut app, data) = at_the_servers("server-list-fail");
    // A port that's taken, by something that never answers.
    let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = silent.local_addr().unwrap().to_string();
    ServerList::load(Some(&data))
        .unwrap()
        .add("Nowhere", &address)
        .unwrap();
    // The scenario commands for it.
    run_more(&mut app, "connect-timeout 0.3\nwait 1");
    click(&mut app, "Back");
    click(&mut app, "Multiplayer");

    click(&mut app, "Server: Nowhere");
    assert_eq!(page(&mut app), Some(MenuPage::Connecting));
    assert!(status(&app).is_connecting());
    run_more(&mut app, "joined\nwait 2");
    assert_eq!(page(&mut app), Some(MenuPage::ConnectError));
    assert_eq!(state(&app), GameState::Menu);
    let error = texts::<ConnectErrorText>(&mut app).concat();
    assert!(error.contains("didn't answer"), "{error}");
    assert!(matches!(
        status(&app),
        ConnectStatus::Failed { new_key: None, .. }
    ));
    click(&mut app, "Back");
    assert_eq!(page(&mut app), Some(MenuPage::Servers));
    assert_eq!(status(&app), ConnectStatus::Idle);

    // Called off before it fails, with Cancel and with Escape: it never
    // says it failed.
    app.insert_resource(ConnectSettings {
        timeout: Duration::from_secs(30),
    });
    click(&mut app, "Server: Nowhere");
    click(&mut app, "Cancel");
    assert_eq!(page(&mut app), Some(MenuPage::Servers));
    assert_eq!(status(&app), ConnectStatus::Idle);
    click(&mut app, "Server: Nowhere");
    run_more(&mut app, "hold Escape 1\nwait 30");
    assert_eq!(page(&mut app), Some(MenuPage::Servers));
    assert_eq!(status(&app), ConnectStatus::Idle);
    drop(silent);
}

/// `--connect` in a headless run, to a server that doesn't answer, exits
/// with an error rather than waiting at the menu.
#[test]
fn a_failed_connect_on_start_exits() {
    let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut app = App::new();
    app.insert_resource(ConnectOnStart {
        address: silent.local_addr().unwrap().to_string(),
        exit_on_failure: true,
    })
    .insert_resource(ConnectSettings {
        timeout: Duration::from_millis(200),
    })
    .insert_resource(defaria_client::session::StartAtMenu)
    .add_plugins((
        crate::headless::default_plugins(false).disable::<bevy::log::LogPlugin>(),
        crate::GamePlugin,
    ));
    app.finish();
    app.cleanup();
    for _ in 0..5000 {
        app.update();
        if let Some(exit) = app.should_exit() {
            assert_eq!(exit, AppExit::error());
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("it never gave up");
}

/// Joining from the screen: over the network, then playing there; the
/// pause menu's **Disconnect** comes back to the list, and a server that
/// stops says so. Another server at that address, with another key, is
/// refused until its new key is trusted.
#[test]
fn joining_from_the_screen_plays_there() {
    let (mut app, data) = at_the_servers("server-list-join");
    let mut server = serve("127.0.0.1:0".parse().unwrap());
    let address = address_of(&server);
    ServerList::load(Some(&data))
        .unwrap()
        .add("Local", &address.to_string())
        .unwrap();
    click(&mut app, "Back");
    click(&mut app, "Multiplayer");

    let joined = |app: &mut App| {
        state(app) == GameState::Playing && app.world().resource::<StreamingStatus>().settled
    };
    click(&mut app, "Server: Local");
    until(&mut app, Some(&mut server), joined);
    assert!(status(&app).is_on_a_server());
    assert!(
        app.world()
            .get_non_send::<crate::IntegratedServer>()
            .is_none()
    );
    let remembered = ServerList::load(Some(&data))
        .unwrap()
        .fingerprint(&address.to_string());
    assert!(remembered.is_some(), "its key is remembered");

    // Disconnect: back at the list.
    run_more(&mut app, "hold MouseLeft 1\nwait 1\nhold Escape 1\nwait 2");
    click(&mut app, "Disconnect");
    assert_eq!(state(&app), GameState::Menu);
    assert_eq!(page(&mut app), Some(MenuPage::Servers));
    assert_eq!(status(&app), ConnectStatus::Idle);

    // Again; and the server stops.
    click(&mut app, "Server: Local");
    until(&mut app, Some(&mut server), joined);
    server
        .world_mut()
        .remove_resource::<Network>()
        .unwrap()
        .close();
    until(&mut app, None, |app| {
        page(app) == Some(MenuPage::ConnectError)
    });
    let why = texts::<ConnectErrorText>(&mut app).concat();
    assert_eq!(why, "The server stopped");
    assert!(matches!(status(&app), ConnectStatus::Lost { .. }));
    drop(server);

    // Another server where it was, with another key: refused, saying so,
    // and showing the key it has.
    let saved = || {
        ServerList::load(Some(&data))
            .unwrap()
            .fingerprint(&address.to_string())
    };
    let failed = |app: &mut App| matches!(status(app), ConnectStatus::Failed { .. });
    let mut impostor = serve(address);
    click(&mut app, "Try again");
    until(&mut app, Some(&mut impostor), failed);
    let ConnectStatus::Failed {
        new_key: Some(first_key),
        ..
    } = status(&app)
    else {
        panic!("{:?}", status(&app));
    };
    run_more(&mut app, "wait 2");
    let why = texts::<ConnectErrorText>(&mut app).concat();
    assert!(why.contains("isn't the one saved"), "{why}");
    let shown = texts::<NewKeyText>(&mut app).concat();
    assert!(shown.contains(&first_key[..4]), "{shown}");
    assert!(shown.contains("replaces the key saved"), "{shown}");

    // That server goes before the player trusts it: the retry fails, and
    // the old key stays saved.
    impostor
        .world_mut()
        .remove_resource::<Network>()
        .unwrap()
        .close();
    drop(impostor);
    app.insert_resource(ConnectSettings {
        timeout: Duration::from_millis(300),
    });
    click(&mut app, "Trust new key");
    until(&mut app, None, |app| {
        matches!(status(app), ConnectStatus::Failed { new_key: None, .. })
    });
    assert_eq!(saved(), remembered);

    // Another again: trusting the key shown before isn't trusting this one.
    let mut impostor = serve(address);
    app.insert_resource(ConnectSettings::default());
    app.world_mut().write_message(Connect {
        server: ServerTarget {
            name: "Local".into(),
            address: address.to_string(),
        },
        trust_key: Some(first_key.clone()),
    });
    until(
        &mut app,
        Some(&mut impostor),
        |app| matches!(&status(app), ConnectStatus::Failed { new_key: Some(key), .. } if *key != first_key),
    );
    assert_eq!(saved(), remembered);

    // Called off once connected, before the key is saved: nothing changes.
    let ConnectStatus::Failed {
        new_key: Some(second_key),
        ..
    } = status(&app)
    else {
        unreachable!()
    };
    let second = Fingerprint(parse_hex(&second_key).unwrap());
    let content = app.world().resource::<Content>();
    let called_off = connect_remembering_with(
        &address.to_string(),
        Library::new(content, Some(&data)),
        &LocalIdentity::load(Some(&data)),
        Some(&data),
        Some(second),
        ConnectOptions {
            cancelled: Some(Arc::new(AtomicBool::new(true))),
            ..default()
        },
    );
    assert!(called_off.is_err());
    assert_eq!(saved(), remembered);
    // (The screen follows the status a frame later.)
    run_more(&mut app, "wait 2");

    // Trusted: it joins, and its key replaces the old.
    click(&mut app, "Trust new key");
    until(&mut app, Some(&mut impostor), joined);
    assert_eq!(saved(), Some(second));
}
