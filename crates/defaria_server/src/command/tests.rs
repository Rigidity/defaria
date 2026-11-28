use super::*;
use crate::{block_entity::BlockEntityPlugin, chat::ChatPlugin, server::ServerNetPlugin};
use defaria_core::{
    content::{base, block},
    net::{ClientLink, link},
    world::Voxels,
};

/// A world with the server's connections, chat and commands, and a
/// player of id `[n; 32]` for each `n` in `ids`, with the links to their
/// clients.
fn server(ids: &[u8]) -> (App, Vec<(Entity, ClientLink)>) {
    let mut app = crate::testing::world_app(base().clone())
        .plugins((
            ServerNetPlugin,
            BlockEntityPlugin,
            ChatPlugin,
            CommandPlugin,
        ))
        .build();
    let players = ids
        .iter()
        .map(|&n| {
            let (client, server) = link();
            let entity = app
                .world_mut()
                .spawn((
                    Name::new(format!("P{n}")),
                    PlayerId([n; 32]),
                    Connection::new(server),
                ))
                .id();
            (entity, client)
        })
        .collect();
    (app, players)
}

fn stone_at(app: &mut App, pos: IVec3) -> bool {
    app.world_mut()
        .run_system_cached_with(|In(pos): In<IVec3>, voxels: Voxels| voxels.get(pos), pos)
        .unwrap()
        == Some(block("stone"))
}

#[test]
fn the_host_operators_and_console_may_run_commands_and_others_may_not() {
    let (mut app, players) = server(&[1, 2, 3]);
    app.insert_resource(Host(PlayerId([1; 32])))
        .insert_resource(Operators([PlayerId([2; 32])].into()));
    let fill = |y: i32| format!("/fill 0 {y} 0 0 {y} 0 stone");
    let world = app.world_mut();

    let host = run(world, Source::Player(players[0].0), &fill(40));
    assert!(host.ok, "{}", host.text);
    assert_eq!(host.filled.map(|(set, _)| set), Some(1));
    let op = run(world, Source::Player(players[1].0), &fill(41));
    assert!(op.ok, "{}", op.text);
    let console = run(world, Source::Console, &fill(42));
    assert!(console.ok, "{}", console.text);
    let other = run(world, Source::Player(players[2].0), &fill(43));
    assert!(!other.ok);
    assert!(other.text.contains("not allowed"), "{}", other.text);
    // But anyone may list who's playing, and ask for help.
    assert!(run(world, Source::Player(players[2].0), "list").ok);
    assert!(run(world, Source::Player(players[2].0), "help").ok);

    for y in 40..43 {
        assert!(stone_at(&mut app, IVec3::new(0, y, 0)), "{y}");
    }
    assert!(!stone_at(&mut app, IVec3::new(0, 43, 0)));
}

#[test]
fn mistakes_and_lines_too_long_are_refused_with_why() {
    let (mut app, _) = server(&[]);
    let world = app.world_mut();
    let unknown = run(world, Source::Console, "jump");
    assert!(!unknown.ok);
    assert!(unknown.text.contains("unknown command"), "{}", unknown.text);
    let bad = run(world, Source::Console, "fill 0 0 0 1 1 1 nonsense");
    assert!(!bad.ok);
    assert!(bad.text.contains("no block"), "{}", bad.text);
    let long = run(
        world,
        Source::Console,
        &format!("say {}", "x".repeat(MAX_MESSAGE)),
    );
    assert!(!long.ok);
    assert!(long.text.contains("too long"), "{}", long.text);
    let huge = run(world, Source::Console, "fill 0 0 0 1000 1000 1000 stone");
    assert!(!huge.ok);
    let nobody = run(world, Source::Console, "tp 0 0 0");
    assert!(!nobody.ok);
}

#[test]
fn save_asks_for_a_save_and_stop_for_an_exit() {
    let (mut app, _) = server(&[]);
    let world = app.world_mut();
    assert!(run(world, Source::Console, "save").ok);
    let save = *world.resource::<SaveNow>();
    assert!(save.chunks && save.players);
    assert!(run(world, Source::Console, "stop").ok);
    assert!(app.should_exit().is_some());
}

/// Players' commands sent over their links are answered on them, in
/// order; the console's go in its replies.
#[test]
fn commands_are_answered_to_whoever_sent_them() {
    let (mut app, mut players) = server(&[1, 2]);
    app.insert_resource(Host(PlayerId([1; 32])));
    let (_, client) = &mut players[0];
    client.send(ClientMessage::Command("list".into()));
    client.send(ClientMessage::Command("nonsense".into()));
    app.world_mut()
        .resource_mut::<Console>()
        .lines
        .push("list".into());
    app.update();
    let replies: Vec<CommandReply> = players[0]
        .1
        .receive()
        .into_iter()
        .filter_map(|message| match message {
            ServerMessage::CommandReply(reply) => Some(reply),
            _ => None,
        })
        .collect();
    assert_eq!(replies.len(), 2, "{replies:?}");
    assert!(replies[0].ok && replies[0].text.starts_with("2 players: P1 ("));
    assert!(!replies[1].ok);
    let console = &app.world().resource::<Console>().replies;
    assert_eq!(console.len(), 1);
    // Nothing for the other player but the chat they're sent.
    assert!(
        players[1]
            .1
            .receive()
            .iter()
            .all(|message| !matches!(message, ServerMessage::CommandReply(_)))
    );
}
