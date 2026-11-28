//! Chat and server commands through the whole game: the chat line,
//! messages between players, who may run commands, the limits on what a
//! player sends, and the console.

use bevy::{ecs::system::RunSystemOnce, prelude::*};
use defaria_client::{
    chat::{ChatLog, ChatScreen},
    net::ToServer,
    ui::typing,
};
use defaria_core::{
    chat::{CHAT_BURST, ChatKind, ChatLine, MAX_MESSAGE},
    content::{BlockState, block},
    net::ClientMessage,
    physics::Body,
    world::Voxels,
};
use defaria_server::command::{Console, Operators, ops};

use crate::headless::{
    Conditions, Multiplayer, Script, run_more, settle, test_game, test_multiplayer,
    test_multiplayer_over, test_player_id,
};

fn is_open(app: &mut App) -> bool {
    let mut screens = app.world_mut().query::<&ChatScreen>();
    screens.iter(app.world()).count() > 0
}

fn lines(app: &App) -> Vec<ChatLine> {
    let log = app.world().resource::<ChatLog>();
    log.lines.iter().map(|(line, _)| line.clone()).collect()
}

fn voxel(app: &mut App, pos: IVec3) -> Option<BlockState> {
    app.world_mut()
        .run_system_once(move |voxels: Voxels| voxels.get(pos))
        .unwrap()
}

/// A command, as a client sends it.
fn command(line: impl Into<String>) -> ClientMessage {
    ClientMessage::Command {
        id: 0,
        line: line.into(),
    }
}

/// Sends `message` from client `client` as its chat line would, and lets
/// a few frames go by for it to arrive and be answered.
fn send(game: &mut Multiplayer, client: usize, message: ClientMessage) {
    game.clients[client]
        .world_mut()
        .write_message(ToServer(message));
    for _ in 0..3 {
        game.update();
    }
}

/// T opens the chat line with the keyboard in it, so letters type rather
/// than open the inventory; Enter sends what's typed (here, a command the
/// single player host may run) and closes it; Escape closes it unsent.
#[test]
fn the_chat_line_opens_types_and_sends() {
    let mut app = test_game(Script::default());
    settle(&mut app);
    run_more(&mut app, "hold KeyT 1\nwait 2");
    assert!(is_open(&mut app));
    assert!(app.world_mut().run_system_cached(typing).unwrap());
    // The T that opened it isn't typed into it.
    assert_eq!(field_text(&mut app), Some(String::new()));

    let at = IVec3::new(64, 70, 80);
    run_more(
        &mut app,
        &format!(
            "type /fill {0} {1} {2} {0} {1} {2} stone\nwait 1\nhold Enter 1\nwait 3",
            at.x, at.y, at.z
        ),
    );
    assert!(!is_open(&mut app));
    assert_eq!(voxel(&mut app, at), Some(block("stone")));
    let said = lines(&app);
    assert_eq!(
        said.last().map(|line| line.text.as_str()),
        Some("filled 1 voxels")
    );
    // E typed into the field didn't open the inventory.
    let inventories = app
        .world_mut()
        .query::<&defaria_client::inventory_screen::screen::InventoryScreen>()
        .iter(app.world())
        .count();
    assert_eq!(inventories, 0);

    run_more(
        &mut app,
        "hold KeyT 1\nwait 1\ntype nothing\nwait 1\nhold Escape 1\nwait 2",
    );
    assert!(!is_open(&mut app));
    assert_eq!(lines(&app).len(), said.len(), "Escape sent nothing");
    // The Escape that closed it didn't open the pause menu too.
    assert!(!pause_menu_open(&mut app));

    // A scenario's own commands run, but their answers stay out of the
    // chat.
    run_more(&mut app, "fill 64 71 80 64 71 80 stone\nlist\nwait 2");
    assert_eq!(voxel(&mut app, at + IVec3::Y), Some(block("stone")));
    assert_eq!(lines(&app).len(), said.len());

    // A command typed in the chat line on the same frame as a scenario's:
    // each reply goes to its own sender. The chat's mistake is shown and
    // doesn't fail the scenario; the scenario's fill isn't shown.
    run_more(
        &mut app,
        "hold KeyT 1\nwait 1\ntype /fill 0 0 0 0 0 0 nonsense\nwait 1\n\
         hold Enter 1\nfill 64 72 80 64 72 80 stone\nwait 3",
    );
    assert!(
        !app.should_exit().is_some_and(|exit| exit.is_error()),
        "the scenario didn't fail"
    );
    assert_eq!(voxel(&mut app, at + IVec3::Y * 2), Some(block("stone")));
    let after = lines(&app);
    assert_eq!(after.len(), said.len() + 1);
    assert_eq!(after.last().unwrap().kind, ChatKind::Refused);
    assert!(after.last().unwrap().text.contains("nonsense"));
}

