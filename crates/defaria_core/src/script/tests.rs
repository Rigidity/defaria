//! The runtime against the test mods in this crate's
//! `tests/fixtures/script-mods`.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use mlua::{UserData, UserDataMethods};

use super::*;

/// The fixture mods' scripts, read from disk as the game reads its own.
pub(super) fn fixture() -> ScriptSources {
    let sources = ScriptSources::read(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/script-mods"),
        &["scripted".into(), "other".into(), "stranger".into()],
    );
    assert!(sources.errors.is_empty(), "{:?}", sources.errors);
    sources
}

/// The fixture runtime, with no time limit, so what runs out runs out
/// of steps, the same every run.
fn runtime() -> ScriptRuntime {
    ScriptRuntime::new(Side::Server, fixture(), ScriptSettings::untimed())
}

const CALLER: Caller = Caller::Other(1);

fn failure(result: Result<Option<()>, CallError>) -> ScriptError {
    match result {
        Err(CallError::Failed(error)) => *error,
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn loads_every_mods_scripts() {
    let runtime = runtime();
    for id in [
        "scripted:answer",
        "scripted:counter",
        "scripted:util",
        "scripted:uses_util",
        "other:peek",
        "stranger:snoop",
    ] {
        assert!(runtime.is_loaded(id), "{id}");
    }
    assert!(runtime.has_hook("scripted:counter", "on_tick"));
    assert!(!runtime.has_hook("scripted:counter", "on_placed"));
    // A module's non-hook functions aren't hooks.
    assert!(!runtime.has_hook("scripted:util", "double"));
    // Only the broken script failed.
    let failed: Vec<_> = runtime.load_errors().iter().map(|e| &e.script).collect();
    assert_eq!(failed, ["scripted:broken"]);
}

#[test]
fn hooks_return_values_and_keep_script_state() {
    let mut runtime = runtime();
    for expected in 1..=3 {
        let count = runtime.call_with::<_, i32>(CALLER, "scripted:counter", "on_tick", 0.05);
        assert_eq!(count, Ok(Some(expected)));
    }
    assert_eq!(
        runtime.call_with::<_, i32>(CALLER, "scripted:uses_util", "on_double", 4),
        Ok(Some(8))
    );
}

#[test]
fn a_missing_hook_costs_nothing_and_does_nothing() {
    let mut runtime = runtime();
    assert_eq!(
        runtime.call_with::<_, ()>(CALLER, "scripted:answer", "on_placed", ()),
        Ok(None)
    );
    assert!(!runtime.is_disabled(CALLER));
}

#[test]
fn log_lines_name_the_mod_file_and_line() {
    let mut runtime = runtime();
    runtime
        .call_with::<_, i32>(CALLER, "scripted:counter", "on_tick", 0.5)
        .unwrap();
    let line = runtime.log_lines().pop().unwrap();
    assert_eq!(line.mod_id, "scripted");
    assert_eq!(line.level, LogLevel::Info);
    assert_eq!(line.message, "tick 1 0.5");
    assert_eq!(
        line.file.as_deref(),
        Some("mods/scripted/scripts/counter.luau")
    );
    assert_eq!(line.line, Some(6));
}

#[test]
fn log_lines_through_a_library_function_name_the_scripts_line() {
    let mut runtime = runtime();
    runtime
        .call_with::<_, ()>(CALLER, "scripted:counter", "on_log_caught", ())
        .unwrap();
    let line = runtime.log_lines().pop().unwrap();
    assert_eq!(line.message, "caught");
    assert_eq!(
        line.file.as_deref(),
        Some("mods/scripted/scripts/counter.luau")
    );
    assert_eq!(line.line, Some(12));
}

#[test]
fn logging_huge_strings_keeps_the_history_small() {
    let mut runtime = runtime();
    runtime
        .call_with::<_, ()>(CALLER, "scripted:counter", "on_log_huge", ())
        .unwrap();
    let lines = runtime.log_lines();
    // A hundred lines, then one saying the rest were dropped.
    assert_eq!(lines.len(), 101);
    assert!(lines[100].message.contains("dropped"), "{:?}", lines[100]);
    assert_eq!(lines[100].level, LogLevel::Warn);
    let first = &lines[0].message;
    assert!(first.ends_with("…(1000000 bytes)"), "{}", &first[4000..]);
    assert!(first.len() < 5000);
    // The next call logs again.
    runtime
        .call_with::<_, ()>(CALLER, "scripted:counter", "on_log_caught", ())
        .unwrap();
    assert_eq!(runtime.log_lines().pop().unwrap().message, "caught");
}

#[test]
fn a_huge_error_message_is_cut_like_a_log_line() {
    let mut runtime = runtime();
    let error = failure(runtime.call_with(CALLER, "scripted:counter", "on_error_huge", ()));
    assert_eq!(error.kind, ErrorKind::Runtime);
    assert!(
        error.message.ends_with("…(1000000 bytes)"),
        "{}",
        &error.message[4000..]
    );
    assert!(error.message.len() < 5000);
    // It still says where it happened.
    assert_eq!(
        error.file.as_deref(),
        Some("mods/scripted/scripts/counter.luau")
    );
    assert_eq!(error.line, Some(25));
}

#[test]
fn a_syntax_error_is_reported_with_its_line() {
    let runtime = runtime();
    let error = &runtime.load_errors()[0];
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert_eq!(error.mod_id, "scripted");
    assert_eq!(
        error.file.as_deref(),
        Some("mods/scripted/scripts/broken.luau")
    );
    assert_eq!(error.line, Some(3));
    assert!(!runtime.is_loaded("scripted:broken"));
}

#[test]
fn calling_a_script_that_didnt_load_disables_the_caller() {
    let mut runtime = runtime();
    let error = failure(runtime.call_with(CALLER, "scripted:broken", "on_tick", ()));
    assert_eq!(error.kind, ErrorKind::Syntax);
    assert!(runtime.is_disabled(CALLER));
    let error = failure(runtime.call_with(Caller::Other(2), "scripted:nowhere", "on_tick", ()));
    assert_eq!(error.kind, ErrorKind::Missing);
}

#[test]
fn a_runtime_error_names_the_line_and_disables_only_its_caller() {
    let mut runtime = runtime();
    let error = failure(runtime.call_with(CALLER, "scripted:failing", "on_tick", ()));
    assert_eq!(error.kind, ErrorKind::Runtime);
    assert_eq!(
        error.file.as_deref(),
        Some("mods/scripted/scripts/failing.luau")
    );
    assert_eq!(error.line, Some(4));
    assert_eq!(error.hook.as_deref(), Some("on_tick"));
    assert!(error.message.contains("attempt to index nil"), "{error}");

    // Disabled from now on, without running or logging again.
    assert!(runtime.is_disabled(CALLER));
    assert_eq!(
        runtime.call_with::<_, i32>(CALLER, "scripted:answer", "on_answer", ()),
        Err(CallError::Disabled)
    );
    // Other callers carry on.
    let other = Caller::Other(2);
    assert_eq!(
        runtime.call_with::<_, i32>(other, "scripted:answer", "on_answer", ()),
        Ok(Some(42))
    );
    // Until the scripts reload.
    runtime.reload(None);
    assert!(!runtime.is_disabled(CALLER));
    assert_eq!(runtime.generation(), 1);
}

#[test]
fn a_runaway_loop_is_stopped_by_the_budget() {
    let mut runtime = runtime();
    let error = failure(runtime.call_with(CALLER, "scripted:runaway", "on_tick", ()));
    assert_eq!(error.kind, ErrorKind::Budget);
    assert_eq!(
        error.file.as_deref(),
        Some("mods/scripted/scripts/runaway.luau")
    );
    assert!(matches!(error.line, Some(2 | 3)), "{error}");
    assert_eq!(
        runtime.steps_used("scripted"),
        Some(ScriptSettings::default().step_budget)
    );
}

#[test]
fn running_out_in_a_library_function_names_the_scripts_line() {
    let mut runtime = runtime();
    let error = failure(runtime.call_with(CALLER, "scripted:runaway", "on_find", ()));
    assert_eq!(error.kind, ErrorKind::Budget);
    assert_eq!(
        error.file.as_deref(),
        Some("mods/scripted/scripts/runaway.luau")
    );
    assert_eq!(error.line, Some(18), "{error}");
}

#[test]
fn catching_the_budgets_error_doesnt_escape_it() {
    let mut runtime = runtime();
    let error = failure(runtime.call_with(CALLER, "scripted:runaway", "on_catch", ()));
    assert_eq!(error.kind, ErrorKind::Budget);
}

#[test]
fn long_library_calls_are_stopped_by_the_time_limit() {
    let mut runtime = ScriptRuntime::new(Side::Server, fixture(), ScriptSettings::default());
    let started = Instant::now();
    let error = failure(runtime.call_with(CALLER, "scripted:runaway", "on_sort", ()));
    let took = started.elapsed();
    assert_eq!(error.kind, ErrorKind::Budget);
    assert!(error.message.contains("time limit of 20 ms"), "{error}");
    // Far fewer steps than the budget: it was the clock that stopped it.
    assert!(runtime.steps_used("scripted").unwrap() < 1_000);
    // The clock is read every few steps, a few sorts apart.
    assert!(took < Duration::from_secs(2), "took {took:?}");
}

/// A call the clock stopped, after a few slow steps, counts as its whole
/// step budget, so a tick's pool of steps can't be drained by slow
/// library calls at a few steps each.
#[test]
fn a_call_that_runs_out_counts_as_its_whole_budget() {
    let mut runtime = ScriptRuntime::new(Side::Server, fixture(), ScriptSettings::default());
    let before = runtime.steps_spent();
    failure(runtime.call_with(CALLER, "scripted:runaway", "on_sort", ()));
    assert!(runtime.steps_used("scripted").unwrap() < 1_000);
    assert_eq!(
        runtime.steps_spent() - before,
        ScriptSettings::default().step_budget
    );
}

#[test]
fn a_smaller_budget_stops_sooner() {
    let mut runtime = ScriptRuntime::new(
        Side::Server,
        fixture(),
        ScriptSettings {
            step_budget: 1,
            ..default()
        },
    );
    // Even loading needs more than one step for a script with a loop at
    // its top level; these have none, so they load.
    assert!(runtime.is_loaded("scripted:counter"));
    let error = failure(runtime.call_with(CALLER, "scripted:hog", "on_tick", ()));
    assert_eq!(error.kind, ErrorKind::Budget);
}

#[test]
fn a_mod_over_its_memory_cap_fails_alone() {
    let mut runtime = ScriptRuntime::new(
        Side::Server,
        fixture(),
        ScriptSettings {
            step_budget: u64::MAX,
            time_limit: Duration::MAX,
            memory_limit: 4 << 20,
        },
    );
    let error = failure(runtime.call_with(CALLER, "scripted:hog", "on_tick", ()));
    assert_eq!(error.kind, ErrorKind::Memory);
    assert!(error.message.contains("4096 KiB"), "{error}");
    assert!(runtime.memory_used("scripted").unwrap() <= 4 << 20);
    // Other mods have caps of their own.
    assert_eq!(
        runtime.call_with::<_, i32>(Caller::Other(2), "other:peek", "on_require", ()),
        Ok(Some(42))
    );
}

#[test]
fn scripts_dont_see_each_others_globals() {
    let mut runtime = runtime();
    assert_eq!(
        runtime.call_with::<_, String>(CALLER, "scripted:secret", "on_secret", ()),
        Ok(Some("mine".into()))
    );
    // Not from another mod, hooks included...
    let (secret, hook) = runtime
        .call_with::<_, (Option<String>, Option<mlua::Function>)>(
            CALLER,
            "other:peek",
            "on_peek",
            (),
        )
        .unwrap()
        .unwrap();
    assert_eq!(secret, None);
    assert!(hook.is_none());
    // ...nor through the shared globals, which are read-only.
    let error = failure(runtime.call_with(CALLER, "other:peek", "on_write_global", ()));
    assert_eq!(error.line, Some(11), "{error}");
    assert_eq!(
        runtime.call_with::<_, String>(Caller::Other(2), "scripted:secret", "on_secret", ()),
        Ok(Some("mine".into()))
    );
}

#[test]
fn scripts_from_the_same_mod_dont_share_globals_either() {
    let sources = ScriptSources::from_mods(&crate::content::load::order(
        vec![crate::content::load::ModFiles::new(
            "mods/m",
            "{ id: \"m\", name: \"M\" }",
            &[
                ("scripts/a.luau", "shared = 1"),
                ("scripts/b.luau", "function on_read() return shared end"),
            ],
        )],
        &mut Vec::new(),
    ));
    let mut runtime = ScriptRuntime::new(Side::Server, sources, ScriptSettings::untimed());
    assert_eq!(
        runtime.call_with::<_, Option<i32>>(CALLER, "m:b", "on_read", ()),
        Ok(Some(None))
    );
}

#[test]
fn require_reaches_only_the_mod_and_its_dependencies() {
    let mut runtime = runtime();
    assert_eq!(
        runtime.call_with::<_, i32>(CALLER, "other:peek", "on_require", ()),
        Ok(Some(42))
    );
    let error = failure(runtime.call_with(CALLER, "stranger:snoop", "on_snoop", ()));
    assert!(error.message.contains("depends on"), "{error}");
    assert_eq!(
        error.file.as_deref(),
        Some("mods/stranger/scripts/snoop.luau")
    );
    assert_eq!(error.line, Some(3));
}

#[test]
fn the_sandbox_leaves_out_the_os_files_and_loading_code() {
    let mut runtime = runtime();
    let types: Vec<String> = runtime
        .call_with::<_, mlua::Variadic<String>>(CALLER, "other:peek", "on_escape", ())
        .unwrap()
        .unwrap()
        .to_vec();
    assert!(types.iter().all(|t| t == "nil"), "{types:?}");
}

/// A stand-in for engine state, such as a machine's tank.
#[derive(Debug, Default)]
struct Tank {
    amount: f64,
}

impl UserData for Tank {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("amount", |_, tank, ()| Ok(tank.amount));
        methods.add_method_mut("fill", |_, tank, amount: f64| {
            tank.amount += amount;
            Ok(())
        });
    }
}

#[test]
fn hooks_change_engine_state_handed_to_them() {
    let mut runtime = runtime();
    let mut tank = Tank { amount: 1.0 };
    let amount = runtime.call::<_, f64>(CALLER, "scripted:tank", "on_fill", |scope| {
        Ok((scope.create_userdata_ref_mut(&mut tank)?, 2.5))
    });
    assert_eq!(amount, Ok(Some(3.5)));
    assert_eq!(tank.amount, 3.5);
}

#[test]
fn engine_handles_dont_outlive_the_call() {
    let mut runtime = runtime();
    let mut tank = Tank::default();
    runtime
        .call::<_, ()>(CALLER, "scripted:tank", "on_keep", |scope| {
            scope.create_userdata_ref_mut(&mut tank)
        })
        .unwrap();
    let error = failure(runtime.call_with(CALLER, "scripted:tank", "on_use_kept", ()));
    assert_eq!(error.kind, ErrorKind::Runtime);
    assert_eq!(error.line, Some(14), "{error}");
}

#[test]
fn changing_the_settings_reloads() {
    let mut runtime = runtime();
    runtime.set_settings(runtime.settings().clone());
    assert_eq!(runtime.generation(), 0);
    runtime.set_settings(ScriptSettings {
        step_budget: 10,
        ..default()
    });
    assert_eq!(runtime.generation(), 1);
    assert_eq!(runtime.settings().step_budget, 10);
}

#[test]
fn forgetting_a_player_enables_only_its_scripts() {
    let mut runtime = runtime();
    let (gone, stays) = (
        Entity::from_raw_u32(1).unwrap(),
        Entity::from_raw_u32(2).unwrap(),
    );
    for caller in [
        Caller::script("scripted:failing", Some(gone)),
        Caller::script("scripted:failing", Some(stays)),
        CALLER,
    ] {
        let _ = runtime.call_with::<_, ()>(caller, "scripted:failing", "on_tick", ());
        assert!(runtime.is_disabled(caller), "{caller:?}");
    }
    runtime.forget_player(gone);
    assert!(!runtime.is_disabled(Caller::script("scripted:failing", Some(gone))));
    assert!(runtime.is_disabled(Caller::script("scripted:failing", Some(stays))));
    assert!(runtime.is_disabled(CALLER));
}
