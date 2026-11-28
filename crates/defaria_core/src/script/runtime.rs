//! [`ScriptRuntime`]: every mod's scripts, loaded, and calls into their
//! hooks with budgets, error reporting and disabling what failed.

use std::sync::Arc;

use bevy::{
    log::error,
    platform::collections::{HashMap, HashSet},
    prelude::*,
};
use mlua::{FromLuaMulti, Function, IntoLuaMulti, Lua, Scope, Value};

use super::{
    ScriptSettings, Side,
    channel::Outbox,
    error::{self, CallError, ErrorKind, ScriptError},
    sandbox::{self, LogHistory, LogLine, Meter, Shared},
    source::ScriptSources,
};

/// Whatever calls a hook: what gets disabled when the call fails. Each
/// caller is disabled on its own, so one machine's broken script stops
/// that machine and no other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Caller {
    Entity(Entity),
    /// A block with no entity of its own.
    Block(IVec3),
    /// A kind of item, by its index in [`Content::items`](crate::content::Content::items).
    Item(usize),
    /// One script's hook called for something other than a block or an
    /// item (a message, a client's `on_join`), for one player or none:
    /// see [`Caller::script`].
    Script {
        script: u64,
        player: Option<Entity>,
    },
    /// Anything else, such as a test.
    Other(u64),
}

impl Caller {
    /// Script `id`'s hooks for `player` (or for no one): disabled on their
    /// own when they fail, so one player's bad message stops that
    /// script's handling of that player's messages, and nothing else.
    pub fn script(id: &str, player: Option<Entity>) -> Self {
        // FNV-1a: the same for the same id, run after run.
        let script = id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
            (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3)
        });
        Caller::Script { script, player }
    }
}

/// One mod's state.
struct ModRuntime {
    id: String,
    lua: Lua,
    meter: Arc<Meter>,
}

/// One of a mod's own scripts.
struct Script {
    /// Index into [`ScriptRuntime::mods`].
    mod_index: usize,
    /// Its hooks, or why it didn't load.
    hooks: Result<HashMap<String, Function>, ScriptError>,
}

/// Every mod's scripts, each mod in a sandboxed Luau state of its own
/// (so mods never share globals, and each has its own memory cap), and
/// each script with globals of its own within it.
///
/// A script's hooks are the global functions it defines whose names
/// start with `on_`. Call them with [`ScriptRuntime::call`], passing the
/// engine's state as userdata made in the call's [`Scope`], so a hook can
/// reach borrowed engine state only for as long as the call lasts. Each
/// call gets [`ScriptSettings::step_budget`] steps. A call that errors or
/// runs out is logged once, naming the mod, file and line, and its
/// [`Caller`] is disabled until the scripts reload.
#[derive(Resource)]
pub struct ScriptRuntime {
    side: Side,
    sources: Arc<ScriptSources>,
    settings: ScriptSettings,
    mods: Vec<ModRuntime>,
    scripts: HashMap<String, Script>,
    load_errors: Vec<ScriptError>,
    disabled: HashSet<Caller>,
    log: Arc<LogHistory>,
    /// What the scripts sent (`net.send`), until the engine takes it.
    outbox: Arc<Outbox>,
    generation: u64,
    /// The steps every hook call has used, since the scripts loaded.
    spent: u64,
}

impl ScriptRuntime {
    /// Loads every mod's scripts: runs each script's top level, within
    /// the budget, to define its hooks. Scripts that fail are logged and
    /// kept in [`ScriptRuntime::load_errors`].
    ///
    /// `side` decides what the scripts are given: the `side` global, and
    /// the `net` functions of that side ([`super::channel`]).
    pub fn new(side: Side, sources: ScriptSources, settings: ScriptSettings) -> Self {
        let shared = Shared {
            side,
            log: Arc::default(),
            outbox: Arc::default(),
        };
        Self::build(Arc::new(sources), settings, shared, 0)
    }

    fn build(
        sources: Arc<ScriptSources>,
        settings: ScriptSettings,
        shared: Shared,
        generation: u64,
    ) -> Self {
        let mut runtime = Self {
            side: shared.side,
            sources: sources.clone(),
            settings,
            mods: Vec::new(),
            scripts: HashMap::default(),
            load_errors: Vec::new(),
            disabled: HashSet::default(),
            log: shared.log.clone(),
            outbox: shared.outbox.clone(),
            generation,
            spent: 0,
        };
        for scripts in &sources.mods {
            let meter = Arc::new(Meter::new(runtime.settings.time_limit));
            let lua = match sandbox::new_state(
                scripts,
                sources.clone(),
                &runtime.settings,
                meter.clone(),
                &shared,
            ) {
                Ok(lua) => lua,
                Err(error) => {
                    // Only if Luau itself can't start; nothing a mod did.
                    error!("mod `{}`: can't start its scripts: {error}", scripts.id);
                    continue;
                }
            };
            let mod_index = runtime.mods.len();
            for id in &scripts.scripts {
                meter.start(runtime.settings.step_budget);
                let hooks = sandbox::load(&lua, &sources, id)
                    .and_then(hooks_of)
                    .map_err(|error| {
                        script_error(&scripts.id, id, None, &error, meter.overran(), &runtime)
                    });
                if let Err(error) = &hooks {
                    error!("{error}");
                    runtime.load_errors.push(error.clone());
                }
                runtime
                    .scripts
                    .insert(id.clone(), Script { mod_index, hooks });
            }
            runtime.mods.push(ModRuntime {
                id: scripts.id.clone(),
                lua,
                meter,
            });
        }
        runtime
    }

