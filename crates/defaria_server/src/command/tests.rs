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
    client.send(ClientMessage::Command {
        id: 0,
        line: "list".into(),
    });
    client.send(ClientMessage::Command {
        id: 0,
        line: "nonsense".into(),
    });
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

/// Operators are trusted to play, not to crash the server: every
/// argument that could overflow, run out of memory or stall is refused
/// with why.
#[test]
fn arguments_too_big_are_refused_not_crashed_on() {
    let (mut app, _) = server(&[]);
    app.init_resource::<RandomTicks>()
        .init_resource::<EvaporationSpeed>();
    let world = app.world_mut();
    for line in [
        "fill -2000000000 0 0 2000000000 0 0 stone",
        "fill -2147483648 -2147483648 -2147483648 2147483647 2147483647 2147483647 stone",
        "fluids -2147483648 -2147483648 -2147483648 2147483647 2147483647 2147483647",
        "tp 1e30 1e30 1e30",
        "tp 0 -20000000 0",
        "random-ticks 4294967295",
        "evaporation 1e30",
    ] {
        let reply = run(world, Source::Console, line);
        assert!(!reply.ok, "{line}: {}", reply.text);
        assert!(!reply.text.is_empty(), "{line}");
    }
    // The systems behind them check too, whatever calls them.
    let filled = world
        .run_system_cached_with(
            scene::fill,
            (
                IVec3::splat(i32::MIN),
                IVec3::splat(i32::MAX),
                "stone".to_string(),
            ),
        )
        .unwrap();
    assert!(filled.is_err());
    let report = report::fluid_report(
        world,
        report::FluidRegion::Box {
            min: IVec3::splat(i32::MIN),
            max: IVec3::splat(i32::MAX),
        },
    );
    assert!(report.contains("more than"), "{report}");
    // As big as they may be, they run.
    assert!(run(world, Source::Console, "random-ticks 4096").ok);
    assert!(run(world, Source::Console, "evaporation 1000").ok);
}

/// `ops.json5` is read again before each command that needs an
/// operator: one taken out by hand can't any more, a file that turns
/// unreadable allows nobody, and `op` keeps what was changed by hand and
/// won't write over a file it can't read.
#[test]
fn the_operators_file_is_read_before_each_command() {
    let folder = defaria_core::testing::temp_folder("ops-reread");
    let (mut app, players) = server(&[1, 2]);
    app.insert_resource(WorldFolder(folder.clone()));
    let (one, two) = (PlayerId([1; 32]), PlayerId([2; 32]));
    let world = app.world_mut();
    let fill = "fill 0 40 0 0 40 0 stone";

    ops::write(&folder, &Operators([one, two].into())).unwrap();
    assert!(run(world, Source::Player(players[0].0), fill).ok);
    // Taken out by hand: refused at once.
    ops::write(&folder, &Operators([two].into())).unwrap();
    assert!(!run(world, Source::Player(players[0].0), fill).ok);
    assert!(run(world, Source::Player(players[1].0), fill).ok);

    // `op` starts from what the file says now, not what was read before.
    ops::write(&folder, &Operators::default()).unwrap();
    assert!(run(world, Source::Console, "op P1").ok);
    assert_eq!(ops::read(&folder).unwrap(), Operators([one].into()));

    // A broken file allows nobody, and isn't written over.
    let path = folder.join(OPS_FILE);
    std::fs::write(&path, "{ broken").unwrap();
    assert!(!run(world, Source::Player(players[0].0), fill).ok);
    assert!(world.resource::<Operators>().0.is_empty());
    let op = run(world, Source::Console, "op P2");
    assert!(!op.ok);
    assert!(op.text.contains("fix or remove"), "{}", op.text);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ broken");
}

/// `op` takes the player called exactly that, or else the one called
/// that in any case; two of those is refused, and their ids tell them
/// apart.
#[test]
fn op_by_a_shared_name_is_refused() {
    let (mut app, players) = server(&[1, 2]);
    let world = app.world_mut();
    world.entity_mut(players[1].0).insert(Name::new("p1"));
    assert!(run(world, Source::Console, "op p1").ok);
    assert_eq!(world.resource::<Operators>().0, [PlayerId([2; 32])].into());
    world.entity_mut(players[0].0).insert(Name::new("Ada"));
    world.entity_mut(players[1].0).insert(Name::new("ADA"));
    let ambiguous = run(world, Source::Console, "op ada");
    assert!(!ambiguous.ok);
    assert!(
        ambiguous.text.contains("more than one"),
        "{}",
        ambiguous.text
    );
    assert!(run(world, Source::Console, "op Ada").ok);
}
