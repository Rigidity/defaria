//! A mod's Luau state: the libraries scripts get, the ones they don't,
//! `require` limited to the mod and its dependencies, `log`, and the meter
//! that stops a call when it runs out of budget.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use bevy::log::{debug, error, info, warn};
use mlua::{Lua, LuaOptions, StdLib, Table, Value, Variadic, VmState};

use super::{
    ScriptSettings,
    source::{ModScripts, ScriptSources},
};
use defaria_core::content;

/// The libraries scripts get. Left out: `os` (time and date), `debug`
/// (reaching into other functions), `io` and `package`, which Luau
/// doesn't have anyway.
fn libraries() -> StdLib {
    StdLib::COROUTINE
        | StdLib::TABLE
        | StdLib::STRING
        | StdLib::UTF8
        | StdLib::BIT
        | StdLib::MATH
        | StdLib::BUFFER
        | StdLib::VECTOR
}

/// Base functions taken away: loading code other than through `require`
/// (`loadstring`, and mlua's own file-system `require`), reaching into
/// other functions' globals (`getfenv`, `setfenv`), the collector
/// (`collectgarbage`, `gcinfo`), and `print`, which `log.info` replaces.
const REMOVED: [&str; 7] = [
    "require",
    "loadstring",
    "getfenv",
    "setfenv",
    "collectgarbage",
    "gcinfo",
    "print",
];

/// Registry tables: the scripts loaded so far, by id (each its globals
/// table, or `false` while it's loading), and what each returned.
const ENVS: &str = "defaria.envs";
const RESULTS: &str = "defaria.results";

/// How often (in steps) [`Meter`] looks at the clock.
const CLOCK_EVERY: u64 = 16;

/// Counts a call's steps down. Luau calls the interrupt at loop back
/// edges and function calls, so a step is about a loop iteration or a
/// call. Once it reaches zero, every step raises an error, so a script
/// can't catch its way past the budget with `pcall`.
///
/// Library functions (`table.sort`, `string.rep`) do any amount of work
/// in one step, so every [`CLOCK_EVERY`] steps it also checks the call
/// hasn't run past its time limit, and stops it the same way if it has.
#[derive(Debug, Default)]
pub(super) struct Meter {
    remaining: AtomicU64,
    budget: AtomicU64,
    overran: AtomicBool,
    /// How long a call may take.
    time_limit: Duration,
    /// When the call started.
    started: Mutex<Option<Instant>>,
    out_of_time: AtomicBool,
    /// The lines logged since [`Meter::start`].
    logged: AtomicU64,
}

impl Meter {
    /// A meter whose calls may each take `time_limit`.
    pub(super) fn new(time_limit: Duration) -> Self {
        Self {
            time_limit,
            ..Self::default()
        }
    }

    pub(super) fn start(&self, budget: u64) {
        self.budget.store(budget, Ordering::Relaxed);
        self.remaining.store(budget, Ordering::Relaxed);
        self.overran.store(false, Ordering::Relaxed);
        self.out_of_time.store(false, Ordering::Relaxed);
        self.logged.store(0, Ordering::Relaxed);
        *self.started.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }

    pub(super) fn overran(&self) -> bool {
        self.overran.load(Ordering::Relaxed)
    }

    /// The steps used since [`Meter::start`].
    pub(super) fn used(&self) -> u64 {
        self.budget.load(Ordering::Relaxed) - self.remaining.load(Ordering::Relaxed)
    }

    /// Whether the call has run past its time limit, looking at the clock
    /// only every [`CLOCK_EVERY`] steps.
    fn late(&self, remaining: u64) -> bool {
        if self.out_of_time.load(Ordering::Relaxed) {
            return true;
        }
        if !remaining.is_multiple_of(CLOCK_EVERY) {
            return false;
        }
        let started = *self.started.lock().unwrap_or_else(|e| e.into_inner());
        let late = started.is_some_and(|started| started.elapsed() > self.time_limit);
        if late {
            self.out_of_time.store(true, Ordering::Relaxed);
        }
        late
    }

    fn step(&self, lua: &Lua) -> mlua::Result<VmState> {
        let remaining = self.remaining.load(Ordering::Relaxed);
        if remaining > 0 && !self.late(remaining) {
            self.remaining.store(remaining - 1, Ordering::Relaxed);
            return Ok(VmState::Continue);
        }
        self.overran.store(true, Ordering::Relaxed);
        let at = location(lua, 0).map_or(String::new(), |(file, line)| format!("{file}:{line}: "));
        Err(mlua::Error::runtime(
            if self.out_of_time.load(Ordering::Relaxed) {
                format!(
                    "{at}ran past its time limit of {} ms",
                    self.time_limit.as_millis()
                )
            } else {
                format!(
                    "{at}ran past its budget of {} steps",
                    self.budget.load(Ordering::Relaxed)
                )
            },
        ))
    }
}