    /// Loads everything again from `sources`, or from the same sources
    /// (with the current settings) if it's `None`. Scripts start afresh,
    /// every disabled caller is enabled, and the generation goes up.
    pub fn reload(&mut self, sources: Option<ScriptSources>) {
        let sources = sources.map_or_else(|| self.sources.clone(), Arc::new);
        let shared = Shared {
            side: self.side,
            log: self.log.clone(),
            outbox: self.outbox.clone(),
        };
        *self = Self::build(sources, self.settings.clone(), shared, self.generation + 1);
    }

    /// Which side's runtime it is.
    pub fn side(&self) -> Side {
        self.side
    }

    /// What the scripts sent with `net.send` (and `net.broadcast`), and
    /// the budget a client's are held to: each side's systems take what's
    /// waiting once a frame.
    pub fn outbox(&self) -> &Outbox {
        &self.outbox
    }

    /// Mod `mod_id`'s scripts that loaded and define `hook`, in id order.
    pub fn scripts_with_hook(&self, mod_id: &str, hook: &str) -> Vec<&str> {
        let Some(m) = self.sources.mods.iter().find(|m| m.id == mod_id) else {
            return Vec::new();
        };
        let mut ids: Vec<&str> = (m.scripts.iter())
            .filter(|id| self.has_hook(id, hook))
            .map(String::as_str)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Changes the settings, reloading if they're different.
    pub fn set_settings(&mut self, settings: ScriptSettings) {
        if settings != self.settings {
            self.settings = settings;
            self.reload(None);
        }
    }

    pub fn settings(&self) -> &ScriptSettings {
        &self.settings
    }

    pub fn sources(&self) -> &ScriptSources {
        &self.sources
    }

    /// Goes up by one every time the scripts reload, so callers can tell
    /// their scripts' state was reset.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The scripts that didn't load this generation.
    pub fn load_errors(&self) -> &[ScriptError] {
        &self.load_errors
    }

    /// Whether script `id` exists and loaded.
    pub fn is_loaded(&self, id: &str) -> bool {
        self.scripts.get(id).is_some_and(|s| s.hooks.is_ok())
    }

    /// Whether script `id` loaded and defines `hook`. Calling a hook that
    /// isn't defined costs nothing, but callers with a lot to set up for a
    /// call can ask first.
    pub fn has_hook(&self, id: &str, hook: &str) -> bool {
        self.scripts
            .get(id)
            .and_then(|s| s.hooks.as_ref().ok())
            .is_some_and(|hooks| hooks.contains_key(hook))
    }

    /// Whether `caller` is disabled by an earlier error.
    pub fn is_disabled(&self, caller: Caller) -> bool {
        self.disabled.contains(&caller)
    }

    /// Enables `caller` again after an error disabled it, such as a block
    /// position when another block is placed there.
    pub fn enable(&mut self, caller: Caller) {
        self.disabled.remove(&caller);
    }

    /// Forgets the scripts disabled for `player` (its
    /// [`Caller::script`]s), once it has gone, so what a player's errors
    /// disabled doesn't outlive it.
    pub fn forget_player(&mut self, player: Entity) {
        self.disabled.retain(
            |caller| !matches!(caller, Caller::Script { player: Some(p), .. } if *p == player),
        );
    }

    /// The most recent lines the scripts logged, oldest first.
    pub fn log_lines(&self) -> Vec<LogLine> {
        self.log.lines()
    }

    /// The memory mod `mod_id`'s scripts use, in bytes.
    pub fn memory_used(&self, mod_id: &str) -> Option<usize> {
        let m = self.mods.iter().find(|m| m.id == mod_id)?;
        Some(m.lua.used_memory())
    }

    /// The steps the last call (or load) in mod `mod_id` used.
    pub fn steps_used(&self, mod_id: &str) -> Option<u64> {
        Some(self.mods.iter().find(|m| m.id == mod_id)?.meter.used())
    }

    /// The steps every hook call has used since the scripts last loaded,
    /// so callers can tell what a run of calls cost:
    /// `runtime.steps_spent() - before`. A call that ran out (of steps
    /// or time) counts as its whole budget.
    pub fn steps_spent(&self) -> u64 {
        self.spent
    }

    /// Calls `hook` of script `id` for `caller`, with the arguments `args`
    /// makes. `args` gets the call's [`Scope`], in which it can wrap
    /// borrowed engine state as userdata
    /// (`scope.create_userdata(MachineHandle { .. })`, or
    /// `scope.create_userdata_ref_mut(&mut tank)` for a `'static` type);
    /// it stops working when the call returns, so a script can't keep it.
    ///
    /// `Ok(None)` if the script doesn't define the hook; `Ok(Some(r))`
    /// with what it returned if it does.
    pub fn call<'env, A, R>(
        &mut self,
        caller: Caller,
        id: &str,
        hook: &str,
        args: impl for<'scope> FnOnce(&'scope Scope<'scope, 'env>) -> mlua::Result<A>,
    ) -> Result<Option<R>, CallError>
    where
        A: IntoLuaMulti,
        R: FromLuaMulti,
    {
        self.call_scoped(caller, id, hook, |_, scope, function| {
            function.call::<R>(args(scope)?)
        })
    }

    /// [`ScriptRuntime::call`] for callers that set more up around the
    /// call: `call` gets the mod's state, the call's [`Scope`] and the
    /// hook, and calls it itself, within the budget. What it returns is
    /// the call's result; an error from it fails the call as the hook's
    /// own would. (`ScriptWorld::call` puts the world in the registry this
    /// way, and saves blocks' data after the hook returns.)
    pub fn call_scoped<'env, R>(
        &mut self,
        caller: Caller,
        id: &str,
        hook: &str,
        call: impl for<'scope> FnOnce(&Lua, &'scope Scope<'scope, 'env>, &Function) -> mlua::Result<R>,
    ) -> Result<Option<R>, CallError> {
        if self.disabled.contains(&caller) {
            return Err(CallError::Disabled);
        }
        let Some(script) = self.scripts.get(id) else {
            let mod_id = id.split_once(':').map_or("", |(m, _)| m);
            let error = ScriptError {
                mod_id: mod_id.into(),
                script: id.into(),
                hook: Some(hook.into()),
                kind: ErrorKind::Missing,
                file: None,
                line: None,
                message: "there's no such script".into(),
            };
            error!("{error}");
            self.disabled.insert(caller);
            return Err(CallError::Failed(Box::new(error)));
        };
        let hooks = match &script.hooks {
            Ok(hooks) => hooks,
            Err(error) => {
                // Logged when it failed to load.
                self.disabled.insert(caller);
                return Err(CallError::Failed(Box::new(error.clone())));
            }
        };
        let Some(function) = hooks.get(hook) else {
            return Ok(None);
        };
        let state = &self.mods[script.mod_index];
        state.meter.start(self.settings.step_budget);
        let result = state.lua.scope(|scope| call(&state.lua, scope, function));
        // A call stopped for running out is charged its whole budget,
        // even if the clock stopped it after a few slow steps.
        self.spent += if state.meter.overran() {
            self.settings.step_budget
        } else {
            state.meter.used()
        };
        match result {
            Ok(value) => Ok(Some(value)),
            Err(error) => {
                let error = script_error(
                    &state.id,
                    id,
                    Some(hook),
                    &error,
                    state.meter.overran(),
                    self,
                );
                error!("{error}");
                self.disabled.insert(caller);
                Err(CallError::Failed(Box::new(error)))
            }
        }
    }

    /// [`ScriptRuntime::call`] with arguments that need no scope: plain
    /// values, or userdata the engine owns.
    pub fn call_with<A, R>(
        &mut self,
        caller: Caller,
        id: &str,
        hook: &str,
        args: A,
    ) -> Result<Option<R>, CallError>
    where
        A: IntoLuaMulti,
        R: FromLuaMulti,
    {
        self.call(caller, id, hook, |_| Ok(args))
    }
}

