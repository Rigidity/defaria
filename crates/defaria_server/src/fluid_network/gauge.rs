//! Tanks that show how full they are: a machine whose block has a `level`
//! property (values `0` to some number) has it set to how full its first
//! tank is, rounded up so any fluid shows, and one with a `fluid`
//! property to the fluid it holds, where that's one of its values (the
//! fluid block's name without its mod). The block draws each state as it
//! likes.

use bevy::prelude::*;

use crate::block_entity::BlockEntity;
use defaria_core::{
    content::{BlockState, Content},
    machine::{Tank, Tanks},
    world::Voxels,
};

/// The state of `state`'s block showing `tank`, or `state` itself if its
/// block has nothing to show it with.
pub fn showing(content: &Content, state: BlockState, tank: &Tank) -> BlockState {
    let block = content.block_of(state);
    let mut values: Vec<(String, String)> = content.state(state).values.clone();
    let mut changed = false;
    if let Some(level) = block.properties.iter().find(|p| p.name == "level") {
        let top = level
            .values
            .iter()
            .filter_map(|v| v.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        let shown = if tank.capacity == 0 || tank.amount == 0 {
            0
        } else {
            (tank.amount * top).div_ceil(tank.capacity).min(top)
        };
        let shown = shown.to_string();
        if level.values.contains(&shown)
            && let Some(value) = values.iter_mut().find(|(n, _)| n == "level")
        {
            changed |= value.1 != shown;
            value.1 = shown;
        }
    }
    if let (Some(property), Some(fluid)) = (
        block.properties.iter().find(|p| p.name == "fluid"),
        tank.fluid.as_deref(),
    ) {
        let name = fluid.rsplit(':').next().unwrap_or(fluid).to_string();
        if property.values.contains(&name)
            && let Some(value) = values.iter_mut().find(|(n, _)| n == "fluid")
        {
            changed |= value.1 != name;
            value.1 = name;
        }
    }
    if !changed {
        return state;
    }
    let pairs: Vec<(&str, &str)> = values
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    block.state(&pairs).unwrap_or(state)
}

/// Puts each machine whose tanks changed in the state that shows them.
pub(super) fn show_levels(
    machines: Query<(&BlockEntity, &Tanks), Changed<Tanks>>,
    content: Res<Content>,
    mut voxels: Voxels,
) {
    for (entity, tanks) in &machines {
        let (Some(state), Some(tank)) = (voxels.get(entity.pos), tanks.0.first()) else {
            continue;
        };
        let shown = showing(&content, state, tank);
        if shown != state {
            voxels.update(entity.pos, shown);
        }
    }
}
