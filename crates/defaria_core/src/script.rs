//! Mod scripts, in Luau: the runtime both sides run them in. Loading
//! them, running them in a sandbox with a budget, reporting their
//! errors, what every hook can read of the world, and the channel a
//! mod's scripts on a client and on its server talk through. The format
//! and the rules for modders are in `docs/scripting.md`; why Luau is in
//! `docs/decisions.md`.
//!
//! Every mod's `scripts/` folder is loaded into a [`ScriptRuntime`] on
//! each side, the server's and a client's, alike: one sandboxed Luau
//! state per mod, so mods share no globals and each has a memory cap, and
//! a globals table per script within it. A script defines hooks, global
//! functions named `on_*`, that the engine calls through
//! [`ScriptRuntime::call`] with engine state passed in as userdata.
//!
//! Each hook belongs to a [`Side`]: the gameplay hooks (`on_used`,
//! `on_tick`, ...) are called only by the server's systems
//! (`defaria_server::script`), the client's (`on_join`) only by the
//! client's (`defaria_client::script`), and `on_message` by both, for
//! what the other side sent ([`channel`]). [`api`] is what both sides
//! hand hooks to read the world with (block handles, the `world`
//! global); each side adds its own writes, and a client has none.

pub mod api;
pub mod channel;
mod error;
#[cfg(feature = "dev")]
mod reload;
mod runtime;
mod sandbox;
pub mod source;

use std::time::Duration;

use bevy::prelude::*;

use crate::content::{self, Content};
pub use error::{CallError, ErrorKind, ScriptError};
pub use runtime::{Caller, ScriptRuntime};
pub use sandbox::{LogLevel, LogLine};
pub use source::ScriptSources;

/// Which app a runtime is in, which decides the hooks it calls and what
/// its scripts may do: the server's, which decides the world, or a
/// client's, which only reads its copy of it. Scripts read it as the
/// `side` global (`"server"` or `"client"`).
#[derive(Resource, Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[reflect(Resource)]
pub enum Side {
    Server,
    Client,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::Server => "server",
            Side::Client => "client",
        }
    }
}

/// The limits scripts run under. Changing them reloads the scripts.
#[derive(Resource, Reflect, Debug, Clone, PartialEq)]
#[reflect(Resource)]
pub struct ScriptSettings {
    /// The steps one hook call (or one script's top level, as it loads)
    /// may take. A step is about one loop iteration or function call, so
    /// the default is roughly a millisecond of work.
    pub step_budget: u64,
    /// The wall-clock time one hook call (or load) may take, a safety net
    /// beside the steps: library functions such as `table.sort` or
    /// `string.rep` do a lot of work in one step, so steps alone don't
    /// bound time. Checked every few steps, so a call can overrun it by
    /// a few library calls. Unlike the steps it isn't deterministic.
    pub time_limit: Duration,
    /// The memory each mod's scripts may use between them, in bytes.
    pub memory_limit: usize,
}

impl Default for ScriptSettings {
    fn default() -> Self {
        Self {
            step_budget: 100_000,
            time_limit: Duration::from_millis(20),
            memory_limit: 32 << 20,
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ScriptSettings {
    /// The defaults with no time limit, for tests: what runs out runs
    /// out of steps, the same every run however busy the machine is.
    pub fn untimed() -> Self {
        Self {
            time_limit: Duration::MAX,
            ..default()
        }
    }
}

/// Sent when the scripts have reloaded (a script changed in a dev build,
/// or the settings did). Every script's state is fresh and every
/// disabled caller enabled again.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptsReloaded {
    pub generation: u64,
}

/// The runtime for one side: a [`ScriptRuntime`] for `side`, loaded from
/// the mods the content loaded, reloaded when its [`ScriptSettings`]
/// change (and in dev builds when a script file does). Each side's own
/// script plugin adds it and calls its hooks.
pub struct ScriptRuntimePlugin {
    pub side: Side,
}

impl Plugin for ScriptRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ScriptSettings>()
            .register_type::<Side>()
            .insert_resource(self.side)
            .init_resource::<ScriptSettings>()
            .add_message::<ScriptsReloaded>()
            .init_resource::<ScriptRuntime>()
            // On the first frame too, which does nothing: the runtime
            // loaded with these settings.
            .add_systems(
                Update,
                apply_settings.run_if(resource_changed::<ScriptSettings>),
            );
        #[cfg(feature = "dev")]
        reload::plugin(app);
    }
}

impl FromWorld for ScriptRuntime {
    /// Loads the scripts of the mods the content loaded, from disk, for
    /// the world's [`Side`] (the server's, if it has none).
    fn from_world(world: &mut World) -> Self {
        let settings = world.get_resource_or_init::<ScriptSettings>().clone();
        let side = world
            .get_resource::<Side>()
            .copied()
            .unwrap_or(Side::Server);
        let mods: Vec<String> = world
            .get_resource::<Content>()
            .map(|content| content.mods.iter().map(|m| m.id.clone()).collect())
            .unwrap_or_default();
        let root = content::mods_dir();
        let sources = ScriptSources::read(&root, &mods);
        for error in &sources.errors {
            error!("{error}");
        }
        let runtime = Self::new(side, sources, settings);
        info!(
            "loaded {} scripts on the {} ({} failed)",
            runtime.sources().sources.len(),
            side.name(),
            runtime.load_errors().len()
        );
        runtime
    }
}

fn apply_settings(
    settings: Res<ScriptSettings>,
    mut runtime: ResMut<ScriptRuntime>,
    mut reloaded: MessageWriter<ScriptsReloaded>,
) {
    let generation = runtime.generation();
    runtime.set_settings(settings.clone());
    if runtime.generation() != generation {
        reloaded.write(ScriptsReloaded {
            generation: runtime.generation(),
        });
    }
}

#[cfg(test)]
mod tests;