/// What the chat line's field holds, while it's open.
fn field_text(app: &mut App) -> Option<String> {
    let mut fields = app
        .world_mut()
        .query_filtered::<&defaria_client::ui::TextField, With<ChatScreen>>();
    fields
        .iter(app.world())
        .next()
        .map(|field| field.value.clone())
}

fn pause_menu_open(app: &mut App) -> bool {
    let mut menus = app.world_mut().query::<&defaria_client::pause::PauseMenu>();
    menus.iter(app.world()).count() > 0
}

/// The chat line doesn't open over another screen, or over the pause
/// menu.
#[test]
fn the_chat_line_waits_for_other_screens() {
    let mut app = test_game(Script::default());
    settle(&mut app);
    run_more(&mut app, "hold KeyE 1\nwait 2\nhold KeyT 1\nwait 2");
    assert!(!is_open(&mut app), "over the inventory");
    run_more(
        &mut app,
        "hold KeyE 1\nwait 2\nhold Escape 1\nwait 2\nhold KeyT 1\nwait 2",
    );
    assert!(!is_open(&mut app), "over the pause menu");
}

/// What one player says reaches the other, with their name; the server
/// announces who joins and leaves.
#[test]
fn chat_goes_between_two_clients() {
    let mut game = test_multiplayer(2);
    game.settle();
    game.run(
        0,
        "hold KeyT 1\ntype hello there\nwait 1\nhold Enter 1\nwait 3",
    );
    let said = ChatLine {
        kind: ChatKind::Said,
        from: Some("Player 1".into()),
        text: "hello there".into(),
    };
    for client in &game.clients {
        assert!(lines(client).contains(&said), "{:?}", lines(client));
    }
    // Both joined at once, so only the second was told the first joined
    // (and the first the second).
    assert!(
        lines(&game.clients[0])
            .iter()
            .any(|line| line.kind == ChatKind::Notice && line.text == "Player 2 joined")
    );

    // The second leaves: the first is told.
    game.disconnect(1);
    for _ in 0..5 {
        game.update();
    }
    assert!(
        lines(&game.clients[0])
            .iter()
            .any(|line| line.text == "Player 2 left")
    );
}

/// On a dedicated server nobody may run commands until listed: the
/// command is refused, with why, and changes nothing. An operator's run,
/// and anyone may `list`.
#[test]
fn commands_are_refused_for_players_not_allowed() {
    let mut game = test_multiplayer(2);
    game.server
        .insert_resource(Operators([test_player_id(0)].into()));
    game.settle();
    let at = IVec3::new(64, 70, 80);
    let fill = format!("fill {0} {1} {2} {0} {1} {2} stone", at.x, at.y, at.z);

    send(&mut game, 1, command(fill.clone()));
    assert_eq!(voxel(&mut game.server, at), Some(BlockState::AIR));
    let refused = lines(&game.clients[1]);
    let last = refused.last().unwrap();
    assert_eq!(last.kind, ChatKind::Refused);
    assert!(last.text.contains("not allowed"), "{}", last.text);

    // The operator may, through a scenario's command too.
    game.run(0, &format!("{fill}\nwait 1"));
    assert_eq!(voxel(&mut game.server, at), Some(block("stone")));

    send(
        &mut game,
        1,
        ClientMessage::Command {
            id: 0,
            line: "list".into(),
        },
    );
    let listed = lines(&game.clients[1]).last().unwrap().clone();
    assert_eq!(listed.kind, ChatKind::Output);
    assert!(
        listed.text.starts_with("2 players: Player 1, Player 2 ("),
        "{}",
        listed.text
    );
    assert!(listed.text.contains(&test_player_id(1).to_string()));
    assert!(!listed.text.contains(&test_player_id(0).to_string()));
}

