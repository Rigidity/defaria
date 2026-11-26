//! Recipes as content: `recipes/<name>.json5`, what a recipe machine takes,
//! makes and needs (see "Recipes" in `docs/modding.md`).
//!
//! A recipe with `materials` is a template: it's baked once for each
//! combination of its materials, with `{name}` in its inputs and outputs
//! filled with the material's id. A combination that names something that
//! isn't there (an ingot of coal) is skipped, so one file smelts every
//! `#ore` that has an ingot into it. Each baked recipe is a [`Recipe`] in
//! [`Content::recipes`], its item inputs already resolved to the item
//! states they take.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    BlockState, Content, FULL_FLUID, ItemState, Machine, MachineRecipes, NONE, Solver,
    bake::{When, pattern_matches},
    check_namespace, index_of, qualify,
    schema::{Condition, OneOrMany, RecipeDef, RecipeInputDef, RecipeOutputDef},
    state_count,
};
use crate::{machine::SLOT_SIZE, network::TICKS_PER_SECOND};

/// A recipe, baked: for a templated one, one combination of its materials.
#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    /// Its file's id, and for a templated one its materials:
    /// `defaria:smelt_ore[ore=defaria:iron]`. What a machine's progress is
    /// saved under.
    pub id: String,
    /// Which machines run it: those whose `recipes` is this type.
    pub kind: String,
    pub inputs: Vec<RecipeInput>,
    pub outputs: Vec<RecipeOutput>,
    /// Joules it uses, over its time.
    pub energy: u64,
    /// How long it takes, in network ticks (at least 1).
    pub ticks: u32,
}

/// What a recipe takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipeInput {
    /// `count` of any of these item states, together.
    Item { items: Vec<ItemState>, count: u32 },
    /// `amount` of a fluid, in [`FULL_FLUID`] units, by its block's id.
    Fluid { fluid: String, amount: u64 },
}

/// What a recipe gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipeOutput {
    Item {
        item: ItemState,
        count: u32,
    },
    /// In [`FULL_FLUID`] units.
    Fluid {
        fluid: String,
        amount: u64,
    },
    /// Joules into the machine's buffer, over the recipe's time.
    Energy(u64),
}

impl RecipeInput {
    /// Whether an item input takes `item`.
    pub fn takes(&self, item: ItemState) -> bool {
        match self {
            RecipeInput::Item { items, .. } => items.binary_search(&item).is_ok(),
            RecipeInput::Fluid { .. } => false,
        }
    }
}

impl Recipe {
    /// The energy it makes, over its time.
    pub fn generates(&self) -> u64 {
        self.outputs
            .iter()
            .map(|output| match output {
                RecipeOutput::Energy(energy) => *energy,
                _ => 0,
            })
            .sum()
    }
}

/// The part of `total` due on tick `tick` of `ticks`: the ticks add up to
/// exactly `total`.
pub fn share(total: u64, tick: u32, ticks: u32) -> u64 {
    let (total, tick, ticks) = (total as u128, tick as u128, ticks.max(1) as u128);
    (total * (tick + 1) / ticks - total * tick / ticks) as u64
}

impl Content {
    /// The recipe saved as `id` (see [`Recipe::id`]).
    pub fn recipe(&self, id: &str) -> Option<&Recipe> {
        self.recipe_index.get(id).map(|&i| &self.recipes[i])
    }

