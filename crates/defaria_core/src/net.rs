//! The messages between a server and its clients, and the links that carry
//! them. See "Multiplayer: a server app and client apps" in
//! `docs/decisions.md`.
//!
//! A server app owns the world: it generates, loads, simulates and saves
//! it, and decides everything. A client app draws what it's sent and sends
//! what its player does: what it pressed and where it looked each physics
//! step ([`ClientMessage::Inputs`]), and its intents (a use, holding the
//! break button, the hotbar). Single player runs both, in one process,
//! over a [`Link`] in memory; `quic` carries them over the network.
//!
//! Each message is reliable (it arrives, in order with the others of its
//! kind) or not ([`ClientMessage::reliable`]): what's sent again and again,
//! where only the newest matters, may be lost.

use std::sync::{
    Mutex,
    mpsc::{Receiver, Sender, TryRecvError, channel},
};

use bevy::prelude::*;

use crate::{
    content::BlockState,
    game_mode::GameMode,
    hotbar::{ActiveSlot, Hotbar},
    inventory::{Inventory, ItemStack},
    light::ChunkLight,
    machine::{EnergyBuffer, MachineActive, Slots, Tanks},
    player::{Controls, Look, Motion},
    recipe::RecipeProgress,
    replication::{EntityPose, Kind, Pose},
    use_action::Use,
    world::{Chunk, ChunkCoord},
};

/// What a client sends its server.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ClientMessage {
    /// The player's inputs for the physics steps the server hasn't said
    /// it's applied yet, oldest first. Each is sent until the server says
    /// it's applied it ([`ServerMessage::Moved`]), so losing some of these
    /// loses nothing.
    Inputs(Vec<Input>),
    /// The player used something (right click), as `use_action` acts on,
    /// after its input `tick` (the server waits until it's stepped the
    /// player that far, so it acts on the use where the client did).
    /// `edit` numbers the block the client placed at once, predicting the
    /// server would ([`ServerMessage::EditDone`] answers it).
    Use {
        using: Use,
        tick: u32,
        edit: Option<u32>,
    },
    /// Whether the player holds the break button ([`Mining::holding`]).
    Mining(bool),
    /// The player's hotbar slots.
    Hotbar(Hotbar),
    /// The hotbar slot in hand.
    ActiveSlot(ActiveSlot),
    /// Switch between creative and survival.
    ToggleGameMode,
    /// Move items between the player and the creative catalog, or between
    /// the player and the machine whose screen is open.
    Transfer(Transfer),
    /// The player closed the machine screen it had open, of the machine
    /// at this voxel.
    CloseScreen(IVec3),
    /// The player paused (its pause menu is open), or stopped.
    Paused(bool),
}

impl ClientMessage {
    /// Whether it must arrive: all but [`ClientMessage::Inputs`], which are
    /// sent again until they're applied.
    pub fn reliable(&self) -> bool {
        !matches!(self, ClientMessage::Inputs(_))
    }
}

/// A player's input for one physics step: what it pressed and which way it
/// looked, numbered from 1 in the order they're stepped.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Input {
    pub tick: u32,
    pub controls: Controls,
    pub look: Look,
    /// Where the player was put before this step, if it was (a scenario's
    /// `camera`): the server puts it there too.
    pub teleport: Option<Motion>,
}

/// A move of items the player asks for, from a screen.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Transfer {
    /// Up to `count` of `kind` from the creative catalog to the player
    /// (`to_player`) or back.
    Catalog {
        kind: ItemStack,
        count: u32,
        to_player: bool,
    },
    /// Up to `count` of `kind` from the player into the open machine, into
    /// slot group `first` first if one's chosen
    /// (`machine::exchange::into_machine_from`).
    IntoMachine {
        kind: ItemStack,
        count: u32,
        first: Option<usize>,
    },
    /// Up to `count` out of slot `index` of the open machine's slot group
    /// `group` to the player (`machine::exchange::out_of_machine`).
    OutOfMachine {
        group: usize,
        index: usize,
        count: u32,
    },
}

