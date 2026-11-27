//! Machines' own hooks, on top of the hooks every scripted block has
//! (`hooks`):
//!
//! - `on_tick(m, dt)`: every network tick ([`NetworkClock`], 20 a second,
//!   `dt` the tick's length in seconds), while none of the networks on
//!   the machine's ports is frozen. It's the one hook in which
//!   `m.energy:generate(n)` makes anything: up to the machine's
//!   `max_generate` share of the tick.
//! - `on_recipe_finished(m, recipe)`: a recipe machine finished a recipe
//!   (its outputs went in); `recipe` is `{ id, type, ticks, energy }`.
//!
//! Both run in `FixedUpdate`, after the energy, fluid and item solvers and
//! the recipe machines, so a script sees what this tick moved, and what
//! it makes or moves is offered to the networks on the next one.
//! `on_recipe_finished` runs on the network tick; `on_tick`s are spread
//! over the fixed steps until the next one ([`MachineTickPhase`]), so no
//! one frame runs every machine's. Each is called with the machine's handle ([`MachineRef`]) and
//! [`Caller::Block`], as its block hooks are, so an error disables that
//! machine's script and no other. Machines' `on_tick`s get a pool of
//! steps and of time a tick of their own, the size of block hooks'
//! ([`HookSettings::steps_per_tick`], [`HookSettings::time_per_tick`]),
//! split between those steps; `on_recipe_finished`s get a pool as big
//! again, and those that don't fit wait for the next network tick.
//!
//! [`MachineRef`]: super::api::MachineRef

use std::{collections::VecDeque, time::Instant};

use bevy::prelude::*;
use mlua::Lua;

use super::{
    Caller, ScriptRuntime,
    api::{MachineRef, ScriptWorld},
    hooks::{DropWarning, HookSettings, HookSystems, ON_RECIPE_FINISHED, ON_TICK, ScriptIndex},
};
use crate::{
    block_entity::{BlockEntities, BlockEntity},
    fluid_network::FluidSystems,
    machine::Ports,
    network::{ItemSystems, NetworkClock, NetworkTick, Networks, TICK, on_network_tick},
    recipe::{RecipeFinished, RecipeSystems},
};
use defaria_core::{content::Content, machine::MachineActive};

pub struct MachineScriptPlugin;

impl Plugin for MachineScriptPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MachineTickPhase>()
            .init_resource::<PendingRecipeHooks>()
            .add_systems(
                FixedUpdate,
                (run_recipe_hooks.run_if(on_network_tick), run_machine_ticks)
                    .chain()
                    .in_set(MachineScriptSystems)
                    .in_set(NetworkTick::Machines)
                    .after(FluidSystems)
                    .after(ItemSystems)
                    .after(RecipeSystems)
                    .after(HookSystems::Run),
            );
    }
}

/// Where machines' `on_tick` and `on_recipe_finished` run, in
/// `FixedUpdate`, on network ticks.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct MachineScriptSystems;

/// Which of the fixed steps between two network ticks the machine at
/// `pos` runs its `on_tick` on, of `steps`: spread by position, so no one
/// step runs them all.
fn tick_slot(pos: IVec3, steps: u32) -> u32 {
    let mut hash = (pos.x as u32).wrapping_mul(0x9E37_79B1)
        ^ (pos.y as u32).wrapping_mul(0x85EB_CA77)
        ^ (pos.z as u32).wrapping_mul(0xC2B2_AE3D);
    // Mixed, so neighbours land in any slot.
    hash = (hash ^ (hash >> 16)).wrapping_mul(0x7FEB_352D);
    hash = (hash ^ (hash >> 15)).wrapping_mul(0x846C_A68B);
    (hash ^ (hash >> 16)) % steps
}

/// How many fixed steps every network tick has, at least: the steps
/// machines' `on_tick`s are spread over.
fn steps_per_network_tick(fixed: &Time<Fixed>) -> u32 {
    let ratio = TICK.as_secs_f64() / fixed.timestep().as_secs_f64();
    (ratio + 1e-6).floor().max(1.0) as u32
}