    /// Every recipe of type `kind`, in load order (each mod's by id): the
    /// order a machine tries them in.
    pub fn recipes_of<'a>(&'a self, kind: &str) -> impl Iterator<Item = &'a Recipe> + 'a {
        self.recipe_types
            .get(kind)
            .into_iter()
            .flatten()
            .map(|&i| &self.recipes[i])
    }

    /// Whether some recipe of type `kind` takes `item`.
    pub fn recipes_take(&self, kind: &str, item: ItemState) -> bool {
        self.recipe_items
            .get(kind)
            .is_some_and(|items| items.binary_search(&item).is_ok())
    }

    /// Whether slot group `group` of `machine` takes `item`: a recipe
    /// machine's input groups take only what one of its type's recipes
    /// takes, so nothing goes in that would sit there for good; every
    /// other group takes anything.
    pub fn slot_group_takes(&self, machine: &Machine, group: usize, item: ItemState) -> bool {
        match &machine.recipes {
            Some(spec) if spec.input_slots.contains(&group) => self.recipes_take(&spec.kind, item),
            _ => true,
        }
    }

    /// Adds recipes, after every item has loaded.
    pub(super) fn add_recipes(&mut self, recipes: Vec<Recipe>) {
        for recipe in recipes {
            let taken = self.recipe_items.entry(recipe.kind.clone()).or_default();
            for input in &recipe.inputs {
                if let RecipeInput::Item { items, .. } = input {
                    taken.extend(items);
                }
            }
            let index = self.recipes.len();
            self.recipe_index.insert(recipe.id.clone(), index);
            self.recipe_types
                .entry(recipe.kind.clone())
                .or_default()
                .push(index);
            self.recipes.push(recipe);
        }
        for items in self.recipe_items.values_mut() {
            items.sort();
            items.dedup();
        }
    }
}

/// Recipe setups that can never run, once every block and recipe has
/// loaded: errors for a recipe machine none of whose type's recipes it can
/// run (or whose type has none), warnings for one that can run only some
/// of them (another machine of the type may run the rest) and for a recipe
/// type no machine runs. Each names the block and the recipes.
pub(super) fn check_recipe_machines(content: &Content) -> (Vec<String>, Vec<String>) {
    let (mut errors, mut warnings) = (Vec::new(), Vec::new());
    let mut run = BTreeSet::new();
    for block in &content.blocks {
        let mut problems = BTreeSet::new();
        let mut runs_one = false;
        let mut kind = None;
        for offset in 0..block.state_count {
            let state = BlockState(block.first_state + offset);
            let Some(machine) = content.machine(state) else {
                continue;
            };
            let Some(spec) = &machine.recipes else {
                continue;
            };
            kind = Some(spec.kind.as_str());
            for recipe in content.recipes_of(&spec.kind) {
                match cannot_run(content, recipe, machine, spec) {
                    Some(why) => {
                        problems.insert(format!("`{}` {why}", recipe.id));
                    }
                    None => runs_one = true,
                }
            }
        }
        let Some(kind) = kind else {
            continue;
        };
        run.insert(kind);
        let problems: Vec<String> = problems.into_iter().collect();
        if content.recipes_of(kind).next().is_none() {
            errors.push(format!(
                "block `{}` runs `{kind}` recipes, and there are none",
                block.id
            ));
        } else if !runs_one {
            errors.push(format!(
                "block `{}` can't run any of its `{kind}` recipes: {}",
                block.id,
                problems.join("; ")
            ));
        } else if !problems.is_empty() {
            warnings.push(format!(
                "block `{}` can't run some of its `{kind}` recipes: {}",
                block.id,
                problems.join("; ")
            ));
        }
    }
    let kinds: BTreeSet<&str> = content.recipes.iter().map(|r| r.kind.as_str()).collect();
    for kind in kinds.difference(&run) {
        warnings.push(format!("no machine runs `{kind}` recipes"));
    }
    (errors, warnings)
}

