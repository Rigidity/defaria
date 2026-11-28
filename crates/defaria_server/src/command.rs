//! Server commands: what a player types after `/` in chat
//! ([`ClientMessage::Command`]), the dedicated server's console types, and
//! a headless scenario sends to build its scene. One parser ([`parse`])
//! and one way to run them ([`run`]), wherever they come from: each
//! answers with a [`CommandReply`], sent back to whoever ran it (the
//! console's go in [`Console::replies`]).
//!
//! Commands are allowed by player id: the console and the player hosting
//! a world in the game's own process ([`Host`]) always may, and on a
//! dedicated server those the world's `ops.json5` lists ([`Operators`]),
//! read as the server starts and again when someone not listed tries (so
//! an edit to the file counts without a restart). Anyone may run `list`
//! and `help`. A player who isn't allowed is answered why, and what they
//! send is held to chat's rate ([`chat::Allowance`]).
//!
//! [`ClientMessage::Command`]: defaria_core::net::ClientMessage::Command

pub mod ops;
pub mod parse;
pub mod report;
pub mod scene;
#[cfg(test)]
mod tests;

pub use ops::{Host, OPS_FILE, Operators};
pub use parse::{HELP, NAMES, ServerCommand, parse};

use std::time::Instant;

use bevy::prelude::*;

use crate::{
    chat::{ChatAllowance, broadcast},
    fluid::evaporation::EvaporationSpeed,
    script::random_tick::RandomTicks,
    server::{Connection, ReceiveSystems},
    streaming::SaveNow,
};
use defaria_core::{
    chat::{ChatKind, ChatLine, CommandReply, MAX_MESSAGE, clean_text},
    identity::PlayerId,
    net::{ClientMessage, ServerMessage},
    save::WorldFolder,
};

pub struct CommandPlugin;

impl Plugin for CommandPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Console>()
            .init_resource::<Operators>()
            .add_systems(Startup, load_operators)
            .add_systems(
                PreUpdate,
                run_commands
                    .after(ReceiveSystems)
                    .after(crate::chat::ChatSystems),
            );
    }
}

/// The console's side of commands: lines typed at it to run, and what
/// they answered, for it to show.
#[derive(Resource, Debug, Default)]
pub struct Console {
    pub lines: Vec<String>,
    pub replies: Vec<CommandReply>,
}

/// Who runs a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The server's own console: always allowed, with no player.
    Console,
    /// A player, by their entity.
    Player(Entity),
}

impl Source {
    /// The player who runs it, for a command that acts on them.
    fn player(self, command: &str) -> Result<Entity, String> {
        match self {
            Source::Player(entity) => Ok(entity),
            Source::Console => Err(format!("{command}: the console has no player to act on")),
        }
    }
}

/// Reads the world's operators as the server starts.
fn load_operators(mut commands: Commands, folder: Option<Res<WorldFolder>>) {
    let Some(folder) = folder else {
        return;
    };
    match ops::read(&folder.0) {
        Ok(operators) => commands.insert_resource(operators),
        Err(error) => error!("{error}; nobody may run commands until it's fixed"),
    }
}

/// Runs the commands players sent this frame, and those typed at the
/// console, in order, answering each.
fn run_commands(world: &mut World) {
    let mut connections = world.query::<(Entity, &Connection)>();
    let lines: Vec<(Entity, String)> = connections
        .iter(world)
        .flat_map(|(entity, connection)| {
            connection
                .inbox()
                .iter()
                .filter_map(move |message| match message {
                    ClientMessage::Command(line) => Some((entity, line.clone())),
                    _ => None,
                })
        })
        .collect();
    for (entity, line) in lines {
        let reply = run(world, Source::Player(entity), &line);
        if let Some(mut connection) = world.get_mut::<Connection>(entity) {
            connection.send(ServerMessage::CommandReply(reply));
        }
    }
    let lines = std::mem::take(&mut world.resource_mut::<Console>().lines);
    for line in lines {
        let reply = run(world, Source::Console, &line);
        world.resource_mut::<Console>().replies.push(reply);
    }
}