/// Where [`run_machine_ticks`] is in the current network tick.
#[derive(Resource, Debug, Default)]
pub struct MachineTickPhase {
    /// The fixed steps since the last network tick (0 on its own step),
    /// or `None` before the first.
    step: Option<u32>,
    /// How many steps the machines are spread over.
    steps: u32,
    /// For each slot, the machine its turn starts with next (see
    /// [`run_machine_ticks`]).
    next: Vec<Option<IVec3>>,
}

impl MachineTickPhase {
    /// Whether every machine has had its `on_tick` for the last network
    /// tick: its last slot has run.
    pub fn tick_done(&self) -> bool {
        self.step.is_some_and(|step| step + 1 >= self.steps)
    }
}

/// Calls `on_tick` on every machine whose script defines it and whose
/// networks are running, once each network tick. They're spread over the
/// fixed steps until the next network tick ([`tick_slot`]): a third of
/// them on the network tick's own step, after the solvers, and the rest
/// on the two after it, each called with the network tick's `dt` and
/// share of `max_generate`. Each step's machines run in order of
/// position, so runs repeat, until they've used their share of
/// [`HookSettings::steps_per_tick`] or of [`HookSettings::time_per_tick`]
/// (the first always runs). The rest skip the tick, and their
/// slot's next turn starts with them (`next`), so the same machines don't
/// always miss out.
fn run_machine_ticks(
    clock: Res<NetworkClock>,
    fixed: Res<Time<Fixed>>,
    settings: Res<HookSettings>,
    mut phase: ResMut<MachineTickPhase>,
    index: Res<ScriptIndex>,
    networks: Res<Networks>,
    machines: Query<(&BlockEntity, Option<&Ports>), With<MachineActive>>,
    mut runtime: ResMut<ScriptRuntime>,
    mut world: ScriptWorld,
) {
    let steps = steps_per_network_tick(&fixed);
    let step = if clock.due {
        0
    } else {
        match phase.step {
            Some(step) => step + 1,
            None => return,
        }
    };
    phase.step = Some(step);
    phase.steps = steps;
    // No script has an `on_tick`: there's nothing to look through.
    if step >= steps || !index.any_tick() {
        return;
    }
    phase.next.resize(steps as usize, None);
    let mut due: Vec<(IVec3, &str, u64)> = machines
        .iter()
        .filter(|(block, _)| tick_slot(block.pos, steps) == step)
        .filter(|(_, ports)| {
            !ports.is_some_and(|ports| {
                ports
                    .0
                    .iter()
                    .filter_map(|link| networks.get(link.network?))
                    .any(|network| network.is_frozen())
            })
        })
        .filter_map(|(block, _)| {
            let state = world.get(block.pos)?;
            let script = index.block_hook(&world.content, state, ON_TICK)?;
            let max = world
                .content
                .machine(state)
                .and_then(|machine| machine.energy.as_ref())
                .map_or(0, |energy| energy.max_generate);
            Some((block.pos, script, clock.share(max)))
        })
        .collect();
    due.sort_by_key(|(pos, ..)| pos.to_array());
    let next = &mut phase.next[step as usize];
    let first = next
        .take()
        .and_then(|next| {
            due.iter()
                .position(|(pos, ..)| pos.to_array() >= next.to_array())
        })
        .unwrap_or(0);
    due.rotate_left(first);
    let start = runtime.steps_spent();
    let pool = settings.steps_per_tick.div_ceil(steps as u64);
    let started = Instant::now();
    let time = settings.time_per_tick / steps;
    let dt = TICK.as_secs_f64();
    for (i, (pos, script, generate)) in due.into_iter().enumerate() {
        if runtime.steps_spent().saturating_sub(start) >= pool
            || (i > 0 && started.elapsed() >= time)
        {
            *next = Some(pos);
            break;
        }
        let call = world.begin(Some(pos)).allow_generating(generate);
        // Errors are logged, and disable the machine's script, in the
        // runtime.
        let _ = runtime.call_scoped(Caller::Block(pos), script, ON_TICK, |lua, scope, hook| {
            call.run::<_, ()>(lua, scope, hook, |lua, _| {
                Ok((MachineRef::create(lua, pos)?, dt))
            })
        });
    }
}

