//! The client's side of its connection to a server (see
//! `defaria_core::net`).
//!
//! The server sends the chunks around the player, and the client keeps
//! copies of them to draw, collide with and aim at; it marks the ones whose
//! neighbours haven't arrived [`AwaitingNeighbours`], so they aren't drawn
//! yet. It also sends the player's own state (inventory, game mode,
//! mining), which the client puts on the local player.
//!
//! The client sends where the local player is, each frame it moves, and
//! what it does: a use (right click), holding the break button, and its
//! hotbar. Features send anything else (moving items, the game mode) as
//! [`ToServer`] messages, and read what they need of the server's as
//! [`FromServer`] ones.

use bevy::{platform::collections::HashSet, prelude::*};

use crate::session::GameState;
use defaria_core::{
    hotbar::{ActiveSlot, Hotbar},
    mining::Mining,
    net::{ClientLink, ClientMessage, PlayerState, Pose, ServerMessage},
    physics::Body,
    player::{LocalPlayer, Look, Player},
    streaming::{StreamingSettings, StreamingStatus},
    use_action::UseAction,
    world::{AwaitingNeighbours, ChunkCoord, ChunkMap, spawn_chunk},
};

pub struct ClientNetPlugin;

impl Plugin for ClientNetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StreamingStatus>()
            .init_resource::<StreamingSettings>()
            .init_resource::<Joined>()
            .init_resource::<ServerSettled>()
            .add_message::<ToServer>()
            .add_message::<FromServer>()
            .add_observer(give_the_player_mining)
            .add_systems(OnExit(GameState::Playing), forget_the_server)
            .add_systems(
                PreUpdate,
                (receive, update_awaiting).chain().in_set(ReceiveSystems),
            )
            .add_systems(
                PostUpdate,
                (send_pose, send_intents, send_messages)
                    .chain()
                    .in_set(SendSystems),
            );
    }
}

/// Where what the server sent is applied, in `PreUpdate`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReceiveSystems;

/// Where what the client does is sent, in `PostUpdate`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SendSystems;

/// The connection to the server.
#[derive(Resource)]
pub struct ServerConnection(pub ClientLink);

/// A message for the server, sent at the end of the frame.
#[derive(Message, Debug, Clone)]
pub struct ToServer(pub ClientMessage);

/// A message from the server that features act on (screens): everything
/// but chunks and the player's state, which this module applies.
#[derive(Message, Debug, Clone)]
pub struct FromServer(pub ServerMessage);

/// Whether the player's first state has arrived, and the hotbar and slot
/// in hand the server last sent. The client changes those itself and
/// sends them; it takes the server's only when the server changed them
/// (a command, or joining), so a state crossing the client's change on
/// its way doesn't undo it.
#[derive(Resource, Debug, Default)]
struct Joined(bool, Option<(Hotbar, ActiveSlot)>);

/// Whether the server last said everything around the player is loaded
/// and sent. The client's [`StreamingStatus`] starts from it each frame
/// (drawing then unsettles it while chunks wait to be meshed).
#[derive(Resource, Debug, Default)]
struct ServerSettled(bool);

/// A world left: the next one's server starts afresh.
fn forget_the_server(mut joined: ResMut<Joined>, mut settled: ResMut<ServerSettled>) {
    *joined = Joined::default();
    *settled = ServerSettled::default();
}

/// The local player mines with the button it holds; the server says how
/// far along it is.
fn give_the_player_mining(add: On<Add<LocalPlayer>>, mut commands: Commands) {
    commands.entity(add.entity).insert_if_new(Mining::default());
}

