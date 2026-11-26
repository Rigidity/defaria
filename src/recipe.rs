//! Recipe machines: machines with `recipes` (see "Recipes" in
//! `docs/modding.md`) run the recipes of their type on what's in their
//! input slots and tanks, whoever put it there (a player, a tube, a pipe,
//! a script).
//!
//! On each network tick ([`NetworkClock`], after the solvers), a
//! machine that isn't running a recipe tries its type's recipes in order
//! and starts the first whose inputs it holds, whose outputs fit and whose
//! first tick it can pay for: it takes the inputs then. A running recipe
//! takes its energy from the machine's buffer a tick's share at a time,
//! and a generator's recipe puts its energy in, so it stops (keeping its
//! progress) while the buffer can't pay, or has no room, and goes on when
//! it can. Once every tick has run, the outputs go in, or it waits until
//! they fit, and the next recipe starts on the same tick. The machine is
//! active ([`MachineActive`]) while it holds a recipe, running or waiting,
//! so a machine short of power or room doesn't blink on and off.
//!
//! [`RecipeProgress`] is the block entity part that keeps which recipe is
//! running and how far it's got, saved with the chunk.

use std::{collections::BTreeMap, sync::Arc};

use bevy::prelude::*;

use crate::{
    block_entity::{BlockEntity, BlockEntityPart, RegisterBlockEntityPart, Value},
    content::{BlockState, Content, MachineRecipes, Recipe, RecipeInput, RecipeOutput, share},
    inventory::ItemStack,
    machine::{EnergyBuffer, MachineActive, Slots, Tanks},
    network::{
        NetworkTick,
        energy::{EnergyNeed, EnergyReport},
        on_network_tick,
    },
};

pub struct RecipePlugin;

impl Plugin for RecipePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedUpdate,
            ask_for_energy
                .after(run_recipes)
                .in_set(NetworkTick::Machines)
                .in_set(RecipeSystems)
                .run_if(on_network_tick),
        );
        app.register_block_entity_part::<RecipeProgress>()
            .add_message::<RecipeFinished>()
            .add_systems(
                FixedUpdate,
                run_recipes
                    .in_set(NetworkTick::Machines)
                    .in_set(RecipeSystems)
                    .run_if(on_network_tick),
            );
    }
}

/// Where recipe machines run and say what energy they need next, in
/// `FixedUpdate`: in [`NetworkTick::Machines`], after the solvers, on
/// network ticks.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct RecipeSystems;

/// A recipe machine finished a recipe: its outputs went in. Scripts'
/// `on_recipe_finished` runs from these.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub struct RecipeFinished {
    /// The machine's voxel.
    pub pos: IVec3,
    /// The recipe's id ([`Recipe::id`]).
    pub recipe: String,
}

/// A recipe machine's progress: the recipe it's running, whose inputs it
/// has taken, and how many of its ticks have run. Saved with the block
/// entity (but not [`RecipeProgress::status`]).
#[derive(Component, Reflect, Debug, Clone, PartialEq, Default)]
#[reflect(Component)]
#[require(EnergyNeed)]
pub struct RecipeProgress {
    /// The running recipe's id ([`Recipe::id`]).
    pub recipe: Option<String>,
    /// Network ticks of it that have run.
    pub ticks: u32,
    /// Network ticks it takes in all; 0 while none is running.
    pub total: u32,
    pub status: RecipeStatus,
    /// Waiting for energy to start a recipe ([`RecipeStatus::NoEnergy`]
    /// with none running): what the first recipe it could start takes on
    /// its first tick, which it asks the network for. Not saved.
    pub start_cost: u64,
    /// What the machine runs and where, from its block: shared, so
    /// copying the progress each tick is cheap.
    #[reflect(ignore)]
    pub spec: Arc<MachineRecipes>,
}

/// What a recipe machine is doing.
#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecipeStatus {
    /// Nothing to do: no recipe's inputs are there.
    #[default]
    Idle,
    /// Running a recipe.
    Running,
    /// Waiting for energy: its buffer can't pay for the next tick.
    NoEnergy,
    /// Waiting for room: the outputs (or a generator's energy) don't fit.
    OutputFull,
}

impl RecipeProgress {
    fn of(spec: &Arc<MachineRecipes>) -> Self {
        Self {
            spec: spec.clone(),
            ..default()
        }
    }