/// The most `on_recipe_finished`s that wait at once. Past that, new ones
/// are dropped (with a warning), so machines finishing faster than their
/// hooks run can't use up memory.
const MAX_WAITING: usize = 1 << 16;

/// Recipes finished whose `on_recipe_finished` hasn't run yet, oldest
/// first, each with the block entity of the machine that finished it.
#[derive(Resource, Debug, Default)]
struct PendingRecipeHooks {
    hooks: VecDeque<(RecipeFinished, Option<Entity>)>,
    dropping: DropWarning,
}

impl PendingRecipeHooks {
    /// Queues `done`, finished by the machine with block entity `entity`,
    /// unless the queue is full.
    fn push(&mut self, done: RecipeFinished, entity: Option<Entity>) {
        if self
            .dropping
            .admits(self.hooks.len(), MAX_WAITING, "`on_recipe_finished` hooks")
        {
            self.hooks.push_back((done, entity));
        }
    }
}

/// Calls `on_recipe_finished` for the recipes machines finished, oldest
/// first, until they've used a tick's pool of steps or of time, the size
/// of block hooks' ([`HookSettings::steps_per_tick`],
/// [`HookSettings::time_per_tick`]; the first always runs). The rest wait
/// for the next network tick, so machines that finish together don't all
/// run at once. A machine broken (or replaced by another) before its hook
/// runs doesn't get it.
fn run_recipe_hooks(
    mut finished: MessageReader<RecipeFinished>,
    mut pending: ResMut<PendingRecipeHooks>,
    block_entities: Res<BlockEntities>,
    settings: Res<HookSettings>,
    index: Res<ScriptIndex>,
    mut runtime: ResMut<ScriptRuntime>,
    mut world: ScriptWorld,
) {
    for done in finished.read() {
        pending.push(done.clone(), block_entities.get(done.pos));
    }
    let start = runtime.steps_spent();
    let started = Instant::now();
    let mut called = false;
    while !pending.hooks.is_empty() {
        if runtime.steps_spent().saturating_sub(start) >= settings.steps_per_tick
            || (called && started.elapsed() >= settings.time_per_tick)
        {
            break;
        }
        let (done, entity) = pending.hooks.pop_front().expect("it isn't empty");
        if block_entities.get(done.pos) != entity {
            // Not the machine that finished it.
            continue;
        }
        let Some(state) = world.get(done.pos) else {
            continue;
        };
        let Some(script) = index.block_hook(&world.content, state, ON_RECIPE_FINISHED) else {
            continue;
        };
        let pos = done.pos;
        called = true;
        let _ = world.call::<_, ()>(
            &mut runtime,
            Caller::Block(pos),
            script,
            ON_RECIPE_FINISHED,
            Some(pos),
            |lua, content| {
                Ok((
                    MachineRef::create(lua, pos)?,
                    recipe_table(lua, content, &done.recipe)?,
                ))
            },
        );
    }
}

/// A recipe as a table: `{ id, type, ticks, energy }`, `energy` what it
/// uses (or, negative, makes) over its time.
fn recipe_table(lua: &Lua, content: &Content, id: &str) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    table.raw_set("id", id)?;
    if let Some(recipe) = content.recipe(id) {
        table.raw_set("type", recipe.kind.as_str())?;
        table.raw_set("ticks", recipe.ticks)?;
        table.raw_set("energy", recipe.energy as f64 - recipe.generates() as f64)?;
    }
    Ok(table)
}

#[cfg(test)]
mod tests;