/// Applies what the server sent: chunks arrive, change and go, and the
/// player's state; the rest goes on as [`FromServer`] messages.
fn receive(
    mut commands: Commands,
    connection: Option<ResMut<ServerConnection>>,
    mut map: ResMut<ChunkMap>,
    mut status: ResMut<StreamingStatus>,
    mut settled: ResMut<ServerSettled>,
    mut joined: ResMut<Joined>,
    mut touched: Local<HashSet<IVec3>>,
    mut player: Query<
        (
            Entity,
            Option<&mut Mining>,
            Option<&mut Hotbar>,
            Option<&mut ActiveSlot>,
        ),
        With<LocalPlayer>,
    >,
    mut forward: MessageWriter<FromServer>,
) {
    let Some(mut connection) = connection else {
        return;
    };
    for message in connection.0.receive() {
        match message {
            ServerMessage::Chunk {
                coord,
                chunk,
                light,
            } => {
                if let Some(old) = map.remove(coord) {
                    commands.entity(old).despawn();
                }
                spawn_chunk(
                    &mut commands,
                    &mut map,
                    coord,
                    chunk,
                    (light, DespawnOnExit(GameState::Playing)),
                );
                touched.insert(coord.0);
            }
            ServerMessage::ChunkChanged { coord, chunk } => {
                if let Some(entity) = map.get(coord) {
                    commands.entity(entity).insert(chunk);
                }
            }
            ServerMessage::LightChanged { coord, light } => {
                if let Some(entity) = map.get(coord) {
                    commands.entity(entity).insert(light);
                }
            }
            ServerMessage::Unload(coord) => {
                if let Some(entity) = map.remove(coord) {
                    commands.entity(entity).despawn();
                }
                touched.insert(coord.0);
            }
            ServerMessage::Settled(now) => settled.0 = now,
            ServerMessage::PlayerState(state) => {
                let Ok((entity, _, hotbar, active)) = player.single_mut() else {
                    continue;
                };
                let sent = (state.hotbar.clone(), state.active);
                let changed = joined.1.as_ref() != Some(&sent);
                apply_state(&mut commands, entity, state, hotbar, active, changed);
                *joined = Joined(true, Some(sent));
            }
            ServerMessage::Mining { target, progress } => {
                if let Ok((_, Some(mut mining), _, _)) = player.single_mut() {
                    // Holding the button is the client's own.
                    mining.target = target;
                    mining.progress = progress;
                }
            }
            other => {
                forward.write(FromServer(other));
            }
        }
    }
    status.loaded_chunks = map.len();
    status.settled = settled.0;
    commands.insert_resource(Touched(std::mem::take(&mut *touched)));
}

/// The chunks that arrived or went this frame.
#[derive(Resource, Debug, Default)]
struct Touched(HashSet<IVec3>);

/// Puts the server's state of the player on the local player: what it
/// carries, its game mode, and its hotbar if the server changed it.
fn apply_state(
    commands: &mut Commands,
    entity: Entity,
    state: PlayerState,
    hotbar: Option<Mut<Hotbar>>,
    active: Option<Mut<ActiveSlot>>,
    hotbar_changed: bool,
) {
    let PlayerState {
        inventory,
        hotbar: new_hotbar,
        active: new_active,
        game_mode,
    } = state;
    commands.entity(entity).insert((inventory, game_mode));
    match (hotbar, active) {
        // Taken quietly: it's the server's change, not the client's to
        // send back.
        (Some(mut hotbar), Some(mut active)) => {
            if hotbar_changed {
                *hotbar.bypass_change_detection() = new_hotbar;
                *active.bypass_change_detection() = new_active;
            }
        }
        _ => {
            commands.entity(entity).insert((new_hotbar, new_active));
        }
    }
}

/// Marks the chunks around those that arrived or went as awaiting their
/// neighbours, or not, by whether all 26 around each are here.
fn update_awaiting(mut commands: Commands, map: Res<ChunkMap>, touched: Option<Res<Touched>>) {
    let Some(touched) = touched else {
        return;
    };
    let candidates: HashSet<IVec3> = touched
        .0
        .iter()
        .flat_map(|&coord| ChunkCoord(coord).neighbours().map(|c| c.0).chain([coord]))
        .collect();
    for coord in candidates {
        let Some(entity) = map.get(ChunkCoord(coord)) else {
            continue;
        };
        let complete = ChunkCoord(coord)
            .neighbours()
            .all(|neighbour| map.get(neighbour).is_some());
        if complete {
            commands.entity(entity).remove::<AwaitingNeighbours>();
        } else {
            commands.entity(entity).insert(AwaitingNeighbours);
        }
    }
    commands.remove_resource::<Touched>();
}