    /// The energy it asks the network for on the next tick, before its
    /// buffer is topped up: the running recipe's share, or, waiting for
    /// energy to start one, that recipe's first tick.
    pub fn energy_need(&self, content: &Content) -> u64 {
        let running = self.recipe.as_ref().and_then(|id| content.recipe(id));
        match running {
            Some(recipe) if self.ticks < recipe.ticks => {
                share(recipe.energy, self.ticks, recipe.ticks)
            }
            None if self.status == RecipeStatus::NoEnergy => self.start_cost,
            _ => 0,
        }
    }

    /// How far the running recipe has got, from 0 to 1.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.ticks as f32 / self.total as f32
        }
    }
}

impl BlockEntityPart for RecipeProgress {
    const KEY: &'static str = "recipe";

    fn new(state: BlockState, content: &Content) -> Option<Self> {
        let spec = content.machine(state)?.recipes.clone()?;
        Some(Self::of(&Arc::new(spec)))
    }

    /// `{ recipe, ticks }` while one is running, `{}` otherwise.
    fn save(&self, _: &Content) -> Value {
        let mut table = BTreeMap::new();
        if let Some(recipe) = &self.recipe {
            table.insert("recipe".to_string(), Value::Text(recipe.clone()));
            table.insert("ticks".to_string(), Value::Int(self.ticks as i64));
        }
        Value::Table(table)
    }

    fn load(value: &Value, version: u32, state: BlockState, content: &Content) -> Option<Self> {
        if version != 1 {
            return None;
        }
        let mut progress = Self::new(state, content)?;
        let table = value.as_table()?;
        let Some(Value::Text(id)) = table.get("recipe") else {
            return Some(progress);
        };
        match content.recipe(id) {
            Some(recipe) if recipe.kind == progress.spec.kind => {
                let ticks = u32::try_from(table.get("ticks")?.as_int()?).ok()?;
                progress.recipe = Some(id.clone());
                progress.total = recipe.ticks;
                progress.ticks = ticks.min(recipe.ticks);
                progress.status = RecipeStatus::Running;
            }
            _ => warn!(
                "a machine was running recipe `{id}`, which isn't there any more; its inputs are lost"
            ),
        }
        Some(progress)
    }
}

/// What a recipe machine holds, for a tick: copies of its parts.
#[derive(Debug, Clone, Default)]
pub struct Holdings {
    pub slots: Option<Slots>,
    pub tanks: Option<Tanks>,
    pub energy: Option<EnergyBuffer>,
    /// Energy drained and filled this tick, for the [`EnergyReport`].
    pub used: u64,
    pub generated: u64,
    /// The recipes that finished this tick, by id.
    pub finished: Vec<String>,
}

impl Holdings {
    fn stored(&self) -> u64 {
        self.energy.map_or(0, |energy| energy.stored)
    }

    fn room(&self) -> u64 {
        self.energy.map_or(0, |energy| energy.room())
    }
}

/// Runs every recipe machine for a network tick.
pub fn run_recipes(
    content: Res<Content>,
    mut report: ResMut<EnergyReport>,
    mut finished: MessageWriter<RecipeFinished>,
    mut machines: Query<(
        &BlockEntity,
        &mut RecipeProgress,
        Option<&mut Slots>,
        Option<&mut Tanks>,
        Option<&mut EnergyBuffer>,
        Option<&mut MachineActive>,
    )>,
) {
    for (block, mut progress, slots, tanks, energy, active) in &mut machines {
        let mut new = progress.clone();
        // An idle machine with nothing in its inputs has nothing to start:
        // every recipe takes something.
        if new.recipe.is_none() && !holds_inputs(&new.spec, slots.as_deref(), tanks.as_deref()) {
            new.status = RecipeStatus::Idle;
            new.start_cost = 0;
            progress.set_if_neq(new);
            if let Some(mut active) = active {
                active.set_if_neq(MachineActive(false));
            }
            continue;
        }
        let mut holdings = Holdings {
            slots: slots.as_deref().cloned(),
            tanks: tanks.as_deref().cloned(),
            energy: energy.as_deref().copied(),
            ..default()
        };
        step(&content, &mut new, &mut holdings);
        progress.set_if_neq(new);
        if let (Some(mut slots), Some(new)) = (slots, holdings.slots) {
            slots.set_if_neq(new);
        }
        if let (Some(mut tanks), Some(new)) = (tanks, holdings.tanks) {
            tanks.set_if_neq(new);
        }
        if let (Some(mut energy), Some(new)) = (energy, holdings.energy) {
            energy.set_if_neq(new);
        }
        if let Some(mut active) = active {
            active.set_if_neq(MachineActive(progress.recipe.is_some()));
        }
        report.used += holdings.used;
        report.generated += holdings.generated;
        finished.write_batch(holdings.finished.into_iter().map(|recipe| RecipeFinished {
            pos: block.pos,
            recipe,
        }));
    }
}

