//! A server's side of its connections (see `net`): each client is a
//! player entity with a [`Connection`]. What a client sends becomes that
//! player's pose and intents, which gameplay acts on as for any player;
//! the server sends each client the chunks in its player's range and how
//! they change, the player's own state, and the screen of the machine it
//! uses.
//!
//! The server owns everything: the client's chunks are copies, and the
//! client only asks (to use, to break, to move items). Even its player's
//! movement: the client predicts it, and the server steps it from the
//! client's inputs ([`movement`]).

mod movement;
mod replication;

pub use movement::PlayerInputs;

use bevy::{platform::collections::HashSet, prelude::*};

use crate::{
    block_entity::BlockEntities,
    streaming::{ChunkLoader, ChunkUnloaded, loader_chunk},
};
use defaria_core::{
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
    net::{ClientMessage, MachineContents, PlayerState, ServerLink, ServerMessage, Transfer},
    player::{Look, MovedByInputs, START_POSITION, START_TARGET, feet_below, player},
    recipe::RecipeProgress,
    replication::{Kind, Replicated},
    streaming::{StreamingSettings, StreamingStatus},
    use_action::{Use, UseAction, UseStep},
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
                    (
                        (movement::take_inputs, movement::move_by_inputs).chain(),
                        apply_intents,
                        apply_transfers,
                        note_pauses,
                        take_uses.after(movement::move_by_inputs),
                    ),
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
                    answer_edits,
                    movement::send_moves,
                    send_player_state,
                    send_mining,
                    replication::send_entities,
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
    /// The uses the client sent that haven't been acted on yet, oldest
    /// first: their use, the input they came after, their edit, and how
    /// long they've waited.
    uses: std::collections::VecDeque<(Use, u32, Option<u32>, f32)>,
    /// The edits the client predicted with uses acted on this frame, to
    /// answer.
    edits: Vec<u32>,
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
            uses: default(),
            edits: Vec::new(),
        }
    }

    /// Sends the client a message.
    pub fn send(&mut self, message: ServerMessage) {
        self.link.send(message);
    }

    /// What the client sent this frame, oldest first.
    pub fn inbox(&self) -> &[ClientMessage] {
        &self.inbox
    }
}