/// An operator's `give`, `hotbar` and `tp` act on them, and the client
/// sees it: what it carries, what's in hand, where it is.
#[test]
fn allowed_commands_act_on_whoever_ran_them() {
    let mut game = test_multiplayer(1);
    game.server
        .insert_resource(Operators([test_player_id(0)].into()));
    game.settle();
    game.run(
        0,
        "give bucket 2 water 1\nhotbar 4 wrench\ntp 70.5 80 90.5\nwait 5",
    );
    let client = &mut game.clients[0];
    let (inventory, hotbar, active, body) = client
        .world_mut()
        .query_filtered::<(
            &defaria_core::inventory::Inventory,
            &defaria_core::hotbar::Hotbar,
            &defaria_core::hotbar::ActiveSlot,
            &Body,
        ), With<defaria_core::player::LocalPlayer>>()
        .single(client.world())
        .unwrap();
    let content = defaria_core::content::base();
    let bucket = content.default_item("defaria:bucket").unwrap();
    assert!(
        inventory
            .entries()
            .iter()
            .any(|stack| stack.item == bucket && stack.count == 2 && stack.data.fluid.is_some()),
        "{inventory:?}"
    );
    assert_eq!(active.0, 3);
    assert_eq!(hotbar.item(*active), content.default_item("defaria:wrench"));
    let eyes = body.position + bevy::math::DVec3::Y * defaria_core::player::EYE_HEIGHT;
    assert!(
        eyes.distance(bevy::math::DVec3::new(70.5, 80.0, 90.5)) < 0.01,
        "{eyes}"
    );
}

/// Over a slow link, a command typed in the chat line and then one a
/// scenario sends are both in flight at once: each reply goes to its own
/// sender, by id. The chat's refusal shows in the chat and doesn't fail
/// the scenario; the scenario's fill runs and stays out of the chat.
#[test]
fn replies_go_to_whichever_sent_the_command() {
    let mut game = test_multiplayer_over(
        1,
        Conditions {
            delay: 3,
            loss: 0.0,
        },
    );
    game.server
        .insert_resource(Operators([test_player_id(0)].into()));
    game.settle();
    let before = lines(&game.clients[0]).len();
    game.run(
        0,
        "hold KeyT 1\ntype /fill 0 0 0 0 0 0 nonsense\nwait 1\nhold Enter 1\n\
         fill 64 70 80 64 70 80 stone\nwait 1",
    );
    let exit = game.clients[0].should_exit();
    assert!(!exit.as_ref().is_some_and(AppExit::is_error), "{exit:?}");
    assert_eq!(
        voxel(&mut game.server, IVec3::new(64, 70, 80)),
        Some(block("stone"))
    );
    let after = lines(&game.clients[0]);
    assert_eq!(after.len(), before + 1, "{after:?}");
    assert_eq!(after.last().unwrap().kind, ChatKind::Refused);
}

/// What a player sends is held to a rate and a length, and cleaned of
/// control characters, on the server: whatever the client does.
#[test]
fn chat_is_limited_and_cleaned_on_the_server() {
    let mut game = test_multiplayer(2);
    game.settle();
    let before = lines(&game.clients[1]).len();
    for n in 0..12 {
        game.clients[0]
            .world_mut()
            .write_message(ToServer(ClientMessage::Chat(format!("spam {n}"))));
    }
    for _ in 0..3 {
        game.update();
    }
    let heard = lines(&game.clients[1]).len() - before;
    assert_eq!(heard, CHAT_BURST as usize);
    let told = lines(&game.clients[0]);
    let slow = told
        .iter()
        .filter(|line| line.kind == ChatKind::Refused)
        .count();
    assert_eq!(slow, 12 - CHAT_BURST as usize);

    // A wait refills it; too long is refused; control and format
    // characters go.
    for _ in 0..120 {
        game.update();
    }
    send(
        &mut game,
        0,
        ClientMessage::Chat("x".repeat(MAX_MESSAGE + 1)),
    );
    let last = lines(&game.clients[0]).last().unwrap().clone();
    assert_eq!(last.kind, ChatKind::Refused);
    assert!(last.text.contains("too long"), "{}", last.text);
    send(
        &mut game,
        0,
        ClientMessage::Chat("hi\u{1b}[2J\u{202E}!".into()),
    );
    assert_eq!(lines(&game.clients[1]).last().unwrap().text, "hi[2J!");

    // Commands from a player not allowed count against the rate too.
    for _ in 0..120 {
        game.update();
    }
    for _ in 0..12 {
        game.clients[1]
            .world_mut()
            .write_message(ToServer(command("fill 0 0 0 0 0 0 stone")));
    }
    for _ in 0..3 {
        game.update();
    }
    let told = lines(&game.clients[1]);
    let fast = told
        .iter()
        .filter(|line| line.text.contains("too fast"))
        .count();
    assert_eq!(fast, 12 - CHAT_BURST as usize);
}