/// One network tick of a recipe machine: starts a recipe if none is
/// running, runs a tick of it, and finishes it once every tick has run.
/// Returns whether it's running.
pub fn step(content: &Content, progress: &mut RecipeProgress, holdings: &mut Holdings) -> bool {
    if progress.recipe.is_none() && !start(content, progress, holdings) {
        return false;
    }
    let Some(recipe) = progress.recipe.as_ref().and_then(|id| content.recipe(id)) else {
        // Content changed under it; nothing to finish.
        *progress = RecipeProgress::of(&progress.spec);
        return false;
    };
    if progress.ticks < recipe.ticks {
        let cost = share(recipe.energy, progress.ticks, recipe.ticks);
        let gain = share(recipe.generates(), progress.ticks, recipe.ticks);
        if holdings.stored() < cost {
            progress.status = RecipeStatus::NoEnergy;
            return false;
        }
        if holdings.room() < gain {
            progress.status = RecipeStatus::OutputFull;
            return false;
        }
        if let Some(energy) = &mut holdings.energy {
            holdings.used += energy.drain(cost);
            holdings.generated += energy.fill(gain);
        }
        progress.ticks += 1;
        progress.status = RecipeStatus::Running;
    }
    if progress.ticks >= recipe.ticks {
        let mut done = holdings.clone();
        if !put_outputs(content, recipe, &progress.spec, &mut done) {
            progress.status = RecipeStatus::OutputFull;
            return false;
        }
        holdings.slots = done.slots;
        holdings.tanks = done.tanks;
        holdings.finished.push(recipe.id.clone());
        *progress = RecipeProgress::of(&progress.spec);
        if !start(content, progress, holdings) {
            return false;
        }
    }
    true
}

/// Starts the first recipe of the machine's type whose inputs it holds,
/// whose outputs would fit, and whose first tick it can pay for, taking
/// its inputs. Says why it can't in the status otherwise: waiting for
/// energy if any recipe it holds the inputs for and has room for could
/// start with more (noting what that first one takes), so it asks for
/// it, or else waiting for room.
fn start(content: &Content, progress: &mut RecipeProgress, holdings: &mut Holdings) -> bool {
    let mut status = RecipeStatus::Idle;
    let mut cost = 0;
    for recipe in content.recipes_of(&progress.spec.kind) {
        if !could_take(recipe, &progress.spec, holdings) {
            continue;
        }
        let mut taken = holdings.clone();
        if !take_inputs(recipe, &progress.spec, &mut taken) {
            continue;
        }
        let mut done = taken.clone();
        if !put_outputs(content, recipe, &progress.spec, &mut done)
            || holdings.room() < share(recipe.generates(), 0, recipe.ticks)
        {
            if status == RecipeStatus::Idle {
                status = RecipeStatus::OutputFull;
            }
            continue;
        }
        let first = share(recipe.energy, 0, recipe.ticks);
        if holdings.stored() < first {
            if status != RecipeStatus::NoEnergy {
                status = RecipeStatus::NoEnergy;
                cost = first;
            }
            continue;
        }
        holdings.slots = taken.slots;
        holdings.tanks = taken.tanks;
        progress.recipe = Some(recipe.id.clone());
        progress.ticks = 0;
        progress.total = recipe.ticks;
        progress.status = RecipeStatus::Running;
        progress.start_cost = 0;
        return true;
    }
    progress.status = status;
    progress.start_cost = cost;
    false
}

/// Tells the energy solver what each recipe machine needs on the next
/// tick ([`RecipeProgress::energy_need`]).
fn ask_for_energy(content: Res<Content>, mut machines: Query<(&RecipeProgress, &mut EnergyNeed)>) {
    for (progress, mut need) in &mut machines {
        need.set_if_neq(EnergyNeed(progress.energy_need(&content)));
    }
}

