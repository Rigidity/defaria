//! A server's side of its connections (see `net`): each client is a
//! player entity with a [`Connection`]. What a client sends becomes that
//! player's pose and intents, which gameplay acts on as for any player;
//! the server sends each client the chunks in its player's range and how
//! they change, the player's own state, and the screen of the machine it
//! uses.
//!
//! The server owns everything: the client's chunks are copies, and the
//! client only asks (to use, to break, to move items). Only movement is
//! the client's own for now; #214 makes the server check it.

use bevy::{platform::collections::HashSet, prelude::*};

use crate::{
    block_entity::BlockEntities,
    content::{BlockState, Content},
    game_mode::GameMode,
    hotbar::{ActiveSlot, Hotbar},
    inventory::{Inventory, transfer::transfer},
    light::ChunkLight,
    machine::{
        EnergyBuffer, MachineActive, Slots, Tanks,
        exchange::{into_machine_from, out_of_machine},
        has_screen,
    },
    mining::Mining,
    net::{
        ClientMessage, MachineContents, OtherPlayer, PlayerState, Pose, ServerLink, ServerMessage,
        Transfer,
    },
    physics::Body,
    player::{Look, Player, START_POSITION, START_TARGET, feet_below, player},
    recipe::RecipeProgress,
    streaming::{ChunkLoader, ChunkUnloaded, StreamingSettings, StreamingStatus, loader_chunk},
    use_action::{UseAction, UseStep},
    world::{Chunk, ChunkCoord, ChunkMap, Voxels},
};

pub struct ServerNetPlugin;

impl Plugin for ServerNetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Incoming>()
            .init_resource::<PauseWithPlayers>()
            .add_systems(
                PreUpdate,
                (
                    accept,
                    receive,
                    (apply_poses, apply_intents, apply_transfers, note_pauses),
                    close_screens,
                    pause_with_players,
                )
                    .chain()
                    .in_set(ReceiveSystems),
            )
            .add_systems(Update, open_machine_screens.in_set(UseStep::Screen))
            .add_systems(
                Last,
                (
                    send_chunks,
                    send_player_state,
                    send_mining,
                    send_players,
                    send_machine_contents,
                    send_status,
                )
                    .chain(),
            );
    }
}

/// Where what clients sent is applied, in `PreUpdate`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReceiveSystems;

/// Whether the game pauses while every player is paused: a server in the
/// game's own process, for single player, does; a dedicated one doesn't.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct PauseWithPlayers(pub bool);

/// New connections, which join as players on the next frame.
#[derive(Resource, Default)]
pub struct Incoming(pub Vec<ServerLink>);

/// A player's connection to its client, and what the client has been sent.
#[derive(Component)]
pub struct Connection {
    link: ServerLink,
    /// What arrived this frame, oldest first.
    inbox: Vec<ClientMessage>,
    /// The chunks the client has.
    sent: HashSet<IVec3>,
    /// The player's state as last sent.
    state: Option<PlayerState>,
    /// Its mining's target and progress as last sent.
    mining: Option<(Option<(IVec3, BlockState)>, f32)>,
    /// The machine whose screen the client has open, and what it was last
    /// sent of it.
    machine: Option<(IVec3, MachineContents)>,
    /// Whether the client was last told everything is loaded.
    settled: Option<bool>,
    /// The chunk the player's range was last worked out around.
    center: Option<IVec3>,
    /// Whether the player is paused.
    paused: bool,
    /// The other players as last sent.
    others: Option<Vec<OtherPlayer>>,
}

impl Connection {
    pub fn new(link: ServerLink) -> Self {
        Self {
            link,
            inbox: Vec::new(),
            sent: HashSet::new(),
            state: None,
            mining: None,
            machine: None,
            settled: None,
            center: None,
            paused: false,
            others: None,
        }
    }

    fn send(&mut self, message: ServerMessage) {
        self.link.send(message);
    }
}

/// Each new connection joins as a player where new players start (the
/// client puts it where it is with its first [`ClientMessage::Pose`]).
fn accept(mut commands: Commands, mut incoming: ResMut<Incoming>) {
    for link in incoming.0.drain(..) {
        let eyes = Transform::from_translation(START_POSITION).looking_at(START_TARGET, Vec3::Y);
        let feet = feet_below(START_POSITION.as_dvec3(), crate::player::EYE_HEIGHT);
        commands.spawn((
            Name::new("Player"),
            player(feet, Look::from_rotation(eyes.rotation)),
            Connection::new(link),
        ));
    }
}