/// Each new connection joins as a player where new players start, moved
/// by its client's inputs.
fn accept(mut commands: Commands, mut incoming: ResMut<Incoming>) {
    for link in incoming.0.drain(..) {
        let eyes = Transform::from_translation(START_POSITION).looking_at(START_TARGET, Vec3::Y);
        let feet = feet_below(START_POSITION.as_dvec3(), defaria_core::player::EYE_HEIGHT);
        commands.spawn((
            Name::new("Player"),
            player(feet, Look::from_rotation(eyes.rotation)),
            MovedByInputs,
            PlayerInputs::default(),
            Replicated(Kind::Player),
            replication::Interest::default(),
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

/// What the player means to do: holding the break button, its hotbar and
/// the slot in hand, the game mode. (Its uses are [`take_uses`]'.)
fn apply_intents(
    mut players: Query<(
        &Connection,
        Option<&mut Mining>,
        Option<&mut Hotbar>,
        Option<&mut ActiveSlot>,
        Option<&mut GameMode>,
    )>,
) {
    for (connection, mut mining, mut hotbar, mut active, mut mode) in &mut players {
        for message in &connection.inbox {
            match message {
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

/// The longest a use waits for the input it came after, should that
/// never come, before it's acted on anyway.
const MAX_USE_WAIT: f32 = 2.0;

/// Queues the uses each client sent, and acts on the oldest once the
/// player has been stepped through the input it came after: one a frame,
/// as `use_action` takes them, each where the client made it.
fn take_uses(
    time: Res<Time>,
    mut players: Query<(&mut Connection, &mut UseAction, Option<&PlayerInputs>)>,
) {
    for (mut connection, mut action, inputs) in &mut players {
        let connection = &mut *connection;
        for message in &connection.inbox {
            if let ClientMessage::Use { using, tick, edit } = message {
                connection.uses.push_back((*using, *tick, *edit, 0.0));
            }
        }
        for (.., waited) in &mut connection.uses {
            *waited += time.delta_secs();
        }
        let applied = inputs.map_or(u32::MAX, PlayerInputs::applied);
        let ready = connection
            .uses
            .front()
            .is_some_and(|(_, tick, _, waited)| *tick <= applied || *waited > MAX_USE_WAIT);
        if !ready {
            continue;
        }
        let (using, _, edit, _) = connection.uses.pop_front().unwrap();
        *action = UseAction(Some(using));
        connection.edits.extend(edit);
    }
}

/// Answers the edits whose uses were acted on this frame, after the chunks
/// they changed were sent.
fn answer_edits(mut connections: Query<&mut Connection>) {
    for mut connection in &mut connections {
        if let Some(last) = connection.edits.drain(..).max() {
            connection.send(ServerMessage::EditDone(last));
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
        block_entity::BlockEntityPlugin, game_mode::GameModePlugin, inventory::InventoryPlugin,
        mining::MiningPlugin, testing::world_app,
    };
    use defaria_core::{
        content::{base, block},
        hotbar::HotbarPlugin,
        inventory::player::STARTING_COUNT,
        net::{ClientLink, Input, link},
        physics::Body,
        player::{Controls, EYE_HEIGHT, Motion, Player},
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

    /// An input of a step `tick`, holding `controls`, putting the player
    /// at `teleport` first if given.
    fn input(tick: u32, controls: Controls, teleport: Option<Motion>) -> Input {
        Input {
            tick,
            controls,
            look: Look::default(),
            teleport,
        }
    }

    /// A player put somewhere by its client (a scenario's `camera`) is put
    /// there on the server too, and the chunks around its new place are
    /// sent and those left behind dropped.
    #[test]
    fn a_teleport_moves_the_player_and_its_range() {
        let (mut app, mut client, _) = server();
        let player = the_player(&mut app);
        let feet = DVec3::new(4064.5, 56.0 - EYE_HEIGHT, 92.5);
        let motion = Motion {
            feet,
            velocity: DVec3::ZERO,
            on_ground: false,
            flying: true,
            sneaking: false,
            sprinting: false,
            eye_height: EYE_HEIGHT,
        };
        client.send(ClientMessage::Inputs(vec![input(
            1,
            Controls::default(),
            Some(motion),
        )]));
        let sent = until(&mut app, &mut client, |message| {
            matches!(message, ServerMessage::Settled(true))
        });
        assert_eq!(app.world().get::<Body>(player).unwrap().position, feet);
        assert!(sent.iter().any(|message| matches!(
            message,
            ServerMessage::Moved { tick: 1, motion } if motion.feet == feet
        )));
        let far = ChunkCoord::split(IVec3::new(4064, 56, 92)).0;
        assert!(
            sent.iter().any(
                |message| matches!(message, ServerMessage::Chunk { coord, .. } if *coord == far)
            )
        );
        let home = ChunkCoord::split(START_POSITION.floor().as_ivec3()).0;
        assert!(sent.contains(&ServerMessage::Unload(home)));
    }

    /// Each input is a step of the player, applied once however often it's
    /// sent, and the client is told the last applied and where it left the
    /// player.
    #[test]
    fn inputs_step_the_player_once_each() {
        let (mut app, mut client, _) = server();
        let player = the_player(&mut app);
        let start = app.world().get::<Body>(player).unwrap().position;
        let forward = Controls {
            forward: 1.0,
            ..default()
        };
        let mut moves = Vec::new();
        for tick in 1..=10 {
            // Every input not yet confirmed, as clients send them.
            let unconfirmed = (tick.max(3) - 2..=tick)
                .map(|tick| input(tick, forward, None))
                .collect();
            client.send(ClientMessage::Inputs(unconfirmed));
            app.update();
            moves.extend(
                client
                    .receive()
                    .into_iter()
                    .filter_map(|message| match message {
                        ServerMessage::Moved { tick, motion } => Some((tick, motion)),
                        _ => None,
                    }),
            );
        }
        assert_eq!(moves.last().unwrap().0, 10);
        let applied = app.world().get::<PlayerInputs>(player).unwrap().applied();
        assert_eq!(applied, 10);
        let body = app.world().get::<Body>(player).unwrap();
        assert_ne!(body.position, start);
        assert_eq!(moves.last().unwrap().1.feet, body.position);
    }

    /// A client can't step its player faster than time passes: a flood of
    /// inputs is taken a frame's worth (and a burst) at a time.
    #[test]
    fn inputs_are_held_to_the_time_passing() {
        let (mut app, mut client, _) = server();
        let player = the_player(&mut app);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            defaria_core::physics::STEP,
        ));
        app.update();
        let flood = (1..=600)
            .map(|tick| input(tick, Controls::default(), None))
            .collect();
        client.send(ClientMessage::Inputs(flood));
        app.update();
        let applied = app.world().get::<PlayerInputs>(player).unwrap().applied();
        assert!((1..=21).contains(&applied), "{applied} steps at once");
        for _ in 0..10 {
            app.update();
        }
        let later = app.world().get::<PlayerInputs>(player).unwrap().applied();
        // A step a frame, a frame a step.
        assert!((applied + 9..=applied + 11).contains(&later), "{later}");
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

    /// Uses that arrive together are acted on one a frame, each once the
    /// player has been stepped through the input it came after, and only
    /// then is its edit answered.
    #[test]
    fn uses_wait_their_turn_and_their_input() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_systems(Update, take_uses);
        let (_client, server) = link();
        let using = |sneak| Use {
            eyes: Transform::default(),
            target: None,
            sneak,
        };
        let mut connection = Connection::new(server);
        connection.inbox = vec![
            ClientMessage::Use {
                using: using(false),
                tick: 0,
                edit: Some(1),
            },
            ClientMessage::Use {
                using: using(true),
                tick: 5,
                edit: Some(2),
            },
        ];
        let player = app
            .world_mut()
            .spawn((connection, UseAction::default(), PlayerInputs::default()))
            .id();
        let frame = |app: &mut App| {
            app.update();
            let world = app.world_mut();
            let action = world.get_mut::<UseAction>(player).unwrap().0.take();
            let mut connection = world.get_mut::<Connection>(player).unwrap();
            connection.inbox.clear();
            let edits = std::mem::take(&mut connection.edits);
            (action.map(|using| using.sneak), edits)
        };
        assert_eq!(frame(&mut app), (Some(false), vec![1]));
        // The second came after input 5, which hasn't been stepped.
        assert_eq!(frame(&mut app), (None, vec![]));
        app.world_mut()
            .get_mut::<PlayerInputs>(player)
            .unwrap()
            .set_applied(5);
        assert_eq!(frame(&mut app), (Some(true), vec![2]));
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