/// Whether anything is in the machine's input slots or tanks.
fn holds_inputs(spec: &MachineRecipes, slots: Option<&Slots>, tanks: Option<&Tanks>) -> bool {
    let items = slots.is_some_and(|slots| spec.input_slots.iter().any(|&g| slots.0[g].total() > 0));
    let fluids = tanks.is_some_and(|tanks| spec.input_tanks.iter().any(|&t| tanks.0[t].amount > 0));
    items || fluids
}

/// Whether each of a recipe's inputs is there on its own, without taking
/// anything: a quick check before [`take_inputs`] on a copy, which also
/// catches two inputs wanting the same items.
fn could_take(recipe: &Recipe, spec: &MachineRecipes, holdings: &Holdings) -> bool {
    recipe.inputs.iter().all(|input| match input {
        RecipeInput::Item { count, .. } => holdings.slots.as_ref().is_some_and(|slots| {
            let held: u64 = spec
                .input_slots
                .iter()
                .flat_map(|&group| slots.0[group].slots.iter().flatten())
                .filter(|stack| input.takes(stack.item))
                .map(|stack| stack.count as u64)
                .sum();
            held >= *count as u64
        }),
        RecipeInput::Fluid { fluid, amount } => holdings.tanks.as_ref().is_some_and(|tanks| {
            let held: u64 = spec
                .input_tanks
                .iter()
                .map(|&tank| &tanks.0[tank])
                .filter(|tank| tank.fluid.as_deref() == Some(fluid.as_str()))
                .map(|tank| tank.amount)
                .sum();
            held >= *amount
        }),
    })
}

/// Takes a recipe's inputs out of the machine's input slots and tanks, in
/// order. False if they aren't all there (and `holdings` is then partly
/// taken from: call it on a copy).
fn take_inputs(recipe: &Recipe, spec: &MachineRecipes, holdings: &mut Holdings) -> bool {
    for input in &recipe.inputs {
        match input {
            RecipeInput::Item { count, .. } => {
                let Some(slots) = &mut holdings.slots else {
                    return false;
                };
                let mut need = *count;
                for &group in &spec.input_slots {
                    let group = &mut slots.0[group];
                    for index in 0..group.slots.len() {
                        let matches = group.slots[index]
                            .as_ref()
                            .is_some_and(|stack| input.takes(stack.item));
                        if need > 0 && matches {
                            need -= group.take(index, need).map_or(0, |taken| taken.count);
                        }
                    }
                }
                if need > 0 {
                    return false;
                }
            }
            RecipeInput::Fluid { fluid, amount } => {
                let Some(tanks) = &mut holdings.tanks else {
                    return false;
                };
                let mut need = *amount;
                for &tank in &spec.input_tanks {
                    let tank = &mut tanks.0[tank];
                    if need > 0 && tank.fluid.as_deref() == Some(fluid.as_str()) {
                        need -= tank.drain(need);
                    }
                }
                if need > 0 {
                    return false;
                }
            }
        }
    }
    true
}

/// Puts a recipe's item and fluid outputs in the machine's output slots and
/// tanks, in order. False if they don't all fit (and `holdings` is then
/// partly filled: call it on a copy).
fn put_outputs(
    content: &Content,
    recipe: &Recipe,
    spec: &MachineRecipes,
    holdings: &mut Holdings,
) -> bool {
    for output in &recipe.outputs {
        match output {
            RecipeOutput::Item { item, count } => {
                let Some(slots) = &mut holdings.slots else {
                    return false;
                };
                let mut left = Some(ItemStack::new(*item, *count));
                for &group in &spec.output_slots {
                    if let Some(stack) = left {
                        left = slots.0[group].insert(content, stack);
                    }
                }
                if left.is_some() {
                    return false;
                }
            }
            RecipeOutput::Fluid { fluid, amount } => {
                let Some(tanks) = &mut holdings.tanks else {
                    return false;
                };
                let mut left = *amount;
                for &tank in &spec.output_tanks {
                    left -= tanks.0[tank].fill(fluid, left);
                }
                if left > 0 {
                    return false;
                }
            }
            RecipeOutput::Energy(_) => {}
        }
    }
    true
}

#[cfg(test)]
mod tests;
