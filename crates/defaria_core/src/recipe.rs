//! A recipe machine's progress ([`RecipeProgress`]), the block entity
//! part that keeps which recipe is running and how far it's got, saved
//! with the chunk and shown on the machine's screen. Running recipes is
//! the server's (`defaria_server::recipe`).

use std::{collections::BTreeMap, sync::Arc};

use bevy::prelude::*;

use crate::{
    block_entity::{BlockEntityPart, Value},
    content::{BlockState, Content, MachineRecipes, share},
};

/// A recipe machine's progress: the recipe it's running, whose inputs it
/// has taken, and how many of its ticks have run. Saved with the block
/// entity (but not [`RecipeProgress::status`]).
#[derive(
    Component, Reflect, Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize,
)]
#[reflect(Component)]
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
    /// Not sent to clients: what they show needs none of it.
    #[reflect(ignore)]
    #[serde(skip)]
    pub spec: Arc<MachineRecipes>,
}

/// What a recipe machine is doing.
#[derive(
    Reflect, Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize,
)]
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
    /// None running yet, for a machine that runs `spec`.
    pub fn of(spec: &Arc<MachineRecipes>) -> Self {
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