/// The console runs any command, with no player: `say` talks to
/// everyone, `op` lets a player run commands (and lists them in
/// `ops.json5`), `give` has nobody to give to, and `stop` stops the
/// server.
#[test]
fn the_console_runs_commands() {
    let folder = defaria_core::testing::temp_folder("console-commands");
    let mut game = test_multiplayer(1);
    // Where `op` keeps its list.
    game.server
        .insert_resource(defaria_core::save::WorldFolder(folder.clone()));
    game.settle();
    let console = |game: &mut Multiplayer, line: &str| {
        let server = game.server.world_mut();
        server.resource_mut::<Console>().lines.push(line.into());
        for _ in 0..3 {
            game.update();
        }
        let replies =
            std::mem::take(&mut game.server.world_mut().resource_mut::<Console>().replies);
        assert_eq!(replies.len(), 1, "{replies:?}");
        replies.into_iter().next().unwrap()
    };

    let said = console(&mut game, "/say the server restarts soon");
    assert!(said.ok);
    let heard = lines(&game.clients[0]).last().unwrap().clone();
    assert_eq!(heard.from.as_deref(), Some("Server"));
    assert_eq!(heard.text, "the server restarts soon");

    let listed = console(&mut game, "list");
    assert_eq!(
        listed.text,
        format!("1 player: Player 1 ({})", test_player_id(0))
    );

    let gave = console(&mut game, "give stone 1");
    assert!(!gave.ok);
    assert!(gave.text.contains("no player"), "{}", gave.text);

    let at = IVec3::new(64, 70, 80);
    send(
        &mut game,
        0,
        command(format!(
            "fill {0} {1} {2} {0} {1} {2} stone",
            at.x, at.y, at.z
        )),
    );
    assert_eq!(voxel(&mut game.server, at), Some(BlockState::AIR));
    let opped = console(&mut game, "op player 1");
    assert!(opped.ok, "{}", opped.text);
    assert_eq!(
        ops::read(&folder).unwrap(),
        Operators([test_player_id(0)].into())
    );
    send(
        &mut game,
        0,
        command(format!(
            "fill {0} {1} {2} {0} {1} {2} stone",
            at.x, at.y, at.z
        )),
    );
    assert_eq!(voxel(&mut game.server, at), Some(block("stone")));
    let deopped = console(&mut game, &format!("deop {}", test_player_id(0)));
    assert!(deopped.ok, "{}", deopped.text);
    assert_eq!(ops::read(&folder).unwrap(), Operators::default());

    // An edit to the file counts without a restart.
    ops::write(&folder, &Operators([test_player_id(0)].into())).unwrap();
    send(
        &mut game,
        0,
        command(format!(
            "fill {0} {1} {2} {0} {1} {2} air",
            at.x, at.y, at.z
        )),
    );
    assert_eq!(voxel(&mut game.server, at), Some(BlockState::AIR));

    let unknown = console(&mut game, "jump");
    assert!(!unknown.ok);
    game.server
        .world_mut()
        .resource_mut::<Console>()
        .lines
        .push("stop".into());
    game.server.update();
    assert_eq!(game.server.should_exit(), Some(AppExit::Success));
}
