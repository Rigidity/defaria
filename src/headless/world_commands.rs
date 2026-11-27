//! World commands, which set up a scene without steering there (`fill`,
//! `pour`, `put`, `hotbar`, `give`, `camera`, `random-ticks`, `lookups`), and the
//! `fluids` and `network` reports.

use std::{collections::HashSet, time::Instant};

use bevy::prelude::*;

use super::{memory, runner::ScriptRunner, script::Command};
use crate::{
    block_entity::{BlockEntities, BlockEntity},
    content::{BlockState, Content, FULL_FLUID, ItemState, StateName},
    fluid::FluidStats,
    fluid_container,
    hotbar::{ActiveSlot, Hotbar},
    inventory::{Inventory, ItemStack},
    machine::{Slots, Tanks},
    physics::Body,
    player::{EYE_HEIGHT, LocalPlayer, Look, MAX_PITCH, Player, put_eyes},
    script::random_tick::RandomTicks,
    streaming::{ChunkLoader, StreamingSettings, loader_chunk},
    view_bob::ViewBob,
    world::{CHUNK_SIZE, Chunk, ChunkCoord, ChunkMap, Voxels},
};

/// Applies this frame's `fill` and `camera` commands. A mistake in one (an
/// unknown block, no camera) fails the run.
pub(super) fn apply_world_commands(world: &mut World) {
    let commands = std::mem::take(&mut world.resource_mut::<ScriptRunner>().world_commands);
    for command in commands {
        let result = match command {
            Command::Fill { min, max, block } => {
                let start = Instant::now();
                let result = world
                    .run_system_cached_with(fill, (min, max, block))
                    .map_err(|error| error.to_string())
                    .and_then(|result| result);
                let elapsed = start.elapsed();
                result.map(|set| {
                    let filled = &mut world.resource_mut::<ScriptRunner>().filled;
                    filled.0 += set;
                    filled.1 += elapsed;
                })
            }
            Command::Lookups { min, max } => world
                .run_system_cached_with(memory::measure_lookups, (min, max))
                .map_err(|error| error.to_string())
                .and_then(|result| result)
                .map(|cost| {
                    let frame = world.resource::<bevy::diagnostic::FrameCount>().0;
                    println!("[frame {frame}] {cost}");
                }),
            Command::Pour { pos, fluid, amount } => world
                .run_system_cached_with(pour, (pos, fluid, amount))
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Command::Put {
                pos,
                name,
                amount,
                what,
            } => world
                .run_system_cached_with(put, (pos, name, amount, what))
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Command::Camera {
                position,
                yaw,
                pitch,
            } => world
                .run_system_cached_with(place_camera, (position, yaw, pitch))
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Command::Hotbar { slot, item } => world
                .run_system_cached_with(hold_item, (slot, item))
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Command::Give { item, count, fluid } => world
                .run_system_cached_with(give_items, (item, count, fluid))
                .map_err(|error| error.to_string())
                .and_then(|result| result),
            Command::RandomTicks(per_chunk) => {
                world.resource_mut::<RandomTicks>().per_chunk = per_chunk;
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(error) = result {
            error!("{error}");
            world.write_message(AppExit::error());
        }
    }
}

/// The state name `name` (see [`StateName`]), its id qualified with
/// `defaria:` if it has no mod.
fn split_name(name: &str) -> Result<StateName, String> {
    let mut parsed = StateName::parse(name)?;
    if !parsed.id.contains(':') {
        parsed.id = format!("defaria:{}", parsed.id);
    }
    Ok(parsed)
}

/// The item state `name` means, named as for [`fill_state`].
fn item_state(content: &Content, name: &str) -> Result<ItemState, String> {
    let parsed = split_name(name)?;
    let id = &parsed.id;
    let item = content.item(id).ok_or_else(|| format!("no item `{id}`"))?;
    item.state(&parsed.pairs())
        .ok_or_else(|| format!("item `{id}` has no state `{name}`"))
}

/// Puts the item `name` in hotbar slot `slot` and makes it the active one.
fn hold_item(
    In((slot, name)): In<(usize, String)>,
    content: Res<Content>,
    mut players: Query<(&mut Hotbar, &mut ActiveSlot), With<LocalPlayer>>,
) -> Result<(), String> {
    let item = item_state(&content, &name).map_err(|error| format!("hotbar: {error}"))?;
    let (mut hotbar, mut active) = players
        .single_mut()
        .map_err(|_| "hotbar: there's no player".to_string())?;
    hotbar.assign(slot, Some(item));
    *active = ActiveSlot(slot);
    Ok(())
}

/// Puts `count` of the item `name` in the player's inventory, each holding
/// `fluid` if it's given.
fn give_items(
    In((name, count, fluid)): In<(String, u32, Option<(String, f32)>)>,
    content: Res<Content>,
    mut players: Query<&mut Inventory, With<LocalPlayer>>,
) -> Result<(), String> {
    let item = item_state(&content, &name).map_err(|error| format!("give: {error}"))?;
    let data = match fluid {
        Some((fluid, amount)) => {
            let state = fill_state(&content, &fluid).map_err(|error| format!("give: {error}"))?;
            let id = &content.block_of(state).id;
            let units = (amount * FULL_FLUID as f32).round().max(0.0) as u64;
            fluid_container::saved_data(&content, item, id, units)
                .map_err(|error| format!("give: `{name}` can't hold that: {error}"))?
        }
        None => default(),
    };
    let mut inventory = players
        .single_mut()
        .map_err(|_| "give: there's no player".to_string())?;
    if let Some(left) = inventory.add(&content, ItemStack { item, data, count }) {
        warn!("give: {} of `{name}` didn't fit", left.count);
    }
    Ok(())
}

/// The state `name` means: a block id (the base game's without
/// `defaria:`), with `[property=value,…]` setting some of its properties.
fn fill_state(content: &Content, name: &str) -> Result<BlockState, String> {
    let parsed = split_name(name)?;
    let id = &parsed.id;
    let block = content
        .block(id)
        .ok_or_else(|| format!("no block `{id}`"))?;
    block
        .state(&parsed.pairs())
        .ok_or_else(|| format!("block `{id}` has no state `{name}`"))
}

fn fill(
    In((min, max, id)): In<(IVec3, IVec3, String)>,
    content: Res<Content>,
    mut voxels: Voxels,
) -> Result<usize, String> {
    let state = if id == crate::content::AIR {
        BlockState::AIR
    } else {
        fill_state(&content, &id).map_err(|error| format!("fill: {error}"))?
    };
    // The voxels it changed: not ones already full of `state`, nor ones
    // in chunks that aren't loaded.
    let mut set = 0;
    for x in min.x..=max.x {
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                let pos = IVec3::new(x, y, z);
                let changes = voxels
                    .get_with_fluid(pos)
                    .is_some_and(|before| before != (state, None));
                if voxels.set(pos, state) && changes {
                    set += 1;
                }
            }
        }
    }
    Ok(set)
}

