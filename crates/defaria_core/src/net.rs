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
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc::{Receiver, Sender, TryRecvError, channel},
};

use bevy::prelude::*;

use crate::{
    block_entity::Value,
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
    /// loses nothing. `made_after` is the newest input the client sent
    /// before: those after it were made in the frame it sent these, and
    /// came out together because of its frame, not the network.
    Inputs { inputs: Vec<Input>, made_after: u32 },
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
    /// A mod's client script sent its server's scripts a message
    /// (`net.send`, see `script::channel`).
    Script(ScriptMessage),
}

impl ClientMessage {
    /// Whether it must arrive: all but [`ClientMessage::Inputs`], which are
    /// sent again until they're applied.
    pub fn reliable(&self) -> bool {
        !matches!(self, ClientMessage::Inputs { .. })
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
    /// A mod's server script sent this client's scripts a message
    /// (`net.send` or `net.broadcast`, see `script::channel`).
    Script(ScriptMessage),
    /// The network is dropping the client, and why (it fell too far
    /// behind): the last thing on its control stream before the
    /// connection closes, so the reason arrives even if the close is
    /// lost. `quic`'s own: the client's network end takes it, and the game
    /// never sees it.
    Dropped(String),
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

    /// Whether it's one of the world's: chunks, their changes and drops,
    /// whether the client's settled, and its edits done (after the chunks
    /// they changed). Over the network these go on a stream of their own,
    /// and count towards the link's [`Backlog`].
    pub fn bulk(&self) -> bool {
        matches!(
            self,
            ServerMessage::Chunk { .. }
                | ServerMessage::ChunkChanged { .. }
                | ServerMessage::LightChanged { .. }
                | ServerMessage::Unload(_)
                | ServerMessage::Settled(_)
                | ServerMessage::EditDone(_)
        )
    }
}

/// What a message counts towards its link's [`Backlog`]: about how many
/// bytes it takes to send, at most, for the world's messages
/// ([`ServerMessage::bulk`]), and nothing for the rest.
pub trait Weigh {
    fn weight(&self) -> usize;
}

impl Weigh for ServerMessage {
    fn weight(&self) -> usize {
        // What the voxels hold, before they're deflated: postcard writes
        // each state in no more bytes than it's kept in.
        const OVERHEAD: usize = 64;
        match self {
            ServerMessage::Chunk { chunk, light, .. } => {
                OVERHEAD
                    + chunk.voxels().heap_bytes()
                    + chunk.fluid_layer().heap_bytes()
                    + light.levels().heap_bytes()
            }
            ServerMessage::ChunkChanged { chunk, .. } => {
                OVERHEAD + chunk.voxels().heap_bytes() + chunk.fluid_layer().heap_bytes()
            }
            ServerMessage::LightChanged { light, .. } => OVERHEAD + light.levels().heap_bytes(),
            message if message.bulk() => 16,
            _ => 0,
        }
    }
}

impl Weigh for ClientMessage {
    fn weight(&self) -> usize {
        0
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

/// A message from a mod's scripts on one side to the same mod's on the
/// other: on a `channel` the mod names, a plain value or none. See
/// `script::channel` for its limits.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScriptMessage {
    pub mod_id: String,
    pub channel: String,
    #[serde(with = "flat")]
    pub value: Option<Value>,
}

/// A plain value as a flat list of tokens, for [`ScriptMessage`]: decoding
/// a nested form would recurse as deep as the bytes say, so a client could
/// overflow the server's stack; this one is read back no deeper than
/// [`MAX_DEPTH`].
mod flat {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

    use crate::block_entity::{MAX_DEPTH, Value};

    #[derive(Serialize, Deserialize)]
    enum Token {
        Bool(bool),
        Int(i64),
        Float(f64),
        Text(String),
        /// A list of this many values, which follow.
        List(u32),
        /// A table of this many entries, which follow: each a `Text` key,
        /// then its value.
        Table(u32),
    }

    fn flatten(value: &Value, out: &mut Vec<Token>) {
        match value {
            Value::Bool(b) => out.push(Token::Bool(*b)),
            Value::Int(n) => out.push(Token::Int(*n)),
            Value::Float(n) => out.push(Token::Float(*n)),
            Value::Text(text) => out.push(Token::Text(text.clone())),
            Value::List(items) => {
                out.push(Token::List(items.len() as u32));
                for item in items {
                    flatten(item, out);
                }
            }
            Value::Table(entries) => {
                out.push(Token::Table(entries.len() as u32));
                for (key, value) in entries {
                    out.push(Token::Text(key.clone()));
                    flatten(value, out);
                }
            }
        }
    }

