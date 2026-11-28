//! The channel a mod's scripts on a client and on its server talk
//! through: `net.send` on one side, the same mod's `on_message` hooks on
//! the other. A message is a channel name the mod picks and a plain value
//! (or none), and goes as a [`ScriptMessage`] in the game's own protocol
//! (`net::ClientMessage::Script`, `net::ServerMessage::Script`).
//!
//! - On a client, `net.send(channel, value?)` sends its server's scripts
//!   a message, and returns whether it went: a client sends at most
//!   [`MESSAGES_PER_SECOND`] (in bursts of up to [`MESSAGE_BURST`]), and
//!   past that its messages are dropped, so a script can't flood the
//!   server. The server holds each connection to the same budget, and
//!   drops what's over it.
//! - On the server, `net.send(player, channel, value?)` sends one player's
//!   client a message (`player` is the `player` an `on_message` was
//!   handed, or its `id`), and `net.broadcast(channel, value?)` every
//!   player's.
//!
//! Either side's message is at most [`MAX_MESSAGE_BYTES`] encoded, its
//! channel at most [`MAX_CHANNEL_BYTES`], and one hook call sends at most
//! [`MESSAGES_PER_CALL`]: more is an error, which fails the hook.
//! What a hook sends is kept in the runtime's [`Outbox`], which each
//! side's systems empty onto the connection once a frame.

use std::sync::{Arc, Mutex};

use mlua::{Lua, Table, Value};

use super::{Side, api::from_lua, sandbox::Meter};
use crate::{block_entity, net::ScriptMessage};

/// The hook each side calls for a message from the other:
/// `on_message(channel, value, player)`, `player` nil on a client.
pub const ON_MESSAGE: &str = "on_message";

/// The most a message may take encoded, its mod's id and channel
/// included, in bytes.
pub const MAX_MESSAGE_BYTES: usize = 4096;
/// The longest a channel's name may be, in bytes.
pub const MAX_CHANNEL_BYTES: usize = 64;
/// How many messages one hook call may send.
pub const MESSAGES_PER_CALL: u64 = 16;
/// How many messages a second a client's scripts may send, on average.
pub const MESSAGES_PER_SECOND: f64 = 20.0;
/// How many a client's scripts may send at once, after a quiet spell.
pub const MESSAGE_BURST: f64 = 40.0;

/// Who a message is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recipient {
    /// The server's scripts: what a client sends.
    Server,
    /// The client of the player with this id: the bits of its entity on
    /// the server, as `player.id` gives it.
    Player(u64),
    /// Every player's client.
    Everyone,
}

/// A message a script sent, waiting to go.
#[derive(Debug, Clone, PartialEq)]
pub struct Outgoing {
    pub to: Recipient,
    pub message: ScriptMessage,
}

/// How many messages may go now: [`MESSAGE_BURST`] at most, refilled at
/// [`MESSAGES_PER_SECOND`]. A client's scripts are held to one, and the
/// server holds each connection to another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MessageBudget {
    tokens: f64,
}

impl Default for MessageBudget {
    fn default() -> Self {
        Self {
            tokens: MESSAGE_BURST,
        }
    }
}

impl MessageBudget {
    /// `seconds` more have passed.
    pub fn refill(&mut self, seconds: f64) {
        self.tokens = (self.tokens + seconds.max(0.0) * MESSAGES_PER_SECOND).min(MESSAGE_BURST);
    }