fn pour(
    In((pos, id, amount)): In<(IVec3, String, f32)>,
    content: Res<Content>,
    mut voxels: Voxels,
) -> Result<(), String> {
    let state = fill_state(&content, &id).map_err(|error| format!("pour: {error}"))?;
    if !content.state(state).is_fluid() {
        return Err(format!("pour: `{id}` isn't a fluid"));
    }
    let units = (amount * FULL_FLUID as f32).round();
    if !(1.0..=FULL_FLUID as f32).contains(&units) {
        return Err(format!(
            "pour: {amount} blocks is out of range; it must be over 0 and at most 1"
        ));
    }
    voxels.set(pos, state);
    voxels.set_fluid(pos, units as u16);
    Ok(())
}

fn put(
    In((pos, name, amount, what)): In<(IVec3, String, f32, String)>,
    content: Res<Content>,
    index: Res<BlockEntities>,
    mut machines: Query<(Option<&mut Slots>, Option<&mut Tanks>)>,
) -> Result<(), String> {
    let at = |error: String| format!("put at {} {} {}: {error}", pos.x, pos.y, pos.z);
    let (slots, tanks) = index
        .get(pos)
        .and_then(|entity| machines.get_mut(entity).ok())
        .ok_or_else(|| at("there's no machine here".into()))?;
    // A fluid goes in a tank, anything else in a slot group: a machine
    // may have both by one name (a generator's `fuel`).
    let fluid = fill_state(&content, &what)
        .ok()
        .filter(|&state| content.state(state).is_fluid());
    if let (Some(fluid), Some(tank)) = (
        fluid,
        tanks
            .map(|tanks| tanks.into_inner())
            .and_then(|tanks| tanks.get_mut(&name)),
    ) {
        let id = content.block_of(fluid).id.clone();
        let units = (amount * FULL_FLUID as f32).round();
        if units < 1.0 {
            return Err(at(format!("{amount} blocks is too little")));
        }
        let units = units as u64;
        if !tank.takes(&id) || tank.room() < units {
            return Err(at(format!(
                "{amount} blocks of `{id}` don't fit in `{name}`"
            )));
        }
        tank.fill(&id, units);
        return Ok(());
    }
    if let Some(group) = slots
        .map(|slots| slots.into_inner())
        .and_then(|slots| slots.group_mut(&name))
    {
        let item = item_state(&content, &what).map_err(at)?;
        if amount < 1.0 || amount.fract() != 0.0 {
            return Err(at(format!("{amount} isn't a whole number of items")));
        }
        let stack = crate::inventory::ItemStack::new(item, amount as u32);
        let room = group.room_for(&content, &stack);
        if room < stack.count {
            return Err(at(format!("only {room} of `{what}` fit in `{name}`")));
        }
        group.insert(&content, stack);
        return Ok(());
    }
    Err(at(format!(
        "the machine has no slot group, or tank for `{what}`, called `{name}`"
    )))
}