    fn build(tokens: &mut std::vec::IntoIter<Token>, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(format!("a value nests deeper than {MAX_DEPTH} tables"));
        }
        Ok(match tokens.next().ok_or("a value ends early")? {
            Token::Bool(b) => Value::Bool(b),
            Token::Int(n) => Value::Int(n),
            Token::Float(n) => Value::Float(n),
            Token::Text(text) => Value::Text(text),
            Token::List(len) => {
                // No more items than there are tokens left.
                let len = (len as usize).min(tokens.len());
                let mut items = Vec::with_capacity(len);
                for _ in 0..len {
                    items.push(build(tokens, depth + 1)?);
                }
                Value::List(items)
            }
            Token::Table(len) => {
                let mut entries = BTreeMap::new();
                for _ in 0..len {
                    let Some(Token::Text(key)) = tokens.next() else {
                        return Err("a table's key isn't text".into());
                    };
                    entries.insert(key, build(tokens, depth + 1)?);
                }
                Value::Table(entries)
            }
        })
    }

    pub fn serialize<S: Serializer>(
        value: &Option<Value>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let tokens = value.as_ref().map(|value| {
            let mut tokens = Vec::new();
            flatten(value, &mut tokens);
            tokens
        });
        tokens.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Value>, D::Error> {
        let Some(tokens) = Option::<Vec<Token>>::deserialize(deserializer)? else {
            return Ok(None);
        };
        let mut tokens = tokens.into_iter();
        let value = build(&mut tokens, 0).map_err(D::Error::custom)?;
        if tokens.len() > 0 {
            return Err(D::Error::custom("a value has more after it"));
        }
        Ok(Some(value))
    }
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
    backlog: Arc<Backlog>,
}

/// How far behind the network is with a link's world messages
/// ([`ServerMessage::bulk`]): about how many bytes of them wait to be
/// written to the client, and how many may wait before the server holds
/// back new chunks. Shared by both ends of a link. A message counts from
/// the moment the game sends it ([`Link::send`] adds its [`Weigh::weight`]
/// as pending), so nothing the game sent is missed while it waits in the
/// link; the network moves it from pending to queued (its real bytes) as
/// it takes it ([`Backlog::carried`]), and takes those off once they're
/// written ([`Backlog::written`]). A link in memory has no budget, so it's
/// never behind: its other end's [`Link::receive`] takes the weight off.
#[derive(Debug, Default)]
pub struct Backlog {
    /// Sent by the game, not yet taken by the network: their weights.
    pending: AtomicUsize,
    /// Taken by the network, not yet written: their bytes.
    queued: AtomicUsize,
    /// 0 for none.
    budget: AtomicUsize,
}

impl Backlog {
    /// Holds back new chunks once `bytes` of them are waiting.
    pub fn set_budget(&self, bytes: usize) {
        self.budget.store(bytes, Ordering::Relaxed);
    }

    /// A message of `weight` the network took, now `bytes` waiting for its
    /// writer. (Counted as queued before it stops being pending, so it's
    /// never counted nowhere.)
    pub fn carried(&self, weight: usize, bytes: usize) {
        self.queued.fetch_add(bytes, Ordering::SeqCst);
        self.taken(weight);
    }

    /// `bytes` that were waiting are written.
    pub fn written(&self, bytes: usize) {
        let _ = (self.queued).try_update(Ordering::SeqCst, Ordering::SeqCst, |queued| {
            Some(queued.saturating_sub(bytes))
        });
    }

    fn taken(&self, weight: usize) {
        if weight > 0 {
            let _ = (self.pending).try_update(Ordering::SeqCst, Ordering::SeqCst, |pending| {
                Some(pending.saturating_sub(weight))
            });
        }
    }

    /// How many bytes the network has taken and not yet written.
    pub fn queued(&self) -> usize {
        self.queued.load(Ordering::SeqCst)
    }

    /// About how many bytes wait now, sent by the game or taken by the
    /// network.
    pub fn waiting(&self) -> usize {
        self.pending.load(Ordering::SeqCst) + self.queued.load(Ordering::SeqCst)
    }

    /// How many more bytes of new chunks may be sent now: unlimited with
    /// no budget.
    pub fn room(&self) -> usize {
        match self.budget.load(Ordering::Relaxed) {
            0 => usize::MAX,
            budget => budget.saturating_sub(self.waiting()),
        }
    }
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
    let backlog = Arc::new(Backlog::default());
    (
        Link {
            send: to_server,
            receive: Mutex::new(from_server),
            closed: false,
            backlog: backlog.clone(),
        },
        Link {
            send: to_client,
            receive: Mutex::new(from_client),
            closed: false,
            backlog,
        },
    )
}

impl<Out: Weigh, In: Weigh> Link<Out, In> {
    /// Sends a message, if the other end is still there.
    pub fn send(&mut self, message: Out) {
        let weight = message.weight();
        if weight > 0 {
            self.backlog.pending.fetch_add(weight, Ordering::SeqCst);
        }
        if self.send.send(message).is_err() {
            self.backlog.taken(weight);
            self.closed = true;
        }
    }

