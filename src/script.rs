//! Mod scripts, in Luau: loading them, running them in a sandbox with a
//! budget, and reporting their errors. The format and the rules for
//! modders are in `docs/scripting.md`; why Luau is in `docs/decisions.md`.
//!
//! Every mod's `scripts/` folder is loaded at startup into a
//! [`ScriptRuntime`]: one sandboxed Luau state per mod, so mods share no
//! globals and each has a memory cap, and a globals table per script
//! within it. A script defines hooks, global functions named `on_*`,
//! that the engine calls through [`ScriptRuntime::call`] with engine state
//! passed in as userdata. [`api`] is what hooks are handed (block handles,
//! the `world` global, players, items and blocks' `data`), and
//! [`ScriptWorld::call`] calls a hook with it; [`hooks`] calls blocks' and
//! items' scripts as things happen to them, [`random_tick`] gives
//! blocks random ticks, and [`machine`] calls machines' `on_tick` and
//! `on_recipe_finished`.
//!
//! Scripts only ever change game state through what they're handed, never
//! rendering or UI, so they can run on a server once the game is split.
//! Calls into them belong in `FixedUpdate`, with the simulation.

pub mod api;
mod error;
pub mod hooks;
pub mod machine;
pub mod random_tick;
#[cfg(feature = "dev")]
mod reload;
mod runtime;
mod sandbox;
pub mod source;
pub(crate) mod using;

use std::time::Duration;

use bevy::{asset::io::file::FileAssetReader, prelude::*};

use crate::content::{self, Content};
pub use api::ScriptWorld;
pub use error::{CallError, ErrorKind, ScriptError};
pub use hooks::{BlockUsed, HookSystems, ItemUsed, ScriptIndex};
pub use runtime::{Caller, ScriptRuntime};
pub use sandbox::{LogLevel, LogLine};
pub use source::ScriptSources;

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

/// Sent when the scripts have reloaded (a script changed in a dev build,
/// or the settings did). Every script's state is fresh and every
/// disabled caller enabled again.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptsReloaded {
    pub generation: u64,
}

pub struct ScriptPlugin;

impl Plugin for ScriptPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ScriptSettings>()
            .init_resource::<ScriptSettings>()
            .add_message::<ScriptsReloaded>()
            .init_resource::<ScriptRuntime>()
            // On the first frame too, which does nothing: the runtime
            // loaded with these settings.
            .add_systems(
                Update,
                apply_settings.run_if(resource_changed::<ScriptSettings>),
            );
        hooks::plugin(app);
        using::plugin(app);
        #[cfg(feature = "dev")]
        reload::plugin(app);
    }
}

impl FromWorld for ScriptRuntime {
    /// Loads the scripts of the mods the content loaded, from disk.
    fn from_world(world: &mut World) -> Self {
        let settings = world.get_resource_or_init::<ScriptSettings>().clone();
        let mods: Vec<String> = world
            .get_resource::<Content>()
            .map(|content| content.mods.iter().map(|m| m.id.clone()).collect())
            .unwrap_or_default();
        let root = FileAssetReader::get_base_path()
            .join("assets")
            .join(content::MODS_DIR);
        let sources = ScriptSources::read(&root, &mods);
        for error in &sources.errors {
            error!("{error}");
        }
        let runtime = Self::new(sources, settings);
        info!(
            "loaded {} scripts ({} failed)",
            runtime.sources().sources.len(),
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