/// The part of the world a `fluids` report counts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FluidRegion {
    /// The voxels between two corners, both included.
    Box { min: IVec3, max: IVec3 },
    /// The chunks the camera's position loads, without the margin (see
    /// [`StreamingSettings::loads`]).
    Window,
}

impl FluidRegion {
    /// The chunks it covers, and the box it's limited to inside them, if
    /// any. Without a loader, the window is around chunk (0, 0, 0).
    fn chunks(self, world: &mut World) -> (Vec<IVec3>, Option<(IVec3, IVec3)>) {
        match self {
            Self::Box { min, max } => {
                let (low, high) = (ChunkCoord::split(min).0.0, ChunkCoord::split(max).0.0);
                let chunks = (low.x..=high.x)
                    .flat_map(|x| {
                        (low.y..=high.y)
                            .flat_map(move |y| (low.z..=high.z).map(move |z| IVec3::new(x, y, z)))
                    })
                    .collect();
                (chunks, Some((min, max)))
            }
            Self::Window => {
                let center = world
                    .query_filtered::<&GlobalTransform, With<ChunkLoader>>()
                    .iter(world)
                    .next()
                    .map(loader_chunk)
                    .unwrap_or_default();
                let settings = world.resource::<StreamingSettings>();
                (settings.window(center).collect(), None)
            }
        }
    }
}