/// The file and line of the script running at stack `level`, or, if a
/// library function is running there (such as `string.find`, or `pcall`
/// between the script and a function of ours), of the innermost script
/// below it: library functions have no file or line of their own.
fn location(lua: &Lua, level: usize) -> Option<(String, usize)> {
    (level..)
        .map_while(|level| {
            lua.inspect_stack(level, |debug| {
                let source = debug.source();
                if source.what == "C" {
                    return None;
                }
                Some((source.short_src?.into_owned(), debug.current_line()?))
            })
        })
        .flatten()
        .next()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// One line a script logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub mod_id: String,
    pub level: LogLevel,
    /// Where it was logged from, when Luau knows.
    pub file: Option<String>,
    pub line: Option<u32>,
    pub message: String,
}

/// How many recent lines [`LogHistory`] keeps.
const LOG_HISTORY: usize = 256;

/// The longest message a line keeps, in bytes; the rest is cut off and
/// counted, so logging a huge string doesn't keep it.
pub(super) const LOG_LINE_MAX: usize = 4096;

/// How many lines one call (or load) may log. Past that, one line says
/// lines were dropped, and the rest are.
pub(super) const LOG_LINES_PER_CALL: u64 = 100;

/// The most recent lines scripts logged, for tests and tools. Everything
/// also goes to the game's log as it's logged.
#[derive(Debug, Default)]
pub struct LogHistory(Mutex<VecDeque<LogLine>>);