/// Why `machine` can never run `recipe`, if it can't.
fn cannot_run(
    content: &Content,
    recipe: &Recipe,
    machine: &Machine,
    spec: &MachineRecipes,
) -> Option<String> {
    // How much of `fluid` the tanks `tanks` hold together, and how many
    // slots the groups `groups` have.
    let room = |tanks: &[usize], fluid: &str| -> u64 {
        tanks
            .iter()
            .map(|&tank| &machine.tanks[tank])
            .filter(|tank| tank.accepts.is_empty() || tank.accepts.iter().any(|f| f == fluid))
            .map(|tank| tank.capacity)
            .sum()
    };
    let slots =
        |groups: &[usize]| -> u32 { groups.iter().map(|&group| machine.slots[group].count).sum() };
    let unique = |item: ItemState| content.item_of(item).is_some_and(|item| item.unique);
    let blocks = |amount: u64| amount as f64 / FULL_FLUID as f64;
    for input in &recipe.inputs {
        match input {
            RecipeInput::Item { .. } if spec.input_slots.is_empty() => {
                return Some("takes items, and the machine has no input slot group".into());
            }
            RecipeInput::Fluid { fluid, .. }
                if !spec.input_tanks.iter().any(|&tank| {
                    let accepts = &machine.tanks[tank].accepts;
                    accepts.is_empty() || accepts.contains(fluid)
                }) =>
            {
                return Some(format!(
                    "takes `{fluid}`, and no input tank of the machine holds it"
                ));
            }
            _ => {}
        }
    }
    for input in &recipe.inputs {
        match input {
            // Unique items take a slot each.
            RecipeInput::Item { items, count }
                if items.iter().all(|&item| unique(item)) && *count > slots(&spec.input_slots) =>
            {
                return Some(format!(
                    "takes {count} unique items, more than the machine's input slots ({})",
                    slots(&spec.input_slots)
                ));
            }
            RecipeInput::Fluid { fluid, amount } if *amount > room(&spec.input_tanks, fluid) => {
                return Some(format!(
                    "takes {} blocks of `{fluid}`, more than the machine's input tanks hold ({})",
                    blocks(*amount),
                    blocks(room(&spec.input_tanks, fluid))
                ));
            }
            _ => {}
        }
    }
    for output in &recipe.outputs {
        match output {
            RecipeOutput::Item { .. } if spec.output_slots.is_empty() => {
                return Some("gives items, and the machine has no output slot group".into());
            }
            RecipeOutput::Fluid { fluid, .. }
                if !spec.output_tanks.iter().any(|&tank| {
                    let accepts = &machine.tanks[tank].accepts;
                    accepts.is_empty() || accepts.contains(fluid)
                }) =>
            {
                return Some(format!(
                    "gives `{fluid}`, and no output tank of the machine holds it"
                ));
            }
            _ => {}
        }
    }
    for output in &recipe.outputs {
        match output {
            RecipeOutput::Item { item, count }
                if unique(*item) && *count > slots(&spec.output_slots) =>
            {
                return Some(format!(
                    "gives {count} unique items, more than the machine's output slots ({})",
                    slots(&spec.output_slots)
                ));
            }
            RecipeOutput::Fluid { fluid, amount } if *amount > room(&spec.output_tanks, fluid) => {
                return Some(format!(
                    "gives {} blocks of `{fluid}`, more than the machine's output tanks hold ({})",
                    blocks(*amount),
                    blocks(room(&spec.output_tanks, fluid))
                ));
            }
            _ => {}
        }
    }
    if let Some(why) = too_many_kinds(content, recipe, machine, spec) {
        return Some(why);
    }
    // Energy it uses has to get into its buffer: through a port that
    // takes it from a network, or made by the machine itself.
    if recipe.energy > 0
        && let Some(energy) = machine.energy
    {
        let fed = energy.max_in > 0
            && machine.ports.iter().any(|port| {
                port.role.takes() && content.networks[port.network].solver == Solver::Energy
            });
        if !fed && energy.generate == 0 && energy.max_generate == 0 {
            return Some(
                "uses energy, and nothing can fill the machine's buffer (no port taking it \
                 with a `max_in`, and no `generate` or `max_generate`)"
                    .into(),
            );
        }
    }
    let capacity = machine.energy.map(|energy| energy.capacity);
    for (energy, what) in [(recipe.energy, "uses"), (recipe.generates(), "makes")] {
        let per_tick = energy.div_ceil(recipe.ticks.max(1) as u64);
        match capacity {
            _ if energy == 0 => {}
            None => {
                return Some(format!(
                    "{what} energy, and the machine has no energy buffer"
                ));
            }
            Some(capacity) if per_tick > capacity => {
                return Some(format!(
                    "{what} {per_tick} J a tick, more than the machine's buffer holds ({capacity} J)"
                ));
            }
            _ => {}
        }
    }
    None
}