/// The `fluids` report: each fluid's total in `region`'s voxels and
/// machines' tanks, and in the fluid networks with a pipe in it, in
/// thousandths of a block; how many chunks the region covers and how many of them
/// weren't loaded; and the active voxels in the whole world. A fluid
/// that's anywhere but the world also shows where it is.
pub(super) fn fluid_report(world: &mut World, region: FluidRegion) -> String {
    use std::collections::BTreeMap;
    let (covered, bounds) = region.chunks(world);
    let in_chunks: HashSet<IVec3> = covered.iter().copied().collect();
    let contains = |pos: IVec3| match bounds {
        Some((min, max)) => pos.cmpge(min).all() && pos.cmple(max).all(),
        None => in_chunks.contains(&ChunkCoord::split(pos).0.0),
    };
    let mut in_world: BTreeMap<String, u64> = default();
    let mut missing = covered.len();
    let map = world.resource::<ChunkMap>();
    let content = world.resource::<Content>();
    for &coord in &covered {
        let Some(chunk) = map
            .get(ChunkCoord(coord))
            .and_then(|entity| world.get::<Chunk>(entity))
        else {
            continue;
        };
        missing -= 1;
        let origin = ChunkCoord(coord).origin();
        // Only a box needs checking voxel by voxel, and only where it
        // doesn't cover the whole chunk.
        let whole = bounds.is_none_or(|(min, max)| {
            origin.cmpge(min).all() && (origin + CHUNK_SIZE as i32 - 1).cmple(max).all()
        });
        for index in 0..chunk.voxels().len() {
            let pos = Chunk::position_of(index);
            if !whole && !contains(origin + pos) {
                continue;
            }
            let amount = chunk.fluid_amount_at(pos, content);
            if amount > 0 {
                let name = &content.blocks[content.state(chunk.at(pos)).block].id;
                *in_world.entry(name.clone()).or_default() += amount as u64;
            }
        }
    }
    let in_pipes = crate::fluid_network::held_in_pipes_within(world, contains);
    let mut in_machines: BTreeMap<String, u64> = default();
    let mut tanks = world.query::<(&crate::machine::Tanks, &BlockEntity)>();
    for (tanks, block_entity) in tanks.iter(world) {
        if !contains(block_entity.pos) {
            continue;
        }
        for tank in &tanks.0 {
            if let Some(fluid) = &tank.fluid {
                *in_machines.entry(fluid.clone()).or_default() += tank.amount;
            }
        }
    }
    let names: std::collections::BTreeSet<&String> = in_world
        .keys()
        .chain(in_pipes.keys())
        .chain(in_machines.keys())
        .collect();
    let stats = world.resource::<FluidStats>();
    let what = match region {
        FluidRegion::Box { min, max } => format!(
            "box {} {} {} to {} {} {}",
            min.x, min.y, min.z, max.x, max.y, max.z
        ),
        FluidRegion::Window => "the camera's window".to_owned(),
    };
    let mut line = format!(
        "fluids in {what} ({} chunks, {missing} not loaded): {} active",
        covered.len(),
        stats.active
    );
    for name in names {
        let get = |map: &BTreeMap<String, u64>| map.get(name).copied().unwrap_or_default();
        let (here, pipes, machines) = (get(&in_world), get(&in_pipes), get(&in_machines));
        line += &format!(", {name} {}", here + pipes + machines);
        if pipes + machines > 0 {
            line += &format!(" (world {here}, pipes {pipes}, machines {machines})");
        }
    }
    line
}

/// The `network` report for the conduit at `pos`.
pub(super) fn network_report(world: &World, pos: IVec3) -> String {
    let ports = world.resource::<crate::machine::MachinePorts>();
    let networks = world.resource::<crate::network::Networks>();
    let mut line =
        crate::network::describe(networks, world.resource::<Content>(), pos, |id, port| {
            ports.role(id, port)
        });
    let energy = networks.at(pos).and_then(|id| {
        let kind = networks.get(id)?.kind();
        let report = world.get_resource::<crate::network::energy::EnergyReport>()?;
        (world.resource::<Content>().networks[kind].solver == crate::content::Solver::Energy)
            .then(|| crate::network::energy::describe(report, id))
    });
    if let Some(energy) = energy {
        line += &format!("; {energy}");
    }
    let fluid = networks.at(pos).and_then(|id| {
        world
            .get_resource::<crate::network::NetworkMap<crate::fluid_network::FluidNet>>()?
            .get(id)
    });
    if let Some(net) = fluid.filter(|net| net.total() > 0) {
        let held: Vec<String> = net
            .held
            .iter()
            .map(|held| format!("{} {}", held.fluid, held.amount))
            .collect();
        line += &format!(", holding {}", held.join(", "));
    }
    line
}

/// Puts the local player's eyes at `position`, looking along `yaw` and
/// `pitch`, flying, so the view stays put.
fn place_camera(
    In((position, yaw, pitch)): In<(Vec3, f32, f32)>,
    mut players: Query<
        (
            &mut Look,
            &mut Transform,
            &mut Player,
            &mut Body,
            Option<&mut ViewBob>,
        ),
        With<LocalPlayer>,
    >,
) -> Result<(), String> {
    let (mut look, mut transform, mut player, mut body, bob) = players
        .single_mut()
        .map_err(|error| format!("camera: {error}"))?;
    // The body turns to face the look as the frame is drawn.
    *look = Look {
        yaw: yaw.to_radians(),
        pitch: pitch.to_radians().clamp(-MAX_PITCH, MAX_PITCH),
    };
    player.flying = true;
    player.eye_height = EYE_HEIGHT;
    player.previous_eye_height = EYE_HEIGHT;
    player.drawn_eye_height = EYE_HEIGHT;
    if let Some(mut bob) = bob {
        bob.settle();
    }
    put_eyes(position.as_dvec3(), EYE_HEIGHT, &mut body, &mut transform);
    Ok(())
}

#[cfg(test)]
mod tests;