    /// Takes one message's worth, if there is one.
    pub fn take(&mut self) -> bool {
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// What a runtime's scripts sent and the engine hasn't taken yet, shared
/// by its mods' states; and, on a client, the budget its messages are
/// held to.
#[derive(Debug, Default)]
pub struct Outbox(Mutex<OutboxState>);

#[derive(Debug, Default)]
struct OutboxState {
    sent: Vec<Outgoing>,
    budget: MessageBudget,
}

impl Outbox {
    /// What's been sent since the last call, oldest first.
    pub fn take(&self) -> Vec<Outgoing> {
        std::mem::take(&mut self.lock().sent)
    }

    /// `seconds` more have passed, for a client's budget.
    pub fn refill(&self, seconds: f64) {
        self.lock().budget.refill(seconds);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OutboxState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Whether a message from the other side is one this side takes: its
/// channel and its size within the limits. (Its mod is the receiver's to
/// check: a mod with no scripts takes nothing.)
pub fn check(message: &ScriptMessage) -> Result<(), String> {
    if message.channel.len() > MAX_CHANNEL_BYTES {
        return Err(format!(
            "a channel's name is at most {MAX_CHANNEL_BYTES} bytes"
        ));
    }
    let bytes = crate::net::encode(message).len();
    if bytes > MAX_MESSAGE_BYTES {
        return Err(format!(
            "a message is {bytes} bytes, more than the {MAX_MESSAGE_BYTES} it may take"
        ));
    }
    Ok(())
}

/// The `net` global for mod `mod_id`'s scripts on `side`.
pub(super) fn net_table(
    lua: &Lua,
    side: Side,
    mod_id: &str,
    outbox: Arc<Outbox>,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    match side {
        Side::Client => {
            let (mod_id, outbox) = (mod_id.to_string(), outbox.clone());
            table.raw_set(
                "send",
                lua.create_function(move |lua, (channel, value): (String, Value)| {
                    let message = message(lua, &mod_id, channel, value)?;
                    let mut state = outbox.lock();
                    if !state.budget.take() {
                        return Ok(false);
                    }
                    state.sent.push(Outgoing {
                        to: Recipient::Server,
                        message,
                    });
                    Ok(true)
                })?,
            )?;
        }
        Side::Server => {
            let (id, out) = (mod_id.to_string(), outbox.clone());
            table.raw_set(
                "send",
                lua.create_function(
                    move |lua, (player, channel, value): (Value, String, Value)| {
                        let to = Recipient::Player(player_id(&player)?);
                        let message = message(lua, &id, channel, value)?;
                        out.lock().sent.push(Outgoing { to, message });
                        Ok(true)
                    },
                )?,
            )?;
            let id = mod_id.to_string();
            table.raw_set(
                "broadcast",
                lua.create_function(move |lua, (channel, value): (String, Value)| {
                    let message = message(lua, &id, channel, value)?;
                    outbox.lock().sent.push(Outgoing {
                        to: Recipient::Everyone,
                        message,
                    });
                    Ok(true)
                })?,
            )?;
        }
    }
    table.set_readonly(true);
    Ok(table)
}

/// A message from what a script passed `net.send`, counted against the
/// call's [`MESSAGES_PER_CALL`] and checked against the limits.
fn message(lua: &Lua, mod_id: &str, channel: String, value: Value) -> mlua::Result<ScriptMessage> {
    let sent = lua
        .app_data_ref::<Arc<Meter>>()
        .map_or(0, |meter| meter.count_sent());
    if sent >= MESSAGES_PER_CALL {
        return Err(mlua::Error::runtime(format!(
            "`net.send` sent more than {MESSAGES_PER_CALL} messages in one call"
        )));
    }
    let value = match value {
        Value::Nil => None,
        value => Some(from_lua(value, "message", MAX_MESSAGE_BYTES)?),
    };
    let message = ScriptMessage {
        mod_id: mod_id.to_string(),
        channel,
        value,
    };
    check(&message).map_err(mlua::Error::runtime)?;
    Ok(message)
}

/// The id of the player a script names: a player table (its `id`) or the
/// id itself.
fn player_id(player: &Value) -> mlua::Result<u64> {
    let id = match player {
        Value::Table(table) => table.raw_get::<Value>("id")?,
        other => other.clone(),
    };
    match id {
        Value::Integer(n) if n >= 0 => Ok(n as u64),
        Value::Number(n) if n >= 0.0 && n.fract() == 0.0 && n < (1u64 << 53) as f64 => Ok(n as u64),
        _ => Err(mlua::Error::runtime(
            "`net.send` on the server sends to a player: the `player` a hook was handed, or its \
             `id`",
        )),
    }
}

/// A received message's value as a Lua value, for `on_message`.
pub fn value_to_lua(lua: &Lua, value: &Option<block_entity::Value>) -> mlua::Result<Value> {
    match value {
        Some(value) => super::api::to_lua(lua, value),
        None => Ok(Value::Nil),
    }
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use super::*;
    use crate::{
        content::{BlockState, Content, base, block, load},
        light::Light,
        script::{
            CallError, Caller, ErrorKind, ScriptRuntime, ScriptSettings, ScriptSources,
            api::{WorldView, call_read_only},
        },
        world::VoxelFluid,
    };

    const CALLER: Caller = Caller::Other(1);

    /// Mod `m` with script `m:s`, on `side`.
    fn runtime(side: Side, script: &str) -> ScriptRuntime {
        let sources = ScriptSources::from_mods(&load::order(
            vec![load::ModFiles::new(
                "mods/m",
                "{ id: \"m\", name: \"M\" }",
                &[("scripts/s.luau", script)],
            )],
            &mut Vec::new(),
        ));
        let runtime = ScriptRuntime::new(side, sources, ScriptSettings::untimed());
        assert!(
            runtime.load_errors().is_empty(),
            "{:?}",
            runtime.load_errors()
        );
        runtime
    }

    fn failure<R: std::fmt::Debug>(result: Result<Option<R>, CallError>) -> String {
        match result {
            Err(CallError::Failed(error)) => error.to_string(),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn scripts_know_their_side() {
        for side in [Side::Server, Side::Client] {
            let mut runtime = runtime(side, "function on_side() return side end");
            assert_eq!(
                runtime.call_with::<_, String>(CALLER, "m:s", "on_side", ()),
                Ok(Some(side.name().to_string()))
            );
        }
    }

    #[test]
    fn a_client_sends_its_server_messages_within_its_budget() {
        let mut runtime = runtime(
            Side::Client,
            "function on_send(n)\n\
             \tlocal sent = 0\n\
             \tfor i = 1, n do if net.send('ping', { i = i }) then sent += 1 end end\n\
             \treturn sent\n\
             end",
        );
        assert_eq!(
            runtime.call_with::<_, u32>(CALLER, "m:s", "on_send", 2),
            Ok(Some(2))
        );
        let sent = runtime.outbox().take();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].to, Recipient::Server);
        assert_eq!(sent[0].message.mod_id, "m");
        assert_eq!(sent[0].message.channel, "ping");
        assert_eq!(
            sent[1].message.value.as_ref().unwrap().as_table().unwrap()["i"],
            block_entity::Value::Int(2)
        );
        assert!(runtime.outbox().take().is_empty());
        // The rest of the burst, a call at a time; then nothing goes
        // until time passes.
        let mut total = 2;
        for _ in 0..10 {
            total += runtime
                .call_with::<_, u32>(CALLER, "m:s", "on_send", 10)
                .unwrap()
                .unwrap();
        }
        assert_eq!(total, MESSAGE_BURST as u32);
        assert_eq!(runtime.outbox().take().len(), MESSAGE_BURST as usize - 2);
        runtime.outbox().refill(0.5);
        assert_eq!(
            runtime.call_with::<_, u32>(CALLER, "m:s", "on_send", 16),
            Ok(Some((MESSAGES_PER_SECOND / 2.0) as u32))
        );
    }

    #[test]
    fn a_hook_sends_only_so_many_messages_of_limited_size() {
        let mut runtime = runtime(
            Side::Client,
            "function on_flood() for i = 1, 17 do net.send('x') end end\n\
             function on_big() net.send('x', string.rep('a', 5000)) end\n\
             function on_long() net.send(string.rep('c', 65), 1) end\n\
             function on_odd() net.send('x', { f = math.floor }) end",
        );
        let error = failure(runtime.call_with::<_, ()>(CALLER, "m:s", "on_flood", ()));
        assert!(error.contains("more than 16 messages"), "{error}");
        let error = failure(runtime.call_with::<_, ()>(Caller::Other(2), "m:s", "on_big", ()));
        assert!(error.contains("bytes"), "{error}");
        let error = failure(runtime.call_with::<_, ()>(Caller::Other(3), "m:s", "on_long", ()));
        assert!(error.contains("channel"), "{error}");
        let error = failure(runtime.call_with::<_, ()>(Caller::Other(4), "m:s", "on_odd", ()));
        assert!(error.contains("function"), "{error}");
        // What the flood sent before it failed waits, as a failing hook's
        // edits stay made.
        assert_eq!(runtime.outbox().take().len(), 16);
    }

    #[test]
    fn the_server_sends_to_a_player_or_everyone() {
        let mut runtime = runtime(
            Side::Server,
            "function on_reply(player) net.send(player, 'hi', 'there') net.send(7, 'hi') end\n\
             function on_all() net.broadcast('news', { a = { 1, 2 } }) end\n\
             function on_nobody() net.send('hi', 1) end",
        );
        runtime
            .call_scoped::<()>(CALLER, "m:s", "on_reply", |lua, _, hook| {
                let player = lua.create_table()?;
                player.raw_set("id", 42)?;
                hook.call(player)
            })
            .unwrap();
        runtime
            .call_with::<_, ()>(CALLER, "m:s", "on_all", ())
            .unwrap();
        let sent: Vec<_> = runtime.outbox().take();
        let to: Vec<Recipient> = sent.iter().map(|s| s.to).collect();
        assert_eq!(
            to,
            [
                Recipient::Player(42),
                Recipient::Player(7),
                Recipient::Everyone
            ]
        );
        assert_eq!(sent[1].message.value, None);
        let error = failure(runtime.call_with::<_, ()>(Caller::Other(6), "m:s", "on_nobody", ()));
        assert!(error.contains("sends to a player"), "{error}");
    }

    /// A world of stone everywhere, with full sky light.
    struct Stone(Content);

    impl WorldView for Stone {
        fn content(&self) -> &Content {
            &self.0
        }
        fn block(&self, _: IVec3) -> Option<BlockState> {
            Some(block("stone"))
        }
        fn fluid(&self, _: IVec3) -> Option<VoxelFluid> {
            None
        }
        fn light(&self, _: IVec3) -> Option<Light> {
            Some(Light::new(15, [0; 3]))
        }
    }

    #[test]
    fn a_client_reads_the_world_but_cant_change_it() {
        let mut runtime = runtime(
            Side::Client,
            "function on_read(x)\n\
             \tlocal b = world.block(x, 0, 0)\n\
             \treturn b.id, b.light.sky, b.data == nil, b:neighbour('up').id\n\
             end\n\
             function on_write() world.block(0, 0, 0):set('defaria:glass') end\n\
             function on_set_state() world.block(0, 0, 0):set_state({ lit = true }) end",
        );
        let view = Stone(base().clone());
        let read = call_read_only::<_, (String, u8, bool, String)>(
            &mut runtime,
            CALLER,
            "m:s",
            "on_read",
            &view,
            |_| Ok(3),
        );
        assert_eq!(
            read,
            Ok(Some((
                "defaria:stone".into(),
                15,
                true,
                "defaria:stone".into()
            )))
        );
        for (i, hook) in ["on_write", "on_set_state"].into_iter().enumerate() {
            let caller = Caller::Other(10 + i as u64);
            let result =
                call_read_only::<_, ()>(&mut runtime, caller, "m:s", hook, &view, |_| Ok(()));
            let Err(CallError::Failed(error)) = result else {
                panic!("{hook}: {result:?}");
            };
            assert_eq!(error.kind, ErrorKind::Runtime);
            assert!(error.message.contains("read-only on a client"), "{error}");
            assert_eq!(error.line, Some(5 + i as u32), "{error}");
            assert!(runtime.is_disabled(caller));
        }
    }

    #[test]
    fn a_budget_allows_a_burst_then_its_rate() {
        let mut budget = MessageBudget::default();
        let burst = (0..100).take_while(|_| budget.take()).count();
        assert_eq!(burst, MESSAGE_BURST as usize);
        assert!(!budget.take());
        budget.refill(0.5);
        let then = (0..100).take_while(|_| budget.take()).count();
        assert_eq!(then, (MESSAGES_PER_SECOND / 2.0) as usize);
        budget.refill(1000.0);
        assert_eq!(
            (0..100).take_while(|_| budget.take()).count(),
            MESSAGE_BURST as usize
        );
    }

    #[test]
    fn messages_over_the_limits_are_refused() {
        let message = |channel: &str, text: usize| ScriptMessage {
            mod_id: "test".into(),
            channel: channel.into(),
            value: Some(block_entity::Value::Text("x".repeat(text))),
        };
        assert!(check(&message("hello", 100)).is_ok());
        assert!(check(&message(&"c".repeat(65), 1)).is_err());
        assert!(check(&message("hello", 5000)).is_err());
    }
}