/// What a server sends a client.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ServerMessage {
    /// A chunk in the client's range, whole: its blocks, fluid and light.
    /// Also sent again when it changes.
    Chunk {
        coord: ChunkCoord,
        chunk: Chunk,
        light: ChunkLight,
    },
    /// A chunk's blocks or fluid changed visibly.
    ChunkChanged { coord: ChunkCoord, chunk: Chunk },
    /// A chunk's light changed.
    LightChanged {
        coord: ChunkCoord,
        light: ChunkLight,
    },
    /// A chunk left the client's range.
    Unload(ChunkCoord),
    /// The client's player, as the server keeps it: what it carries, its
    /// hotbar and game mode.
    PlayerState(PlayerState),
    /// The block the client's player is mining and how far along it is,
    /// when that changes (every frame, while it mines).
    Mining {
        target: Option<(IVec3, BlockState)>,
        progress: f32,
    },
    /// Whether everything in the client's range is loaded and sent.
    Settled(bool),
    /// The server has acted on the use that carried this edit and the
    /// ones before it, and the chunks it changed were sent before this:
    /// the client's world is the server's there now. Sent with the chunks,
    /// so it comes after them.
    EditDone(u32),
    /// Where the server has the client's player after the last of its
    /// inputs it's applied (tick 0 for none yet): what the client predicted
    /// from it is checked against this. Not reliable: it's sent again as
    /// the player moves.
    Moved { tick: u32, motion: Motion },
    /// Open the screen of the machine at this voxel, in this state.
    OpenScreen { pos: IVec3, state: BlockState },
    /// What the machine whose screen is open holds, when it changes.
    MachineContents(MachineContents),
    /// The machine screen closed (the machine went).
    CloseScreen,
    /// A replicated entity (`replication`) came into the client's range:
    /// which (its id, for as long as it's in range), what kind, and where.
    Spawn { id: u64, kind: Kind, pose: Pose },
    /// A replicated entity left the client's range, or went.
    Despawn(u64),
    /// Where the replicated entities in the client's range were at `time`
    /// (seconds of the server's clock). Not reliable: another follows
    /// soon.
    Snapshot {
        time: f64,
        entities: Vec<EntityPose>,
    },
}

impl ServerMessage {
    /// Whether it must arrive: all but [`ServerMessage::Moved`], which is
    /// sent again as things move.
    pub fn reliable(&self) -> bool {
        !matches!(
            self,
            ServerMessage::Moved { .. } | ServerMessage::Snapshot { .. }
        )
    }
}

/// The parts of a player the server owns and its client shows.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlayerState {
    pub inventory: Inventory,
    pub hotbar: Hotbar,
    pub active: ActiveSlot,
    pub game_mode: GameMode,
}

/// What a machine holds, for its screen: each part it has.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct MachineContents {
    pub slots: Option<Slots>,
    pub tanks: Option<Tanks>,
    pub energy: Option<EnergyBuffer>,
    pub progress: Option<RecipeProgress>,
    pub active: Option<MachineActive>,
}

/// A message as bytes, for the network.
pub fn encode<T: serde::Serialize>(message: &T) -> Vec<u8> {
    postcard::to_allocvec(message).expect("messages always encode")
}

/// A message from its bytes, if they're one.
pub fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    postcard::from_bytes(bytes).map_err(|error| error.to_string())
}

/// One end of a connection: messages of `Out` go one way, `In` the other.
pub struct Link<Out, In> {
    send: Sender<Out>,
    // Behind a lock only so a link can be a component or resource, which
    // must be shareable between threads; one system at a time uses it.
    receive: Mutex<Receiver<In>>,
    closed: bool,
}

/// The client's end of a link.
pub type ClientLink = Link<ClientMessage, ServerMessage>;
/// The server's end of a link.
pub type ServerLink = Link<ServerMessage, ClientMessage>;

/// A connection in memory, for a server in the same process: its client
/// end and its server end.
pub fn link() -> (ClientLink, ServerLink) {
    let (to_server, from_client) = channel();
    let (to_client, from_server) = channel();
    (
        Link {
            send: to_server,
            receive: Mutex::new(from_server),
            closed: false,
        },
        Link {
            send: to_client,
            receive: Mutex::new(from_client),
            closed: false,
        },
    )
}

impl<Out, In> Link<Out, In> {
    /// Sends a message, if the other end is still there.
    pub fn send(&mut self, message: Out) {
        if self.send.send(message).is_err() {
            self.closed = true;
        }
    }

    /// The messages that have arrived, oldest first.
    pub fn receive(&mut self) -> Vec<In> {
        let mut messages = Vec::new();
        let receive = self.receive.get_mut().unwrap_or_else(|e| e.into_inner());
        loop {
            match receive.try_recv() {
                Ok(message) => messages.push(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.closed = true;
                    break;
                }
            }
        }
        messages
    }