    /// The messages that have arrived, oldest first, no longer counted in
    /// the backlog.
    pub fn receive(&mut self) -> Vec<In> {
        let messages = self.receive_counted();
        self.backlog.taken(messages.iter().map(Weigh::weight).sum());
        messages
    }

    /// The messages that have arrived, oldest first, still counted in the
    /// backlog as pending: for the network's end, which says when each is
    /// [`Backlog::carried`].
    pub fn receive_counted(&mut self) -> Vec<In> {
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
}

impl<Out, In> Link<Out, In> {
    /// Whether the other end has gone.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// How far behind the network is with this link's world messages.
    pub fn backlog(&self) -> &Arc<Backlog> {
        &self.backlog
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

    /// A link in memory is never behind; one with a budget has room for
    /// what's left of it, shared by both ends.
    #[test]
    fn a_backlog_has_room_for_what_its_budget_leaves() {
        let (mut client, mut server) = link();
        assert_eq!(server.backlog().room(), usize::MAX);
        client.backlog().set_budget(100);
        client.backlog().carried(0, 30);
        assert_eq!(server.backlog().room(), 70);
        client.backlog().carried(0, 120);
        assert_eq!(server.backlog().room(), 0);
        client.backlog().written(150);
        assert_eq!(server.backlog().room(), 100);

        // World messages count from when they're sent, until they're
        // received (in memory), or carried and written (the network).
        let chunk = ServerMessage::Chunk {
            coord: ChunkCoord(IVec3::ZERO),
            chunk: Chunk::default(),
            light: ChunkLight::default(),
        };
        let weight = chunk.weight();
        assert!(weight > 0);
        assert_eq!(ServerMessage::CloseScreen.weight(), 0);
        server.send(chunk.clone());
        server.send(ServerMessage::CloseScreen);
        assert_eq!(server.backlog().waiting(), weight);
        assert_eq!(client.receive().len(), 2);
        assert_eq!(server.backlog().waiting(), 0);
        server.send(chunk);
        assert_eq!(client.receive_counted().len(), 1);
        assert_eq!(server.backlog().waiting(), weight);
        client.backlog().carried(weight, 10);
        assert_eq!(server.backlog().waiting(), 10);
        client.backlog().written(10);
        assert_eq!(server.backlog().waiting(), 0);
    }

    /// A message between scripts with every kind of plain value in it.
    fn script_message() -> ScriptMessage {
        use std::collections::BTreeMap;
        let mut table = BTreeMap::new();
        table.insert("n".into(), Value::Int(-3));
        table.insert("x".into(), Value::Float(0.25));
        table.insert(
            "list".into(),
            Value::List(vec![Value::Bool(true), Value::Text("two".into())]),
        );
        table.insert("empty".into(), Value::Table(BTreeMap::new()));
        ScriptMessage {
            mod_id: "test".into(),
            channel: "ping".into(),
            value: Some(Value::Table(table)),
        }
    }

    /// A value nested deeper than a script could make is refused, rather
    /// than read as deep as the bytes say.
    #[test]
    fn a_message_nested_too_deep_is_refused() {
        let mut value = Value::Int(1);
        for _ in 0..crate::block_entity::MAX_DEPTH {
            value = Value::List(vec![value]);
        }
        let message = |value| ScriptMessage {
            mod_id: "test".into(),
            channel: "c".into(),
            value: Some(value),
        };
        let fits = encode(&message(value.clone()));
        assert!(decode::<ScriptMessage>(&fits).is_ok());
        let deep = encode(&message(Value::List(vec![value])));
        assert!(decode::<ScriptMessage>(&deep).is_err());
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
            ServerMessage::Script(script_message()),
            ServerMessage::Dropped("why".into()),
        ];
        for message in server {
            let back: ServerMessage = decode(&encode(&message)).unwrap();
            assert_eq!(back, message);
        }
        let client = [
            ClientMessage::Inputs {
                inputs: vec![Input {
                    tick: 3,
                    controls: Controls {
                        forward: 1.0,
                        jump: true,
                        ..Default::default()
                    },
                    look: pose.look,
                    teleport: Some(motion),
                }],
                made_after: 2,
            },
            ClientMessage::Mining(true),
            ClientMessage::ToggleGameMode,
            ClientMessage::Paused(true),
            ClientMessage::Script(ScriptMessage {
                value: None,
                ..script_message()
            }),
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