/// Runs the command `line` (a leading `/` is allowed) as `source`, if
/// they may, and says what came of it.
pub fn run(world: &mut World, source: Source, line: &str) -> CommandReply {
    let line = clean_text(line);
    if line.chars().count() > MAX_MESSAGE {
        return CommandReply::refused(format!(
            "that command is too long: at most {MAX_MESSAGE} characters"
        ));
    }
    let command = match parse(&line) {
        Ok(command) => command,
        Err(error) => {
            // Mistakes cost a player who isn't allowed their allowance
            // too, so nonsense can't be sent as fast as anything.
            if !listed(world, source) && !take_allowance(world, source) {
                return slow_down();
            }
            return CommandReply::refused(error);
        }
    };
    if !listed(world, source) {
        if !take_allowance(world, source) {
            return slow_down();
        }
        if !command.open_to_all() && !allowed_after_rereading(world, source) {
            info!("{} isn't allowed to run `{line}`", who(world, source));
            return CommandReply::refused(
                "you're not allowed to run commands on this server: its operators are \
                 listed in the world's ops.json5 (`list` shows your id)",
            );
        }
    }
    // Logged for a server's operators to see who did what; a game's own
    // (single player, and scenarios building their scene) keeps quiet.
    if world.contains_resource::<Host>() {
        debug!("{} ran `{line}`", who(world, source));
    } else {
        info!("{} ran `{line}`", who(world, source));
    }
    execute(world, source, command).unwrap_or_else(CommandReply::refused)
}

fn slow_down() -> CommandReply {
    CommandReply::refused("you're sending too fast; wait a moment")
}

/// Who `source` is, for the log.
fn who(world: &World, source: Source) -> String {
    match source {
        Source::Console => "the console".into(),
        Source::Player(entity) => world
            .get::<Name>(entity)
            .map_or("a player".into(), |name| name.to_string()),
    }
}

/// Whether `source` may run any command, as far as the server knows
/// without reading the operators again.
fn listed(world: &World, source: Source) -> bool {
    let Source::Player(entity) = source else {
        return true;
    };
    let Some(id) = world.get::<PlayerId>(entity) else {
        return false;
    };
    world
        .get_resource::<Host>()
        .is_some_and(|host| host.0 == *id)
        || world.resource::<Operators>().0.contains(id)
}

/// Whether `source` may run any command, reading the world's operators
/// again in case the file changed.
fn allowed_after_rereading(world: &mut World, source: Source) -> bool {
    if let Some(folder) = world.get_resource::<WorldFolder>() {
        match ops::read(&folder.0) {
            Ok(operators) => world.insert_resource(operators),
            Err(error) => warn!("{error}"),
        }
    }
    listed(world, source)
}

/// Takes one message's worth of the player's allowance, if they have it.
fn take_allowance(world: &mut World, source: Source) -> bool {
    let Source::Player(entity) = source else {
        return true;
    };
    world
        .get_mut::<ChatAllowance>(entity)
        .is_none_or(|mut allowance| allowance.0.take())
}