/// Why a recipe's inputs, or its outputs, can't all be in the machine at
/// once, if they can't: each slot holds one kind of item (a slot's worth,
/// or one unique item) and each tank one fluid.
fn too_many_kinds(
    content: &Content,
    recipe: &Recipe,
    machine: &Machine,
    spec: &MachineRecipes,
) -> Option<String> {
    let unique = |item: ItemState| content.item_of(item).is_some_and(|item| item.unique);
    let slots_for = |count: u32, unique: bool| {
        if unique {
            count
        } else {
            count.div_ceil(SLOT_SIZE)
        }
    };
    let slots =
        |groups: &[usize]| -> u32 { groups.iter().map(|&group| machine.slots[group].count).sum() };
    // Tanks among `tanks` that take one of `fluids`.
    let tanks = |tanks: &[usize], fluids: &BTreeSet<&str>| {
        tanks
            .iter()
            .map(|&tank| &machine.tanks[tank])
            .filter(|tank| {
                tank.accepts.is_empty() || tank.accepts.iter().any(|f| fluids.contains(f.as_str()))
            })
            .count()
    };

    // Item inputs that can't share a slot (none of their items in common)
    // need slots of their own.
    let items: Vec<(&Vec<ItemState>, u32)> = recipe
        .inputs
        .iter()
        .filter_map(|input| match input {
            RecipeInput::Item { items, count } => Some((items, *count)),
            RecipeInput::Fluid { .. } => None,
        })
        .collect();
    let apart = items.iter().enumerate().all(|(i, (a, _))| {
        items[i + 1..]
            .iter()
            .all(|(b, _)| !a.iter().any(|item| b.binary_search(item).is_ok()))
    });
    if apart && items.len() > 1 {
        let needed: u32 = items
            .iter()
            .map(|(items, count)| slots_for(*count, items.iter().all(|&item| unique(item))))
            .sum();
        if needed > slots(&spec.input_slots) {
            return Some(format!(
                "takes items needing {needed} slots, more than the machine's input slots ({})",
                slots(&spec.input_slots)
            ));
        }
    }
    let mut outputs: BTreeMap<ItemState, u32> = BTreeMap::new();
    for output in &recipe.outputs {
        if let RecipeOutput::Item { item, count } = output {
            *outputs.entry(*item).or_default() += count;
        }
    }
    if outputs.len() > 1 {
        let needed: u32 = outputs
            .iter()
            .map(|(&item, &count)| slots_for(count, unique(item)))
            .sum();
        if needed > slots(&spec.output_slots) {
            return Some(format!(
                "gives items needing {needed} slots, more than the machine's output slots ({})",
                slots(&spec.output_slots)
            ));
        }
    }

    for (fluids, side, groups) in [
        (
            recipe
                .inputs
                .iter()
                .filter_map(|input| match input {
                    RecipeInput::Fluid { fluid, .. } => Some(fluid.as_str()),
                    RecipeInput::Item { .. } => None,
                })
                .collect::<BTreeSet<&str>>(),
            "takes",
            &spec.input_tanks,
        ),
        (
            recipe
                .outputs
                .iter()
                .filter_map(|output| match output {
                    RecipeOutput::Fluid { fluid, .. } => Some(fluid.as_str()),
                    _ => None,
                })
                .collect(),
            "gives",
            &spec.output_tanks,
        ),
    ] {
        let room = tanks(groups, &fluids);
        if fluids.len() > 1 && fluids.len() > room {
            let which = if side == "takes" { "input" } else { "output" };
            return Some(format!(
                "{side} {} fluids, more than the machine's {which} tanks that hold them ({room})",
                fluids.len()
            ));
        }
    }
    None
}

/// Why a combination of a recipe's materials didn't bake.
enum Miss {
    /// It names something that isn't there for these materials (an ingot
    /// of coal): skipped, unless every combination is.
    Skip(String),
    /// A mistake whatever the materials.
    Fail(String),
}