/// Takes what each client sent; a client that's gone leaves.
fn receive(mut commands: Commands, mut connections: Query<(Entity, &mut Connection)>) {
    for (entity, mut connection) in &mut connections {
        let messages = connection.link.receive();
        if connection.link.is_closed() {
            info!("a player left");
            commands.entity(entity).despawn();
            continue;
        }
        connection.inbox = messages;
    }
}

/// The client moves its player: the last pose it sent is where it is.
fn apply_poses(
    mut players: Query<(
        &Connection,
        &mut Body,
        &mut Transform,
        &mut Look,
        &mut Player,
    )>,
) {
    for (connection, mut body, mut transform, mut look, mut player) in &mut players {
        let pose = connection
            .inbox
            .iter()
            .rev()
            .find_map(|message| match message {
                ClientMessage::Pose(pose) => Some(pose),
                _ => None,
            });
        let Some(Pose {
            feet,
            velocity,
            on_ground,
            look: new_look,
            player: new_player,
        }) = pose
        else {
            continue;
        };
        body.position = *feet;
        body.previous = *feet;
        body.velocity = *velocity;
        body.on_ground = *on_ground;
        transform.translation = feet.as_vec3();
        transform.rotation = new_look.yaw_rotation();
        look.set_if_neq(*new_look);
        player.set_if_neq(new_player.clone());
    }
}

