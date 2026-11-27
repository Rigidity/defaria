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

use bevy::{
    platform::collections::{HashMap, HashSet},
    prelude::*,
};

use crate::{
    block_entity::BlockEntities,
    content::Content,
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
    net::{ClientMessage, MachineContents, PlayerState, Pose, ServerLink, ServerMessage, Transfer},
    physics::Body,
    player::{Look, Player, START_POSITION, START_TARGET, feet_below, player},
    recipe::RecipeProgress,
    streaming::{ChunkLoader, StreamingSettings, StreamingStatus, loader_chunk},
    use_action::{UseAction, UseStep},
    world::{Chunk, ChunkCoord, ChunkMap, Voxels},
};

pub struct ServerNetPlugin;

impl Plugin for ServerNetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Incoming>()
            .add_systems(
                PreUpdate,
                (
                    accept,
                    receive,
                    (apply_poses, apply_intents, apply_transfers),
                    close_screens,
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
    /// The machine whose screen the client has open, and what it was last
    /// sent of it.
    machine: Option<(IVec3, MachineContents)>,
    /// Whether the client was last told everything is loaded.
    settled: Option<bool>,
}

impl Connection {
    pub fn new(link: ServerLink) -> Self {
        Self {
            link,
            inbox: Vec::new(),
            sent: HashSet::new(),
            state: None,
            machine: None,
            settled: None,
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

/// A client that closed its machine screen isn't sent the machine any
/// more.
fn close_screens(mut connections: Query<&mut Connection>) {
    for mut connection in &mut connections {
        if connection
            .inbox
            .iter()
            .any(|message| matches!(message, ClientMessage::CloseScreen))
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
        connection.machine = Some((hit.block, MachineContents::default()));
        connection.send(ServerMessage::OpenScreen {
            pos: hit.block,
            state,
        });
    }
}

/// Sends each client the loaded chunks in its player's range it doesn't
/// have, those it has that changed (their blocks or fluid visibly, or
/// their light), and which ones it should drop.
fn send_chunks(
    settings: Res<StreamingSettings>,
    map: Res<ChunkMap>,
    chunks: Query<(&ChunkCoord, Ref<Chunk>, Ref<ChunkLight>)>,
    loaders: Query<(&GlobalTransform, &ChildOf), With<ChunkLoader>>,
    mut connections: Query<&mut Connection>,
) {
    let mut centers = HashMap::new();
    for (transform, parent) in &loaders {
        centers.insert(parent.parent(), loader_chunk(transform));
    }
    for (player, center) in centers {
        let Ok(mut connection) = connections.get_mut(player) else {
            continue;
        };
        let connection = &mut *connection;
        // Out of range (past the margin) or unloaded: dropped.
        let dropped: Vec<IVec3> = connection
            .sent
            .iter()
            .copied()
            .filter(|&coord| {
                map.get(ChunkCoord(coord)).is_none()
                    || !settings.reaches(coord - center, settings.unload_margin)
            })
            .collect();
        for coord in dropped {
            connection.sent.remove(&coord);
            connection
                .link
                .send(ServerMessage::Unload(ChunkCoord(coord)));
        }
        // Changed since they were sent.
        for &coord in &connection.sent {
            let Some(entity) = map.get(ChunkCoord(coord)) else {
                continue;
            };
            let Ok((_, chunk, light)) = chunks.get(entity) else {
                continue;
            };
            if chunk.is_changed() {
                connection.link.send(ServerMessage::ChunkChanged {
                    coord: ChunkCoord(coord),
                    chunk: (*chunk).clone(),
                });
            }
            if light.is_changed() {
                connection.link.send(ServerMessage::LightChanged {
                    coord: ChunkCoord(coord),
                    light: (*light).clone(),
                });
            }
        }
        // New in range.
        for coord in settings.window(center) {
            if connection.sent.contains(&coord) {
                continue;
            }
            let Some(entity) = map.get(ChunkCoord(coord)) else {
                continue;
            };
            let Ok((_, chunk, light)) = chunks.get(entity) else {
                continue;
            };
            connection.sent.insert(coord);
            connection.link.send(ServerMessage::Chunk {
                coord: ChunkCoord(coord),
                chunk: (*chunk).clone(),
                light: (*light).clone(),
            });
        }
    }
}

/// Sends each client its player's state when it changes.
fn send_player_state(
    mut players: Query<(
        &mut Connection,
        &Inventory,
        &Hotbar,
        &ActiveSlot,
        &GameMode,
        &Mining,
    )>,
) {
    for (mut connection, inventory, hotbar, active, game_mode, mining) in &mut players {
        let state = PlayerState {
            inventory: inventory.clone(),
            hotbar: hotbar.clone(),
            active: *active,
            game_mode: *game_mode,
            mining: *mining,
        };
        if connection.state.as_ref() != Some(&state) {
            connection.state = Some(state.clone());
            connection.send(ServerMessage::PlayerState(state));
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

/// Tells each client whether everything around its player is loaded and
/// sent, when that changes.
fn send_status(
    settings: Res<StreamingSettings>,
    status: Res<StreamingStatus>,
    map: Res<ChunkMap>,
    loaders: Query<(&GlobalTransform, &ChildOf), With<ChunkLoader>>,
    mut connections: Query<&mut Connection>,
) {
    for (transform, parent) in &loaders {
        let Ok(mut connection) = connections.get_mut(parent.parent()) else {
            continue;
        };
        let center = loader_chunk(transform);
        let settled = status.settled
            && settings.window(center).all(|coord| {
                map.get(ChunkCoord(coord)).is_none() || connection.sent.contains(&coord)
            });
        if connection.settled != Some(settled) {
            connection.settled = Some(settled);
            connection.send(ServerMessage::Settled(settled));
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
        let sent = until(&mut app, &mut client, |message| {
            matches!(message, ServerMessage::ChunkChanged { .. })
        });
        let Some(ServerMessage::ChunkChanged { coord, chunk }) = sent.last() else {
            unreachable!();
        };
        let (expected, local) = ChunkCoord::split(pos);
        assert_eq!(*coord, expected);
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
}