    /// Whether the other end has gone.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_carries_messages_both_ways_in_order() {
        let (mut client, mut server) = link();
        client.send(ClientMessage::Mining(true));
        client.send(ClientMessage::ToggleGameMode);
        assert_eq!(
            server.receive(),
            vec![ClientMessage::Mining(true), ClientMessage::ToggleGameMode]
        );
        assert!(server.receive().is_empty());
        server.send(ServerMessage::Settled(true));
        assert_eq!(client.receive(), vec![ServerMessage::Settled(true)]);
    }

    #[test]
    fn a_link_knows_when_its_other_end_goes() {
        let (mut client, server) = link();
        drop(server);
        assert!(client.receive().is_empty());
        assert!(client.is_closed());
        client.send(ClientMessage::CloseScreen(IVec3::ZERO));
        assert!(client.is_closed());
    }

    /// Every kind of message comes back from its bytes as it went, and a
    /// chunk of one block (most chunks: air or rock) is a few bytes.
    #[test]
    fn messages_round_trip_through_bytes() {
        use crate::{
            content::{base, block},
            inventory::ItemStack,
            world::{Chunk, ChunkCoord},
        };
        let mut chunk = Chunk::default();
        chunk.set(1, 2, 3, block("stone"));
        chunk.set_fluid_at(
            IVec3::new(4, 4, 4),
            crate::world::VoxelFluid::of(block("water"), 500),
            crate::content::FULL_FLUID,
        );
        let stone = base().default_item("defaria:stone").unwrap();
        let motion = Motion {
            feet: bevy::math::DVec3::new(1.5, -2.0, 1e6),
            velocity: bevy::math::DVec3::new(0.0, -3.0, 0.25),
            on_ground: false,
            flying: true,
            sneaking: false,
            sprinting: true,
            eye_height: 1.5,
        };
        let pose = Pose {
            feet: motion.feet,
            look: Look {
                yaw: 1.0,
                pitch: -0.5,
            },
            crouched: true,
        };
        let mut inventory = Inventory::new();
        inventory.add(base(), ItemStack::new(stone, 12));
        let server = [
            ServerMessage::Chunk {
                coord: ChunkCoord(IVec3::new(-3, 2, 7)),
                chunk: chunk.clone(),
                light: ChunkLight::default(),
            },
            ServerMessage::ChunkChanged {
                coord: ChunkCoord(IVec3::ZERO),
                chunk,
            },
            ServerMessage::Unload(ChunkCoord(IVec3::ONE)),
            ServerMessage::PlayerState(PlayerState {
                inventory,
                hotbar: Hotbar::default(),
                active: ActiveSlot(4),
                game_mode: GameMode::Survival,
            }),
            ServerMessage::Mining {
                target: Some((IVec3::new(1, 2, 3), block("stone"))),
                progress: 0.5,
            },
            ServerMessage::Settled(true),
            ServerMessage::OpenScreen {
                pos: IVec3::new(9, 8, 7),
                state: block("stone"),
            },
            ServerMessage::MachineContents(MachineContents::default()),
            ServerMessage::CloseScreen,
            ServerMessage::Moved { tick: 7, motion },
            ServerMessage::Spawn {
                id: 42,
                kind: Kind::Player,
                pose,
            },
            ServerMessage::Snapshot {
                time: 12.5,
                entities: vec![EntityPose { id: 42, pose }],
            },
            ServerMessage::Despawn(42),
        ];
        for message in server {
            let back: ServerMessage = decode(&encode(&message)).unwrap();
            assert_eq!(back, message);
        }
        let client = [
            ClientMessage::Inputs(vec![Input {
                tick: 3,
                controls: Controls {
                    forward: 1.0,
                    jump: true,
                    ..Default::default()
                },
                look: pose.look,
                teleport: Some(motion),
            }]),
            ClientMessage::Mining(true),
            ClientMessage::ToggleGameMode,
            ClientMessage::Paused(true),
            ClientMessage::CloseScreen(IVec3::new(1, -2, 3)),
            ClientMessage::Transfer(Transfer::Catalog {
                kind: ItemStack::new(stone, 1),
                count: 10,
                to_player: true,
            }),
        ];
        for message in client {
            let back: ClientMessage = decode(&encode(&message)).unwrap();
            assert_eq!(back, message);
        }
        let air = ServerMessage::Chunk {
            coord: ChunkCoord(IVec3::ZERO),
            chunk: Chunk::default(),
            light: ChunkLight::default(),
        };
        assert!(encode(&air).len() < 32, "{} bytes", encode(&air).len());
        assert!(decode::<ServerMessage>(&[255, 255, 255]).is_err());
    }
}