/// Sends where the local player is when it's moved or turned.
fn send_pose(
    mut connection: Option<ResMut<ServerConnection>>,
    player: Query<(&Body, &Look, &Player), With<LocalPlayer>>,
    mut last: Local<Option<Pose>>,
) {
    let (Some(connection), Ok((body, look, player))) = (&mut connection, player.single()) else {
        return;
    };
    let pose = Pose {
        feet: body.position,
        velocity: body.velocity,
        on_ground: body.on_ground,
        look: *look,
        player: player.clone(),
    };
    if last.as_ref() != Some(&pose) {
        *last = Some(pose.clone());
        connection.0.send(ClientMessage::Pose(pose));
    }
}

/// Sends what the local player does: a use (which the server's steps act
/// on, so the client's is claimed here), holding the break button, and
/// its hotbar when they change.
fn send_intents(
    mut connection: Option<ResMut<ServerConnection>>,
    joined: Res<Joined>,
    mut player: Query<
        (
            &mut UseAction,
            Option<Ref<Mining>>,
            Option<Ref<Hotbar>>,
            Option<Ref<ActiveSlot>>,
        ),
        With<LocalPlayer>,
    >,
    mut last_holding: Local<bool>,
) {
    let (Some(connection), Ok((mut action, mining, hotbar, active))) =
        (&mut connection, player.single_mut())
    else {
        return;
    };
    if let Some(using) = action.pending() {
        connection.0.send(ClientMessage::Use(using));
        action.claim();
    }
    let holding = mining.is_some_and(|mining| mining.holding);
    if holding != *last_holding {
        *last_holding = holding;
        connection.0.send(ClientMessage::Mining(holding));
    }
    // The client's own once it's joined: changes it made go to the server.
    if joined.0 {
        if let Some(hotbar) = hotbar.filter(|hotbar| hotbar.is_changed()) {
            connection.0.send(ClientMessage::Hotbar((*hotbar).clone()));
        }
        if let Some(active) = active.filter(|active| active.is_changed()) {
            connection.0.send(ClientMessage::ActiveSlot(*active));
        }
    }
}

/// Sends the [`ToServer`] messages features wrote this frame.
fn send_messages(
    mut connection: Option<ResMut<ServerConnection>>,
    mut messages: MessageReader<ToServer>,
) {
    let Some(connection) = &mut connection else {
        messages.clear();
        return;
    };
    for ToServer(message) in messages.read() {
        connection.0.send(message.clone());
    }
}

#[cfg(test)]
mod tests {
    use bevy::math::DVec3;
    use defaria_core::{
        content::{ItemState, base},
        game_mode::GameMode,
        inventory::{Inventory, ItemStack},
        light::ChunkLight,
        net::{ServerLink, link},
        player::local_player,
        use_action::Use,
        world::{Chunk, WorldPlugin},
    };

    use super::*;