impl LogHistory {
    fn push(&self, line: LogLine) {
        let mut lines = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if lines.len() == LOG_HISTORY {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    pub fn lines(&self) -> Vec<LogLine> {
        let lines = self.0.lock().unwrap_or_else(|e| e.into_inner());
        lines.iter().cloned().collect()
    }
}

/// A new, sandboxed state for `scripts`' mod.
pub(super) fn new_state(
    scripts: &ModScripts,
    sources: Arc<ScriptSources>,
    settings: &ScriptSettings,
    meter: Arc<Meter>,
    log: Arc<LogHistory>,
) -> mlua::Result<Lua> {
    let lua = Lua::new_with(libraries(), LuaOptions::new())?;
    let globals = lua.globals();
    for name in REMOVED {
        globals.raw_set(name, Value::Nil)?;
    }
    globals.raw_set("log", log_table(&lua, &scripts.id, log)?)?;
    globals.raw_set("world", super::api::world_table(&lua)?)?;
    let mut allowed = vec![scripts.id.clone()];
    allowed.extend(scripts.dependencies.iter().cloned());
    let mod_id = scripts.id.clone();
    globals.raw_set(
        "require",
        lua.create_function(move |lua, name: String| {
            let id = content::qualify(&name, &mod_id).map_err(mlua::Error::runtime)?;
            let namespace = id.split_once(':').map_or("", |(namespace, _)| namespace);
            if !allowed.iter().any(|a| a == namespace) {
                return Err(mlua::Error::runtime(format!(
                    "can't require `{id}`: a script can only require scripts from its own mod \
                     and the mods it depends on"
                )));
            }
            require(lua, &sources, &id)
        })?,
    )?;
    // The same random numbers every run, so headless runs repeat.
    lua.load("math.randomseed(0)").exec()?;
    lua.set_named_registry_value(ENVS, lua.create_table()?)?;
    lua.set_named_registry_value(RESULTS, lua.create_table()?)?;
    // Globals and the libraries become read-only, and every script gets a
    // globals table of its own (see `load`).
    lua.sandbox(true)?;
    lua.set_memory_limit(settings.memory_limit)?;
    // For `log`, which counts the lines each call logs.
    lua.set_app_data(meter.clone());
    lua.set_interrupt(move |lua| meter.step(lua));
    Ok(lua)
}

/// What script `id` returned, loading it into this state the first time.
fn require(lua: &Lua, sources: &ScriptSources, id: &str) -> mlua::Result<Value> {
    load(lua, sources, id)?;
    let results: Table = lua.named_registry_value(RESULTS)?;
    results.raw_get(id)
}

/// Script `id`'s globals table, running the script the first time it's
/// asked for. Each script has its own: what it defines there (hooks
/// included) other scripts don't see, and it reads the shared, read-only
/// globals (the libraries and `log`) through it.
pub(super) fn load(lua: &Lua, sources: &ScriptSources, id: &str) -> mlua::Result<Table> {
    let envs: Table = lua.named_registry_value(ENVS)?;
    match envs.raw_get::<Value>(id)? {
        Value::Table(env) => return Ok(env),
        Value::Boolean(false) => {
            return Err(mlua::Error::runtime(format!(
                "`{id}` requires itself, through the scripts it requires"
            )));
        }
        _ => {}
    }
    let source = sources
        .get(id)
        .ok_or_else(|| mlua::Error::runtime(format!("there's no script `{id}`")))?;
    let env = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.raw_set("__index", lua.globals())?;
    env.set_metatable(Some(meta))?;
    envs.raw_set(id, false)?;
    let result = lua
        .load(&source.text)
        .set_name(format!("@{}", source.file))
        .set_environment(env.clone())
        .into_function()
        .and_then(|function| function.call::<Value>(()));
    match result {
        Ok(value) => {
            envs.raw_set(id, &env)?;
            let value = if value.is_nil() {
                Value::Boolean(true)
            } else {
                value
            };
            let results: Table = lua.named_registry_value(RESULTS)?;
            results.raw_set(id, value)?;
            Ok(env)
        }
        Err(error) => {
            envs.raw_set(id, Value::Nil)?;
            Err(error)
        }
    }
}

/// `log.debug`, `log.info`, `log.warn` and `log.error`: their arguments,
/// turned into text and joined with spaces, go to the game's log under
/// the mod's name. A message is cut to [`LOG_LINE_MAX`] bytes, and a call
/// logs at most [`LOG_LINES_PER_CALL`] lines.
fn log_table(lua: &Lua, mod_id: &str, history: Arc<LogHistory>) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    for (name, level) in [
        ("debug", LogLevel::Debug),
        ("info", LogLevel::Info),
        ("warn", LogLevel::Warn),
        ("error", LogLevel::Error),
    ] {
        let mod_id = mod_id.to_string();
        let history = history.clone();
        let function = lua.create_function(move |lua, values: Variadic<Value>| {
            let logged = lua
                .app_data_ref::<Arc<Meter>>()
                .map_or(0, |meter| meter.logged.fetch_add(1, Ordering::Relaxed));
            let (level, message) = match logged.cmp(&LOG_LINES_PER_CALL) {
                std::cmp::Ordering::Less => (level, log_message(&values)?),
                std::cmp::Ordering::Equal => (
                    LogLevel::Warn,
                    format!(
                        "logged more than {LOG_LINES_PER_CALL} lines in one call; \
                         the rest of this call's lines are dropped"
                    ),
                ),
                std::cmp::Ordering::Greater => return Ok(()),
            };
            // Level 1: what called this function.
            let at = location(lua, 1);
            let place = at
                .as_ref()
                .map_or(String::new(), |(file, line)| format!(" {file}:{line}"));
            match level {
                LogLevel::Debug => debug!(target: "script", "[{mod_id}{place}] {message}"),
                LogLevel::Info => info!(target: "script", "[{mod_id}{place}] {message}"),
                LogLevel::Warn => warn!(target: "script", "[{mod_id}{place}] {message}"),
                LogLevel::Error => error!(target: "script", "[{mod_id}{place}] {message}"),
            }
            let (file, line) = at.map(|(file, line)| (file, line as u32)).unzip();
            history.push(LogLine {
                mod_id: mod_id.clone(),
                level,
                file,
                line,
                message,
            });
            Ok(())
        })?;
        table.raw_set(name, function)?;
    }
    table.set_readonly(true);
    Ok(table)
}

/// `values` as text, joined with spaces, cut to [`LOG_LINE_MAX`] bytes
/// (with how many were cut). Strings are read in place, so a huge one
/// is never copied whole.
fn log_message(values: &[Value]) -> mlua::Result<String> {
    let mut message = Vec::new();
    let mut total = 0;
    let mut add = |bytes: &[u8]| {
        total += bytes.len();
        let room = LOG_LINE_MAX.saturating_sub(message.len());
        message.extend_from_slice(&bytes[..bytes.len().min(room)]);
    };
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            add(b" ");
        }
        match value {
            Value::String(text) => add(&text.as_bytes()),
            value => add(value.to_string()?.as_bytes()),
        }
    }
    // Cutting may split a character, which decodes as U+FFFD.
    let mut message = String::from_utf8_lossy(&message).into_owned();
    if total > LOG_LINE_MAX {
        message.push_str(&format!("…({total} bytes)"));
    }
    Ok(message)
}