/// What the player means to do: a use, holding the break button, its
/// hotbar and the slot in hand, the game mode.
fn apply_intents(
    mut players: Query<(
        &Connection,
        &mut UseAction,
        Option<&mut Mining>,
        Option<&mut Hotbar>,
        Option<&mut ActiveSlot>,
        Option<&mut GameMode>,
    )>,
) {
    for (connection, mut action, mut mining, mut hotbar, mut active, mut mode) in &mut players {
        for message in &connection.inbox {
            match message {
                ClientMessage::Use(using) => *action = UseAction(Some(*using)),
                ClientMessage::Mining(holding) => {
                    if let Some(mining) = &mut mining
                        && mining.holding != *holding
                    {
                        mining.holding = *holding;
                    }
                }
                ClientMessage::Hotbar(new) => {
                    if let Some(hotbar) = &mut hotbar {
                        hotbar.set_if_neq(new.clone());
                    }
                }
                ClientMessage::ActiveSlot(new) => {
                    if let Some(active) = &mut active {
                        active.set_if_neq(*new);
                    }
                }
                ClientMessage::ToggleGameMode => {
                    if let Some(mode) = &mut mode {
                        **mode = match **mode {
                            GameMode::Creative => GameMode::Survival,
                            GameMode::Survival => GameMode::Creative,
                        };
                        info!("game mode: {:?}", **mode);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Moves items as the player asks from a screen: to and from the creative
/// catalog, and between it and the machine whose screen it has open.
fn apply_transfers(
    content: Res<Content>,
    index: Res<BlockEntities>,
    voxels: Voxels,
    mut players: Query<(&Connection, &mut Inventory, Option<&GameMode>)>,
    mut machines: Query<&mut Slots>,
) {
    for (connection, mut inventory, mode) in &mut players {
        for message in &connection.inbox {
            let ClientMessage::Transfer(transfer_message) = message else {
                continue;
            };
            match transfer_message {
                Transfer::Catalog {
                    kind,
                    count,
                    to_player,
                } => {
                    // Only creative has a catalog.
                    if mode.is_some_and(|mode| !mode.is_creative()) {
                        continue;
                    }
                    if *to_player {
                        transfer(&content, None, Some(&mut inventory), kind, *count);
                    } else {
                        transfer(&content, Some(&mut inventory), None, kind, *count);
                    }
                }
                Transfer::IntoMachine { kind, count, first } => {
                    let Some((pos, _)) = connection.machine else {
                        continue;
                    };
                    let machine = voxels.get(pos).and_then(|state| content.machine(state));
                    let slots = index.get(pos).and_then(|e| machines.get_mut(e).ok());
                    if let (Some(machine), Some(mut slots)) = (machine, slots) {
                        into_machine_from(
                            &content,
                            &mut inventory,
                            &mut slots,
                            machine,
                            kind,
                            *count,
                            *first,
                        );
                    }
                }
                Transfer::OutOfMachine {
                    group,
                    index: slot,
                    count,
                } => {
                    let Some((pos, _)) = connection.machine else {
                        continue;
                    };
                    if let Some(mut slots) = index.get(pos).and_then(|e| machines.get_mut(e).ok()) {
                        out_of_machine(&content, &mut inventory, &mut slots, *group, *slot, *count);
                    }
                }
            }
        }
    }
}

/// Notes which players paused or stopped.
fn note_pauses(mut connections: Query<&mut Connection>) {
    for mut connection in &mut connections {
        let paused = connection
            .inbox
            .iter()
            .rev()
            .find_map(|message| match message {
                ClientMessage::Paused(paused) => Some(*paused),
                _ => None,
            });
        if let Some(paused) = paused {
            connection.paused = paused;
        }
    }
}

/// While every player is paused (and the server pauses with its players),
/// the game is: time stands still, so nothing moves or grows.
fn pause_with_players(
    pause: Res<PauseWithPlayers>,
    connections: Query<&Connection>,
    mut time: ResMut<Time<Virtual>>,
) {
    let paused = pause.0 && !connections.is_empty() && connections.iter().all(|c| c.paused);
    if paused != time.is_paused() {
        if paused {
            time.pause();
        } else {
            time.unpause();
        }
    }
}

/// A client that closed its machine screen isn't sent the machine any
/// more (unless it's since opened another, or the same one again).
fn close_screens(mut connections: Query<&mut Connection>) {
    for mut connection in &mut connections {
        let closed: Vec<IVec3> = (connection.inbox.iter())
            .filter_map(|message| match message {
                ClientMessage::CloseScreen(pos) => Some(*pos),
                _ => None,
            })
            .collect();
        if connection
            .machine
            .as_ref()
            .is_some_and(|(open, _)| closed.contains(open))
        {
            connection.machine = None;
        }
    }
}

/// Using a machine that has a screen (not sneaking) opens it on the
/// player's client, which is then sent what the machine holds.
fn open_machine_screens(
    content: Res<Content>,
    voxels: Voxels,
    index: Res<BlockEntities>,
    mut players: Query<(&mut UseAction, &mut Connection)>,
) {
    for (mut action, mut connection) in &mut players {
        let Some(using) = action.pending() else {
            continue;
        };
        if using.sneak {
            continue;
        }
        let Some(hit) = using.target else {
            continue;
        };
        let Some(state) = voxels.get(hit.block) else {
            continue;
        };
        if !content.machine(state).is_some_and(has_screen) || index.get(hit.block).is_none() {
            continue;
        }
        // Used: nothing is placed against it.
        action.claim();
        // Already open (a second click on its way before the first
        // opened it): nothing more to do.
        if connection
            .machine
            .as_ref()
            .is_some_and(|(open, _)| *open == hit.block)
        {
            continue;
        }
        connection.machine = Some((hit.block, MachineContents::default()));
        connection.send(ServerMessage::OpenScreen {
            pos: hit.block,
            state,
        });
    }
}

/// Sends each client the loaded chunks in its player's range it doesn't
/// have, those it has that changed (their blocks or fluid visibly, or
/// their light), and which ones it should drop. Only what changed is
/// looked at: chunks that loaded, unloaded or changed this frame, and the
/// whole range only when the player crosses into another chunk.
fn send_chunks(
    settings: Res<StreamingSettings>,
    map: Res<ChunkMap>,
    chunks: Query<(&Chunk, &ChunkLight)>,
    changed: Query<
        (&ChunkCoord, Ref<Chunk>, Ref<ChunkLight>),
        Or<(Changed<Chunk>, Changed<ChunkLight>)>,
    >,
    added: Query<&ChunkCoord, Added<Chunk>>,
    mut unloaded: MessageReader<ChunkUnloaded>,
    loaders: Query<(&GlobalTransform, &ChildOf), With<ChunkLoader>>,
    mut connections: Query<&mut Connection>,
) {
    let unloaded: Vec<IVec3> = unloaded.read().map(|unloaded| unloaded.0.0).collect();
    let added: Vec<IVec3> = added.iter().map(|coord| coord.0).collect();
    for (transform, parent) in &loaders {
        let Ok(mut connection) = connections.get_mut(parent.parent()) else {
            continue;
        };
        let connection = &mut *connection;
        let center = loader_chunk(transform);
        let moved = connection.center != Some(center);
        connection.center = Some(center);
        let send = |connection: &mut Connection, message| connection.link.send(message);
        // Gone, or (once it's moved) out of range past the margin: dropped.
        let mut dropped: Vec<IVec3> = unloaded
            .iter()
            .copied()
            .filter(|coord| connection.sent.contains(coord))
            .collect();
        if moved {
            dropped.extend(
                connection
                    .sent
                    .iter()
                    .copied()
                    .filter(|&coord| !settings.reaches(coord - center, settings.unload_margin)),
            );
        }
        for coord in dropped {
            if connection.sent.remove(&coord) {
                send(connection, ServerMessage::Unload(ChunkCoord(coord)));
            }
        }
        // New in range: the whole range when it's moved, else what loaded.
        let mut fresh = HashSet::new();
        let candidates: Vec<IVec3> = if moved {
            settings.window(center).collect()
        } else {
            added
                .iter()
                .copied()
                .filter(|&coord| settings.reaches(coord - center, 0))
                .collect()
        };
        for coord in candidates {
            if connection.sent.contains(&coord) {
                continue;
            }
            let Some((chunk, light)) = map
                .get(ChunkCoord(coord))
                .and_then(|entity| chunks.get(entity).ok())
            else {
                continue;
            };
            connection.sent.insert(coord);
            fresh.insert(coord);
            send(
                connection,
                ServerMessage::Chunk {
                    coord: ChunkCoord(coord),
                    chunk: chunk.clone(),
                    light: light.clone(),
                },
            );
        }
        // Changed since they were sent.
        for (coord, chunk, light) in &changed {
            if !connection.sent.contains(&coord.0) || fresh.contains(&coord.0) {
                continue;
            }
            if chunk.is_changed() {
                send(
                    connection,
                    ServerMessage::ChunkChanged {
                        coord: *coord,
                        chunk: (*chunk).clone(),
                    },
                );
            }
            if light.is_changed() {
                send(
                    connection,
                    ServerMessage::LightChanged {
                        coord: *coord,
                        light: (*light).clone(),
                    },
                );
            }
        }
    }
}

/// Sends each client its player's state when it changes.
fn send_player_state(
    mut players: Query<(
        &mut Connection,
        Ref<Inventory>,
        Ref<Hotbar>,
        Ref<ActiveSlot>,
        Ref<GameMode>,
    )>,
) {
    for (mut connection, inventory, hotbar, active, game_mode) in &mut players {
        let changed = inventory.is_changed()
            || hotbar.is_changed()
            || active.is_changed()
            || game_mode.is_changed();
        if !changed && connection.state.is_some() {
            continue;
        }
        let state = PlayerState {
            inventory: (*inventory).clone(),
            hotbar: (*hotbar).clone(),
            active: *active,
            game_mode: *game_mode,
        };
        if connection.state.as_ref() != Some(&state) {
            connection.state = Some(state.clone());
            connection.send(ServerMessage::PlayerState(state));
        }
    }
}

/// Sends each client the block its player is mining and how far along,
/// when that changes.
fn send_mining(mut players: Query<(&mut Connection, &Mining)>) {
    for (mut connection, mining) in &mut players {
        let now = (mining.target, mining.progress);
        if connection.mining != Some(now) {
            connection.mining = Some(now);
            connection.send(ServerMessage::Mining {
                target: now.0,
                progress: now.1,
            });
        }
    }
}

/// Sends each client where the other players are, when that changes.
fn send_players(
    players: Query<(Entity, &Body, &Look, &Player)>,
    mut connections: Query<(Entity, &mut Connection)>,
) {
    let all: Vec<(Entity, OtherPlayer)> = players
        .iter()
        .map(|(entity, body, look, player)| {
            let other = OtherPlayer {
                id: entity.to_bits(),
                feet: body.position,
                look: *look,
                sneaking: player.sneaking,
            };
            (entity, other)
        })
        .collect();
    for (me, mut connection) in &mut connections {
        let others: Vec<OtherPlayer> = all
            .iter()
            .filter(|(entity, _)| *entity != me)
            .map(|(_, other)| other.clone())
            .collect();
        if connection.others.as_ref() != Some(&others) {
            connection.others = Some(others.clone());
            connection.send(ServerMessage::Players(others));
        }
    }
}

/// Sends a client with a machine's screen open what the machine holds
/// when it changes, and closes the screen when the machine goes.
fn send_machine_contents(
    content: Res<Content>,
    voxels: Voxels,
    index: Res<BlockEntities>,
    parts: Query<(
        Option<&Slots>,
        Option<&Tanks>,
        Option<&EnergyBuffer>,
        Option<&RecipeProgress>,
        Option<&MachineActive>,
    )>,
    mut connections: Query<&mut Connection>,
) {
    for mut connection in &mut connections {
        let Some((pos, last)) = connection.machine.clone() else {
            continue;
        };
        let machine = voxels
            .get(pos)
            .is_some_and(|state| content.machine(state).is_some());
        let parts = index.get(pos).and_then(|entity| parts.get(entity).ok());
        let (true, Some((slots, tanks, energy, progress, active))) = (machine, parts) else {
            connection.machine = None;
            connection.send(ServerMessage::CloseScreen);
            continue;
        };
        let contents = MachineContents {
            slots: slots.cloned(),
            tanks: tanks.cloned(),
            energy: energy.cloned(),
            progress: progress.cloned(),
            active: active.copied(),
        };
        if contents != last {
            connection.machine = Some((pos, contents.clone()));
            connection.send(ServerMessage::MachineContents(contents));
        }
    }
}

/// Tells each client whether everything around its player is loaded,
/// when that changes. Whatever's loaded in its range has been sent by now
/// (see [`send_chunks`]).
fn send_status(status: Res<StreamingStatus>, mut connections: Query<&mut Connection>) {
    for mut connection in &mut connections {
        if connection.settled != Some(status.settled) {
            connection.settled = Some(status.settled);
            connection.send(ServerMessage::Settled(status.settled));
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::{ecs::system::RunSystemOnce, math::DVec3};

    use super::*;
    use crate::{
        block_entity::BlockEntityPlugin,
        content::{base, block},
        game_mode::GameModePlugin,
        hotbar::HotbarPlugin,
        inventory::{InventoryPlugin, player::STARTING_COUNT},
        mining::MiningPlugin,
        net::{ClientLink, link},
        player::EYE_HEIGHT,
        testing::world_app,
        use_action::UseActionPlugin,
    };

    /// A server with a client joined, and everything it was sent until it
    /// was told it's settled.
    fn server() -> (App, ClientLink, Vec<ServerMessage>) {
        let mut app = world_app(base().clone())
            .plugins((
                ServerNetPlugin,
                BlockEntityPlugin,
                InventoryPlugin,
                HotbarPlugin,
                GameModePlugin,
                MiningPlugin,
                UseActionPlugin,
            ))
            .build();
        let (mut client, server) = link();
        app.world_mut().resource_mut::<Incoming>().0.push(server);
        let sent = until(&mut app, &mut client, |message| {
            matches!(message, ServerMessage::Settled(true))
        });
        (app, client, sent)
    }

    /// Updates the app until the client is sent a message `done` says is
    /// the one, and returns everything sent up to it.
    fn until(
        app: &mut App,
        client: &mut ClientLink,
        done: impl Fn(&ServerMessage) -> bool,
    ) -> Vec<ServerMessage> {
        let mut sent = Vec::new();
        for _ in 0..3000 {
            app.update();
            crate::streaming::finish_loads(app.world_mut());
            for message in client.receive() {
                let last = done(&message);
                sent.push(message);
                if last {
                    return sent;
                }
            }
        }
        panic!("never sent; sent {} messages", sent.len());
    }

    fn the_player(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<Connection>>()
            .single(app.world())
            .unwrap()
    }

    /// A client that joins is a player, and is sent every chunk in its
    /// range (each once), what it carries, and then that it's settled.
    #[test]
    fn a_client_joins_and_is_sent_its_range() {
        let (mut app, _client, sent) = server();
        let player = the_player(&mut app);
        let center = ChunkCoord::split((START_POSITION).floor().as_ivec3()).0.0;
        let settings = app.world().resource::<StreamingSettings>().clone();
        let mut chunks = HashSet::new();
        for message in &sent {
            if let ServerMessage::Chunk { coord, .. } = message {
                assert!(chunks.insert(coord.0), "{coord:?} sent twice");
            }
        }
        let window: HashSet<IVec3> = settings.window(center).collect();
        assert_eq!(chunks, window);
        let state = sent.iter().find_map(|message| match message {
            ServerMessage::PlayerState(state) => Some(state),
            _ => None,
        });
        let state = state.expect("its state was sent");
        let stone = base().default_item("defaria:stone").unwrap();
        assert_eq!(state.inventory.count(stone), STARTING_COUNT);
        assert_eq!(state.game_mode, GameMode::Creative);
        assert!(app.world().get::<Player>(player).is_some());
    }

    /// The client moves its player: a pose puts it there, and the chunks
    /// around its new place are sent and those left behind dropped.
    #[test]
    fn a_pose_moves_the_player_and_its_range() {
        let (mut app, mut client, _) = server();
        let player = the_player(&mut app);
        let feet = DVec3::new(4064.5, 56.0 - EYE_HEIGHT, 92.5);
        client.send(ClientMessage::Pose(Pose {
            feet,
            velocity: DVec3::ZERO,
            on_ground: false,
            look: Look::default(),
            player: Player::default(),
        }));
        let sent = until(&mut app, &mut client, |message| {
            matches!(message, ServerMessage::Settled(true))
        });
        assert_eq!(app.world().get::<Body>(player).unwrap().position, feet);
        let far = ChunkCoord::split(IVec3::new(4064, 56, 92)).0;
        assert!(
            sent.iter().any(
                |message| matches!(message, ServerMessage::Chunk { coord, .. } if *coord == far)
            )
        );
        let home = ChunkCoord::split(START_POSITION.floor().as_ivec3()).0;
        assert!(sent.contains(&ServerMessage::Unload(home)));
    }

    /// An edit to a chunk the client has sends the chunk again.
    #[test]
    fn edits_in_range_are_sent() {
        let (mut app, mut client, _) = server();
        let pos = START_POSITION.floor().as_ivec3();
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| {
                voxels.set(pos, block("stone"));
            })
            .unwrap();
        // (The chunks next to it are sent again too, as they're redrawn.)
        let (expected, local) = ChunkCoord::split(pos);
        let sent = until(
            &mut app,
            &mut client,
            |message| matches!(message, ServerMessage::ChunkChanged { coord, .. } if *coord == expected),
        );
        let Some(ServerMessage::ChunkChanged { chunk, .. }) = sent.last() else {
            unreachable!();
        };
        assert_eq!(chunk.at(local), block("stone"));
    }

    /// Asking to switch the game mode switches it, and the client is sent
    /// its new state.
    #[test]
    fn the_game_mode_switches_when_asked() {
        let (mut app, mut client, _) = server();
        client.send(ClientMessage::ToggleGameMode);
        let sent = until(&mut app, &mut client, |message| {
            matches!(message, ServerMessage::PlayerState(_))
        });
        let Some(ServerMessage::PlayerState(state)) = sent.last() else {
            unreachable!();
        };
        assert_eq!(state.game_mode, GameMode::Survival);
    }

    /// A client that goes leaves: its player is gone.
    #[test]
    fn a_client_that_goes_leaves() {
        let (mut app, client, _) = server();
        drop(client);
        app.update();
        app.update();
        let mut players = app.world_mut().query::<&Player>();
        assert_eq!(players.iter(app.world()).count(), 0);
    }

    /// A server that pauses with its players stops time while every player
    /// is paused, and starts it again when one isn't; one that doesn't
    /// (a dedicated server) never stops.
    #[test]
    fn the_game_pauses_while_every_player_is() {
        let (mut app, mut client, _) = server();
        app.insert_resource(PauseWithPlayers(true));
        client.send(ClientMessage::Paused(true));
        app.update();
        assert!(app.world().resource::<Time<Virtual>>().is_paused());
        client.send(ClientMessage::Paused(false));
        app.update();
        assert!(!app.world().resource::<Time<Virtual>>().is_paused());

        app.insert_resource(PauseWithPlayers(false));
        client.send(ClientMessage::Paused(true));
        app.update();
        assert!(!app.world().resource::<Time<Virtual>>().is_paused());
    }
}