    /// A client with a local player, connected to a server this test plays.
    fn client() -> (App, ServerLink) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(base().clone())
            .add_plugins((WorldPlugin, ClientNetPlugin));
        let (client, server) = link();
        app.insert_resource(ServerConnection(client));
        app.world_mut()
            .spawn(local_player(DVec3::new(0.5, 0.0, 0.5), Look::default()));
        app.update();
        (app, server)
    }

    fn chunk(coord: IVec3) -> ServerMessage {
        ServerMessage::Chunk {
            coord: ChunkCoord(coord),
            chunk: Chunk::default(),
            light: ChunkLight::default(),
        }
    }

    fn awaiting(app: &App, coord: IVec3) -> Option<bool> {
        let entity = app.world().resource::<ChunkMap>().get(ChunkCoord(coord))?;
        Some(app.world().get::<AwaitingNeighbours>(entity).is_some())
    }

    /// Chunks the server sends are spawned, and wait to be drawn until all
    /// 26 around them have come; one that goes is despawned, and the
    /// chunks around it wait again.
    #[test]
    fn chunks_arrive_and_go() {
        let (mut app, mut server) = client();
        server.send(chunk(IVec3::ZERO));
        app.update();
        assert_eq!(awaiting(&app, IVec3::ZERO), Some(true));
        for neighbour in ChunkCoord(IVec3::ZERO).neighbours() {
            server.send(chunk(neighbour.0));
        }
        app.update();
        assert_eq!(awaiting(&app, IVec3::ZERO), Some(false));
        assert_eq!(awaiting(&app, IVec3::X), Some(true));

        server.send(ServerMessage::Unload(ChunkCoord(IVec3::X)));
        app.update();
        assert_eq!(awaiting(&app, IVec3::X), None);
        assert_eq!(awaiting(&app, IVec3::ZERO), Some(true));
        assert_eq!(app.world().resource::<StreamingStatus>().loaded_chunks, 26);
    }

    fn state(hotbar: Hotbar, active: usize, game_mode: GameMode) -> ServerMessage {
        let mut inventory = Inventory::new();
        let stone = base().default_item("defaria:stone").unwrap();
        inventory.add(base(), ItemStack::new(stone, 3));
        ServerMessage::PlayerState(PlayerState {
            inventory,
            hotbar,
            active: ActiveSlot(active),
            game_mode,
        })
    }

    fn local<T: Component + Clone>(app: &mut App) -> T {
        app.world_mut()
            .query_filtered::<&T, With<LocalPlayer>>()
            .single(app.world())
            .unwrap()
            .clone()
    }

    /// The player's state goes on the local player. Its hotbar and slot in
    /// hand are the client's own once it has them: a state that only
    /// echoes what the server had leaves the client's change alone, and
    /// one in which the server changed them takes over.
    #[test]
    fn the_players_state_is_applied() {
        let (mut app, mut server) = client();
        let item = |id: &str| -> Option<ItemState> { base().default_item(id) };
        let mut hotbar = Hotbar::default();
        hotbar.assign(0, item("defaria:stone"));
        server.send(state(hotbar.clone(), 0, GameMode::Survival));
        app.update();
        let stone = item("defaria:stone").unwrap();
        assert_eq!(local::<Inventory>(&mut app).count(stone), 3);
        assert_eq!(local::<GameMode>(&mut app), GameMode::Survival);
        assert_eq!(local::<ActiveSlot>(&mut app), ActiveSlot(0));

        // The client picks slot 3; the server's next state hasn't seen it.
        app.world_mut()
            .query_filtered::<&mut ActiveSlot, With<LocalPlayer>>()
            .single_mut(app.world_mut())
            .unwrap()
            .0 = 3;
        server.send(state(hotbar.clone(), 0, GameMode::Creative));
        app.update();
        assert_eq!(local::<ActiveSlot>(&mut app), ActiveSlot(3));
        assert_eq!(local::<GameMode>(&mut app), GameMode::Creative);
        assert!(
            server
                .receive()
                .contains(&ClientMessage::ActiveSlot(ActiveSlot(3)))
        );

        // The server moves it (a command): that's the slot now, and the
        // client doesn't send it back as its own.
        server.receive();
        server.send(state(hotbar, 5, GameMode::Creative));
        app.update();
        assert_eq!(local::<ActiveSlot>(&mut app), ActiveSlot(5));
        app.update();
        let echoes = server.receive();
        assert!(
            !echoes.iter().any(|message| matches!(
                message,
                ClientMessage::ActiveSlot(_) | ClientMessage::Hotbar(_)
            )),
            "{echoes:?}"
        );
    }

    /// A use goes to the server, and is claimed on the client so no step
    /// there acts on it; holding the break button is sent as it changes.
    #[test]
    fn uses_and_mining_are_sent() {
        let (mut app, mut server) = client();
        server.receive();
        let using = Use {
            eyes: Transform::default(),
            target: None,
            sneak: true,
        };
        let player = app
            .world_mut()
            .query_filtered::<Entity, With<LocalPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(player)
            .insert(UseAction(Some(using)));
        app.world_mut().get_mut::<Mining>(player).unwrap().holding = true;
        app.update();
        let sent = server.receive();
        assert!(sent.contains(&ClientMessage::Use(using)));
        assert!(sent.contains(&ClientMessage::Mining(true)));
        assert_eq!(
            app.world().get::<UseAction>(player).unwrap().pending(),
            None
        );
        app.update();
        let again = server.receive();
        assert!(!again.iter().any(|m| matches!(m, ClientMessage::Mining(_))));
    }
}