/// Runs an allowed command.
fn execute(
    world: &mut World,
    source: Source,
    command: ServerCommand,
) -> Result<CommandReply, String> {
    let text = match command {
        ServerCommand::Fill { min, max, block } => {
            let start = Instant::now();
            let set = world
                .run_system_cached_with(scene::fill, (min, max, block))
                .map_err(|error| error.to_string())??;
            let took = start.elapsed();
            return Ok(CommandReply {
                filled: Some((set as u64, took.as_nanos() as u64)),
                ..CommandReply::ok(format!("filled {set} voxels"))
            });
        }
        ServerCommand::Pour { pos, fluid, amount } => {
            let text = format!(
                "poured {amount} of {fluid} at {} {} {}",
                pos.x, pos.y, pos.z
            );
            world
                .run_system_cached_with(scene::pour, (pos, fluid, amount))
                .map_err(|error| error.to_string())??;
            text
        }
        ServerCommand::Put {
            pos,
            name,
            amount,
            what,
        } => {
            let text = format!("put {amount} of {what} in {name}");
            world
                .run_system_cached_with(scene::put, (pos, name, amount, what))
                .map_err(|error| error.to_string())??;
            text
        }
        ServerCommand::Hotbar { slot, item } => {
            let player = source.player("hotbar")?;
            let text = format!("holding {item}");
            world
                .run_system_cached_with(scene::hold_item, (player, slot, item))
                .map_err(|error| error.to_string())??;
            text
        }
        ServerCommand::Give { item, count, fluid } => {
            let player = source.player("give")?;
            let left = world
                .run_system_cached_with(scene::give_items, (player, item.clone(), count, fluid))
                .map_err(|error| error.to_string())??;
            if left > 0 {
                warn!("give: {left} of `{item}` didn't fit");
                format!("gave {} of {item}; {left} didn't fit", count - left)
            } else {
                format!("gave {count} of {item}")
            }
        }
        ServerCommand::Teleport(eyes) => {
            let player = source.player("tp")?;
            world
                .run_system_cached_with(scene::teleport, (player, eyes))
                .map_err(|error| error.to_string())??;
            format!("teleported to {} {} {}", eyes.x, eyes.y, eyes.z)
        }
        ServerCommand::RandomTicks(per_chunk) => {
            world
                .get_resource_mut::<RandomTicks>()
                .ok_or("this server has no random ticks")?
                .per_chunk = per_chunk;
            format!("{per_chunk} random ticks a chunk each tick")
        }
        ServerCommand::Evaporation(speed) => {
            world
                .get_resource_mut::<EvaporationSpeed>()
                .ok_or("this server has no evaporation")?
                .0 = speed;
            format!("films evaporate {speed} times as fast")
        }
        ServerCommand::Fluids(region) => {
            let region = match region {
                Some((min, max)) => report::FluidRegion::Box { min, max },
                None => report::FluidRegion::Window(match source {
                    Source::Player(entity) => Some(entity),
                    Source::Console => None,
                }),
            };
            report::fluid_report(world, region)
        }
        ServerCommand::Network(pos) => report::network_report(world, pos),
        ServerCommand::Save => {
            *world.resource_mut::<SaveNow>() = SaveNow {
                chunks: true,
                players: true,
            };
            "saving the world".into()
        }
        ServerCommand::Stop => {
            world.write_message(AppExit::Success);
            "stopping the server".into()
        }
        ServerCommand::List => list(world, source),
        ServerCommand::Help => HELP.into(),
        ServerCommand::Say(text) => {
            let from = match source {
                Source::Console => "Server".into(),
                Source::Player(_) => who(world, source),
            };
            broadcast(
                world,
                ChatLine {
                    kind: ChatKind::Said,
                    from: Some(from),
                    text,
                },
            );
            String::new()
        }
        ServerCommand::Op(player) => set_operator(world, &player, true)?,
        ServerCommand::Deop(player) => set_operator(world, &player, false)?,
    };
    Ok(CommandReply::ok(text))
}

/// The players playing, by name; with their ids for those who may run
/// commands (whose job it is to list operators by id) and for the asker.
fn list(world: &mut World, source: Source) -> String {
    let full = listed(world, source);
    let asker = match source {
        Source::Player(entity) => Some(entity),
        Source::Console => None,
    };
    let mut players: Vec<String> = world
        .query_filtered::<(Entity, &Name, Option<&PlayerId>), With<Connection>>()
        .iter(world)
        .map(|(entity, name, id)| match id {
            Some(id) if full || asker == Some(entity) => format!("{name} ({id})"),
            _ => name.to_string(),
        })
        .collect();
    players.sort();
    match players.len() {
        0 => "nobody's playing".into(),
        1 => format!("1 player: {}", players[0]),
        n => format!("{n} players: {}", players.join(", ")),
    }
}

/// Lets `who` (an online player's name, or an id) run commands, or stops
/// letting them, saving the list in the world's `ops.json5`.
fn set_operator(world: &mut World, who: &str, allow: bool) -> Result<String, String> {
    let named = world
        .query_filtered::<(&Name, &PlayerId), With<Connection>>()
        .iter(world)
        .find(|(name, _)| name.as_str().eq_ignore_ascii_case(who))
        .map(|(name, id)| (name.to_string(), *id));
    let (name, id) = match named {
        Some(found) => found,
        None => {
            let id: PlayerId = who
                .parse()
                .map_err(|_| format!("no player called `{who}` is playing, and it isn't an id"))?;
            (id.to_string(), id)
        }
    };
    let mut operators = world.resource::<Operators>().clone();
    let changed = if allow {
        operators.0.insert(id)
    } else {
        operators.0.remove(&id)
    };
    if let Some(folder) = world.get_resource::<WorldFolder>() {
        ops::write(&folder.0, &operators)?;
    }
    world.insert_resource(operators);
    Ok(match (allow, changed) {
        (true, true) => format!("{name} may run commands now"),
        (true, false) => format!("{name} could already run commands"),
        (false, true) => format!("{name} may no longer run commands"),
        (false, false) => format!("{name} wasn't an operator"),
    })
}