impl From<String> for Miss {
    fn from(error: String) -> Self {
        Miss::Fail(error)
    }
}

/// Bakes recipe file `id` (in mod `namespace`): one recipe, or one for each
/// combination of its materials that works.
pub(super) fn bake_recipe(
    id: &str,
    namespace: &str,
    def: &RecipeDef,
    content: &Content,
) -> Result<Vec<Recipe>, String> {
    check_namespace(&def.kind).map_err(|error| format!("recipe type {error}"))?;
    if def.inputs.is_empty() {
        return Err("a recipe needs at least one input".into());
    }
    if !(def.time.is_finite() && def.time > 0.0) {
        return Err(format!("`time` is {}; it must be over 0 seconds", def.time));
    }
    let mut names = Vec::new();
    for (name, materials) in &def.materials {
        check_namespace(name).map_err(|error| format!("recipe material name {error}"))?;
        names.push((
            name.clone(),
            materials_of(name, materials, namespace, content)?,
        ));
    }

    let mut recipes = Vec::new();
    let mut skipped = None;
    let combinations = names.iter().map(|(_, m)| m.len()).product::<usize>();
    for n in 0..combinations {
        let mut rest = n;
        let mut values = Vec::new();
        for (name, materials) in names.iter().rev() {
            values.push((name.clone(), materials[rest % materials.len()].clone()));
            rest /= materials.len();
        }
        values.reverse();
        match bake_one(id, namespace, def, &values, content) {
            Ok(recipe) => recipes.push(recipe),
            Err(Miss::Skip(why)) if !names.is_empty() => {
                skipped.get_or_insert(why);
            }
            Err(Miss::Skip(why) | Miss::Fail(why)) => return Err(why),
        }
    }
    if recipes.is_empty() {
        return Err(format!(
            "no combination of its materials makes a recipe: {}",
            skipped.unwrap_or_default()
        ));
    }
    Ok(recipes)
}