/// The hook functions in a script's globals.
fn hooks_of(env: mlua::Table) -> mlua::Result<HashMap<String, Function>> {
    let mut hooks = HashMap::default();
    for pair in env.pairs::<Value, Value>() {
        if let (Value::String(name), Value::Function(function)) = pair? {
            let name = name.to_str()?.to_string();
            if name.starts_with("on_") {
                hooks.insert(name, function);
            }
        }
    }
    Ok(hooks)
}

fn script_error(
    mod_id: &str,
    id: &str,
    hook: Option<&str>,
    error: &mlua::Error,
    overran: bool,
    runtime: &ScriptRuntime,
) -> ScriptError {
    let (mut kind, text) = error::describe(error);
    let mut message = error::strip(&text);
    error::cut(&mut message, sandbox::LOG_LINE_MAX);
    if overran {
        kind = ErrorKind::Budget;
    }
    if kind == ErrorKind::Memory {
        message = format!(
            "used more than the mod's memory cap of {} KiB ({message})",
            runtime.settings.memory_limit / 1024
        );
    }
    let (file, line) = error::locate(&text).unzip();
    ScriptError {
        mod_id: mod_id.into(),
        script: id.into(),
        hook: hook.map(Into::into),
        kind,
        file,
        line,
        message,
    }
}
