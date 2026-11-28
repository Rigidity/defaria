//! Scripts on both sides: a mod's scripts load on the server and on a
//! client alike, the gameplay hooks run only on the server, a client's
//! scripts read their world but can't change it, and the two talk through
//! `net.send` and `on_message`.

use bevy::{ecs::system::RunSystemOnce, prelude::*};
use defaria_core::{
    content::{BlockState, Content, block, load::order},
    script::{ScriptRuntime, ScriptSettings, ScriptSources, Side},
    testing::test_mods,
    world::Voxels,
};
use defaria_server::script::ScriptIndex;

use crate::headless::{Multiplayer, test_multiplayer};

/// One script, loaded on both sides, that logs each hook it's called on
/// with the side it's on. Placed, the marker block tells every client to
/// look at the stone; a client answers what it read there and whether it
/// could change it.
const BOTH: &str = r#"
function on_placed(block)
    log.info(side, "on_placed")
    net.broadcast("probe", { x = 64, y = 60, z = 88 })
end

function on_join()
    log.info(side, "on_join")
    net.send("hello", { from = side })
end

function on_message(channel, value, player)
    log.info(side, "got", channel)
    if side == "server" then
        if channel == "hello" then
            log.info("hello from", value.from)
            net.send(player, "welcome", { id = player.id })
        elseif channel == "report" then
            log.info("report", value.read, value.data, value.wrote, value.error)
        end
    elseif channel == "probe" then
        local stone = world.block(value.x, value.y, value.z)
        local wrote, error = pcall(function()
            return stone:set("defaria:glass")
        end)
        net.send("report", {
            read = stone.id,
            data = stone.data == nil,
            wrote = wrote,
            error = tostring(error),
        })
    end
end
"#;

fn mods() -> Vec<defaria_core::content::load::ModFiles> {
    test_mods(&[
        (
            "blocks/marker.json5",
            r#"{ model: "defaria:cube", textures: { all: "defaria:machine_side" }, script: "both" }"#,
        ),
        ("scripts/both.luau", BOTH),
    ])
}

/// A server and a client with the test mod's content and scripts, each
/// side's runtime its own.
fn game() -> Multiplayer {
    let content = Content::from_mods(mods());
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    let runtime = |side| {
        let sources = ScriptSources::from_mods(&order(mods(), &mut Vec::new()));
        let runtime = ScriptRuntime::new(side, sources, ScriptSettings::untimed());
        assert!(
            runtime.load_errors().is_empty(),
            "{:?}",
            runtime.load_errors()
        );
        runtime
    };
    let mut game = test_multiplayer(1);
    let server = runtime(Side::Server);
    game.server
        .insert_resource(ScriptIndex::new(&content, &server))
        .insert_resource(server)
        .insert_resource(content.clone());
    game.clients[0]
        .insert_resource(runtime(Side::Client))
        .insert_resource(content);
    game
}

/// What one side's scripts logged, a line each.
fn logged(app: &App) -> Vec<String> {
    let runtime = app.world().resource::<ScriptRuntime>();
    runtime.log_lines().into_iter().map(|l| l.message).collect()
}

fn voxel(app: &mut App, pos: IVec3) -> Option<BlockState> {
    app.world_mut()
        .run_system_once(move |voxels: Voxels| voxels.get(pos))
        .unwrap()
}

#[test]
fn scripts_on_both_sides_talk_and_only_the_server_changes_the_world() {
    let mut game = game();
    game.settle();

    // Joining ran the client's `on_join`, whose message reached the
    // server's `on_message`, whose answer came back to that player.
    let client = logged(&game.clients[0]);
    assert!(client.contains(&"client on_join".into()), "{client:?}");
    assert!(client.contains(&"client got welcome".into()), "{client:?}");
    let server = logged(&game.server);
    assert!(server.contains(&"hello from client".into()), "{server:?}");
    assert!(!server.iter().any(|l| l.contains("on_join")), "{server:?}");

    // A stone, and the marker beside it, placed on the server.
    let stone = IVec3::new(64, 60, 88);
    let marker = IVec3::new(66, 60, 88);
    let state = game
        .server
        .world()
        .resource::<Content>()
        .default_state("test:marker")
        .unwrap();
    game.server
        .world_mut()
        .run_system_once(move |mut voxels: Voxels| {
            voxels.set(stone, block("stone"));
            voxels.set(marker, state);
        })
        .unwrap();
    for _ in 0..10 {
        game.update();
    }

    // The block hook ran on the server, never on the client.
    let server = logged(&game.server);
    assert!(server.contains(&"server on_placed".into()), "{server:?}");
    let client = logged(&game.clients[0]);
    assert!(
        !client.iter().any(|l| l.contains("on_placed")),
        "{client:?}"
    );
    assert_eq!(voxel(&mut game.clients[0], marker), Some(state));

    // The client read the stone, had no data for it, and was refused
    // changing it, with an error saying why; nothing changed on either
    // side.
    assert!(client.contains(&"client got probe".into()), "{client:?}");
    let report = server
        .iter()
        .find(|l| l.starts_with("report"))
        .unwrap_or_else(|| panic!("no report: {server:?}"));
    assert!(
        report.starts_with("report defaria:stone true false"),
        "{report}"
    );
    assert!(report.contains("read-only on a client"), "{report}");
    assert_eq!(voxel(&mut game.server, stone), Some(block("stone")));
    assert_eq!(voxel(&mut game.clients[0], stone), Some(block("stone")));
}

/// A client that sends more script messages than a client's scripts may
/// (one that isn't the game's) has the rest dropped by the server, and
/// one too big or on too long a channel is dropped whole.
#[test]
fn the_server_holds_a_client_to_its_message_budget() {
    use defaria_client::net::ToServer;
    use defaria_core::{
        block_entity::Value,
        net::{ClientMessage, ScriptMessage},
        script::channel::MESSAGE_BURST,
    };
    let mut game = game();
    game.settle();
    let message = |channel: &str, text: usize| {
        ToServer(ClientMessage::Script(ScriptMessage {
            mod_id: "test".into(),
            channel: channel.into(),
            value: Some(Value::Text("x".repeat(text))),
        }))
    };
    let client = game.clients[0].world_mut();
    client.write_message(message("too big", 5000));
    client.write_message(message(&"c".repeat(65), 1));
    for _ in 0..100 {
        client.write_message(message("flood", 1));
    }
    for _ in 0..3 {
        game.update();
    }
    let server = logged(&game.server);
    let count = |what: &str| server.iter().filter(|l| *l == what).count();
    assert_eq!(count("server got too big"), 0);
    assert!(!server.iter().any(|l| l.contains("ccc")), "{server:?}");
    // The two dropped for their size count against the budget too.
    assert_eq!(count("server got flood"), MESSAGE_BURST as usize - 2);
}
