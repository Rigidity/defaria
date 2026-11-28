//! The reports commands print: `fluids` and `network`.

use std::collections::HashSet;

use bevy::prelude::*;

use crate::{
    block_entity::BlockEntity,
    fluid::FluidStats,
    streaming::{ChunkLoader, loader_chunk},
};
use defaria_core::{
    content::Content,
    streaming::StreamingSettings,
    world::{CHUNK_SIZE, Chunk, ChunkCoord, ChunkMap},
};

/// The part of the world a `fluids` report counts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FluidRegion {
    /// The voxels between two corners, both included.
    Box { min: IVec3, max: IVec3 },
    /// The chunks a player's position loads, without the margin (see
    /// [`StreamingSettings::loads`]): this one's, or with `None` (or
    /// one with no loader) the first player's.
    Window(Option<Entity>),
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
            Self::Window(player) => {
                let loaders: Vec<(IVec3, Option<Entity>)> = world
                    .query_filtered::<(&GlobalTransform, Option<&ChildOf>), With<ChunkLoader>>()
                    .iter(world)
                    .map(|(transform, parent)| {
                        (loader_chunk(transform), parent.map(ChildOf::parent))
                    })
                    .collect();
                let center = loaders
                    .iter()
                    .find(|(_, parent)| player.is_some() && *parent == player)
                    .or(loaders.first())
                    .map(|(center, _)| *center)
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
pub fn fluid_report(world: &mut World, region: FluidRegion) -> String {
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
        for (index, fluid) in chunk.fluids() {
            let pos = Chunk::position_of(index);
            if !whole && !contains(origin + pos) {
                continue;
            }
            let name = &content.block_of(fluid.kind).id;
            *in_world.entry(name.clone()).or_default() += fluid.amount as u64;
        }
    }
    let in_pipes = crate::fluid_network::held_in_pipes_within(world, contains);
    let mut in_machines: BTreeMap<String, u64> = default();
    let mut tanks = world.query::<(&defaria_core::machine::Tanks, &BlockEntity)>();
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
        FluidRegion::Window(_) => "the camera's window".to_owned(),
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
    // What's gone since the start, if any: what blocks placed into fluid
    // displaced with nowhere to go (what they pushed aside is still here),
    // what reactions used up and what evaporated.
    let lost: Vec<String> = [
        ("displaced", stats.deleted),
        ("reacted", stats.reacted),
        ("evaporated", stats.evaporated),
    ]
    .into_iter()
    .filter(|&(_, amount)| amount > 0)
    .map(|(how, amount)| format!("{how} {amount}"))
    .collect();
    if !lost.is_empty() {
        line += &format!("; lost: {}", lost.join(", "));
    }
    line
}

/// The `network` report for the conduit at `pos`.
pub fn network_report(world: &World, pos: IVec3) -> String {
    let ports = world.resource::<crate::machine::MachinePorts>();
    let networks = world.resource::<crate::network::Networks>();
    let mut line =
        crate::network::describe(networks, world.resource::<Content>(), pos, |id, port| {
            ports.role(id, port)
        });
    let energy = networks.at(pos).and_then(|id| {
        let kind = networks.get(id)?.kind();
        let report = world.get_resource::<crate::network::energy::EnergyReport>()?;
        (world.resource::<Content>().networks[kind].solver == defaria_core::content::Solver::Energy)
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