/// The materials a recipe's material name stands for, each once.
fn materials_of(
    name: &str,
    materials: &OneOrMany,
    namespace: &str,
    content: &Content,
) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for entry in materials.values() {
        let found: Vec<String> = if let Some(tag) = entry.strip_prefix('#') {
            content
                .materials
                .values()
                .filter(|material| material.has_tag(tag))
                .map(|material| material.id.clone())
                .collect()
        } else {
            let id = qualify(entry, namespace)?;
            if !content.materials.contains_key(&id) {
                return Err(format!(
                    "material `{name}` names `{id}`, which isn't a material"
                ));
            }
            vec![id]
        };
        for id in found {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    if out.is_empty() {
        return Err(format!("material `{name}` names no materials"));
    }
    Ok(out)
}

/// Fills `{name}` in `text` with the material's id. Whether anything was
/// filled is returned too: what a templated value names may be missing for
/// some materials.
fn fill(text: &str, values: &[(String, String)]) -> Result<(String, bool), String> {
    let mut out = String::new();
    let mut rest = text;
    let mut filled = false;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('}')
            .ok_or_else(|| format!("`{text}` has a `{{` with no `}}`"))?;
        let name = &rest[start + 1..start + end];
        let value = values.iter().find(|(n, _)| n == name).ok_or_else(|| {
            format!("`{text}` uses `{{{name}}}`, which isn't one of the recipe's `materials`")
        })?;
        out.push_str(&value.1);
        filled = true;
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    Ok((out, filled))
}

fn bake_one(
    file_id: &str,
    namespace: &str,
    def: &RecipeDef,
    values: &[(String, String)],
    content: &Content,
) -> Result<Recipe, Miss> {
    let id = if values.is_empty() {
        file_id.to_string()
    } else {
        let pairs: Vec<String> = values.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{file_id}[{}]", pairs.join(","))
    };
    let mut inputs = Vec::new();
    for (i, input) in def.inputs.iter().enumerate() {
        let input =
            bake_input(input, namespace, values, content).map_err(|miss| at("input", i, miss))?;
        inputs.push(input);
    }
    let mut outputs = Vec::new();
    for (i, output) in def.outputs.iter().enumerate() {
        let output = bake_output(output, namespace, values, content)
            .map_err(|miss| at("output", i, miss))?;
        outputs.push(output);
    }
    let ticks = (def.time * TICKS_PER_SECOND as f32).round().max(1.0) as u32;
    Ok(Recipe {
        id,
        kind: def.kind.clone(),
        inputs,
        outputs,
        energy: def.energy,
        ticks,
    })
}

fn at(what: &str, i: usize, miss: Miss) -> Miss {
    let say = |why: String| format!("{what} {}: {why}", i + 1);
    match miss {
        Miss::Skip(why) => Miss::Skip(say(why)),
        Miss::Fail(why) => Miss::Fail(say(why)),
    }
}

/// How many of an item, 1 to a slot's worth.
fn count(count: Option<u32>) -> Result<u32, Miss> {
    let count = count.unwrap_or(1);
    if count == 0 || count > SLOT_SIZE {
        return Err(Miss::Fail(format!(
            "`count` is {count}; give 1 to {SLOT_SIZE}"
        )));
    }
    Ok(count)
}

/// A fluid's id and amount, in [`FULL_FLUID`] units.
fn fluid(
    fluid: &str,
    amount: Option<f32>,
    namespace: &str,
    values: &[(String, String)],
    content: &Content,
) -> Result<(String, u64), Miss> {
    let (fluid, filled) = fill(fluid, values)?;
    let id = qualify(&fluid, namespace)?;
    let is_fluid = content
        .block(&id)
        .is_some_and(|block| content.state(block.default_state()).is_fluid());
    if !is_fluid {
        let why = format!("`{id}` isn't a fluid");
        return Err(if filled {
            Miss::Skip(why)
        } else {
            Miss::Fail(why)
        });
    }
    let amount = amount.ok_or_else(|| Miss::Fail("a fluid needs an `amount`".into()))?;
    let units = (amount * FULL_FLUID as f32).round();
    if !(amount.is_finite() && units >= 1.0) {
        return Err(Miss::Fail(format!(
            "`amount` is {amount}; it must be at least a thousandth of a block"
        )));
    }
    Ok((id, units as u64))
}

fn bake_input(
    def: &RecipeInputDef,
    namespace: &str,
    values: &[(String, String)],
    content: &Content,
) -> Result<RecipeInput, Miss> {
    match (&def.item, &def.fluid) {
        (Some(item), None) => {
            if def.amount.is_some() {
                return Err(Miss::Fail("an item has a `count`, not an `amount`".into()));
            }
            let (items, count) = (
                item_states(item, &def.when, namespace, values, content)?,
                count(def.count)?,
            );
            Ok(RecipeInput::Item { items, count })
        }
        (None, Some(name)) => {
            if def.count.is_some() || !def.when.is_empty() {
                return Err(Miss::Fail(
                    "a fluid has an `amount`, not a `count` or `when`".into(),
                ));
            }
            let (fluid, amount) = fluid(name, def.amount, namespace, values, content)?;
            Ok(RecipeInput::Fluid { fluid, amount })
        }
        _ => Err(Miss::Fail("an input is an `item` or a `fluid`".into())),
    }
}

/// The states of item `item` whose properties match `when`, filled with
/// the recipe's materials, in order.
fn item_states(
    item: &str,
    when: &BTreeMap<String, Condition>,
    namespace: &str,
    values: &[(String, String)],
    content: &Content,
) -> Result<Vec<ItemState>, Miss> {
    let (item, _) = fill(item, values)?;
    let id = qualify(&item, namespace)?;
    let item = content
        .item(&id)
        .ok_or_else(|| Miss::Fail(format!("no item `{id}`")))?;
    let mut filled_when = BTreeMap::new();
    for (name, condition) in when {
        let property = item
            .properties
            .iter()
            .find(|p| &p.name == name)
            .ok_or_else(|| Miss::Fail(format!("item `{id}` has no property `{name}`")))?;
        let (patterns, not) = match condition {
            Condition::Is(patterns) => (patterns, false),
            Condition::Not(not) => (&not.not, true),
        };
        let mut out = Vec::new();
        for pattern in patterns.values() {
            let (pattern, filled) = fill(pattern, values)?;
            if filled && pattern != NONE {
                let hits = pattern_matches(property, &pattern, namespace, content)?;
                if !hits.iter().any(|&hit| hit) {
                    return Err(Miss::Skip(format!(
                        "item `{id}` has no `{name}` of `{pattern}`"
                    )));
                }
            }
            out.push(pattern);
        }
        let patterns = OneOrMany::Many(out);
        let condition = if not {
            Condition::Not(super::schema::NotCondition { not: patterns })
        } else {
            Condition::Is(patterns)
        };
        filled_when.insert(name.clone(), condition);
    }
    let when = When::new(&filled_when, &item.properties, namespace, content)
        .map_err(|error| Miss::Fail(format!("item `{id}`: {error}")))?;
    let states: Vec<ItemState> = (0..state_count(&item.properties))
        .filter(|&offset| when.matches(&index_of(&item.properties, offset)))
        .map(|offset| ItemState(item.first_state + offset as u32))
        .collect();
    Ok(states)
}

fn bake_output(
    def: &RecipeOutputDef,
    namespace: &str,
    values: &[(String, String)],
    content: &Content,
) -> Result<RecipeOutput, Miss> {
    match (&def.item, &def.fluid, def.energy) {
        (Some(item), None, None) => {
            if def.amount.is_some() {
                return Err(Miss::Fail("an item has a `count`, not an `amount`".into()));
            }
            let (item, _) = fill(item, values)?;
            let id = qualify(&item, namespace)?;
            let item = content
                .item(&id)
                .ok_or_else(|| Miss::Fail(format!("no item `{id}`")))?;
            let mut pairs = Vec::new();
            for (name, value) in &def.properties {
                let (value, filled) = fill(value, values)?;
                let property = item
                    .properties
                    .iter()
                    .find(|p| &p.name == name)
                    .ok_or_else(|| Miss::Fail(format!("item `{id}` has no property `{name}`")))?;
                // A material is named by its id; a reference in this mod
                // can leave out the mod.
                let value = if property.materials && value != NONE {
                    qualify(&value, namespace)?
                } else {
                    value
                };
                if !property.values.contains(&value) {
                    let why = format!("item `{id}` has no `{name}` of `{value}`");
                    return Err(if filled {
                        Miss::Skip(why)
                    } else {
                        Miss::Fail(why)
                    });
                }
                pairs.push((name.clone(), value));
            }
            let pairs: Vec<(&str, &str)> = pairs
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let state = item.state(&pairs).expect("every value checked");
            Ok(RecipeOutput::Item {
                item: state,
                count: count(def.count)?,
            })
        }
        (None, Some(name), None) => {
            if def.count.is_some() || !def.properties.is_empty() {
                return Err(Miss::Fail(
                    "a fluid has an `amount`, not a `count` or `properties`".into(),
                ));
            }
            let (fluid, amount) = fluid(name, def.amount, namespace, values, content)?;
            Ok(RecipeOutput::Fluid { fluid, amount })
        }
        (None, None, Some(energy)) => {
            if def.count.is_some() || def.amount.is_some() || !def.properties.is_empty() {
                return Err(Miss::Fail(
                    "`energy` is a number of joules on its own".into(),
                ));
            }
            if energy == 0 {
                return Err(Miss::Fail(
                    "`energy` is 0; make some, or leave it out".into(),
                ));
            }
            Ok(RecipeOutput::Energy(energy))
        }
        _ => Err(Miss::Fail(
            "an output is an `item`, a `fluid` or `energy`".into(),
        )),
    }
}

#[cfg(test)]
mod tests;
