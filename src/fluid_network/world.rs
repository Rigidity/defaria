//! Machines that move fluid between their tank and the world: pumps draw
//! it from the fluid in front of them, outlets pour it out in front of
//! them. Both go through [`Voxels`], so flow wakes around what they change,
//! and nothing is made or lost.
//!
//! A machine is a pump if its block has a `pump_rate` stat, and an outlet
//! if it has an `outlet_rate`, in blocks of fluid a second. Its front is
//! its `facing` property, and it fills or drains its first tank.

use std::collections::VecDeque;

use bevy::{platform::collections::HashSet, prelude::*};

use super::solver::units_this_tick;
use crate::{
    block_entity::BlockEntity,
    content::{BlockState, Content, Direction, FULL_FLUID},
    fluid::{FluidSim, VoxelCells, reservoir::Reservoirs},
    machine::Tanks,
    network::NetworkClock,
    world::Voxels,
};

/// How many voxels a pump looks through, from the one in front of it, for
/// fluid to draw: it reaches across the bed of the pond it's draining, so
/// it draws the thin film flow leaves there, which doesn't run to it.
pub const PUMP_REACH: usize = 64;

/// How far (along the axes, added up) from the voxel in front of it a pump
/// reaches.
pub const PUMP_DISTANCE: i32 = 8;

/// The side the block state's `facing` property names.
pub fn front(content: &Content, state: BlockState) -> Option<Direction> {
    let (_, value) = content
        .state(state)
        .values
        .iter()
        .find(|(name, _)| name == "facing")?;
    Direction::ALL
        .into_iter()
        .find(|side| side.name() == value.as_str())
}

/// Draws up to `want` of fluid from the world into `tank`: from the voxel
/// at `pos` (in front of the pump), and from the fluid reached from it
/// through fluid and air, no higher than it, up to [`PUMP_REACH`] voxels
/// and [`PUMP_DISTANCE`] away. It draws one fluid, the first it finds that
/// the tank takes. From a calm body big enough to be a reservoir, it draws
/// from the reservoir's level, lowering the whole surface evenly (see
/// [`crate::fluid::reservoir`]); otherwise from the front first, then from
/// the top down. Returns how much it drew.
pub fn draw(
    voxels: &mut Voxels,
    content: &Content,
    reservoirs: &mut Reservoirs,
    pos: IVec3,
    want: u64,
    tank: &mut crate::machine::Tank,
) -> u64 {
    let mut want = want.min(tank.room());
    if want == 0 {
        return 0;
    }
    // Fluid reaches through any voxel with room for it.
    let open = |state: BlockState| content.state(state).fluid_room > 0;
    if !voxels.get(pos).is_some_and(open) {
        return 0;
    }
    // Everything reachable, nearest first.
    let mut reached = vec![pos];
    let mut seen = HashSet::from([pos]);
    let mut queue = VecDeque::from([pos]);
    while let Some(at) = queue.pop_front() {
        for side in Direction::ALL {
            let next = at + side.normal();
            if reached.len() >= PUMP_REACH
                || next.y > pos.y
                || (next - pos).abs().element_sum() > PUMP_DISTANCE
                || !seen.insert(next)
            {
                continue;
            }
            if voxels.get(next).is_some_and(open) {
                reached.push(next);
                queue.push_back(next);
            }
        }
    }
    let Some((first, kind)) = reached.iter().find_map(|&at| {
        let (_, fluid) = voxels.get_with_fluid(at)?;
        let kind = fluid?.kind;
        tank.takes(&content.block_of(kind).id).then_some((at, kind))
    }) else {
        return 0;
    };
    let fluid = content.block_of(kind).id.clone();
    if let Some(flow) = content.state(kind).fluid.as_ref() {
        let cells = VoxelCells {
            voxels: &*voxels,
            content,
        };
        if let Some(drawn) = reservoirs.draw(&cells, flow, first, want) {
            return tank.fill(&fluid, drawn);
        }
    }
    // The front first, then from the top down, so a pond drains as it
    // would: its surface falls.
    reached[1..].sort_by_key(|p| (-p.y, (*p - pos).abs().element_sum()));
    let mut drawn = 0;
    for at in reached {
        if want == 0 {
            break;
        }
        let here = voxels.get_with_fluid(at).and_then(|(_, fluid)| fluid);
        if here.is_none_or(|here| here.kind != kind) {
            continue;
        }
        let taken = voxels
            .drain(at, want.min(FULL_FLUID as u64) as u16)
            .map_or(0, |taken| taken.amount as u64);
        want -= taken;
        drawn += taken;
    }
    tank.fill(&fluid, drawn);
    drawn
}

/// Pours up to `want` of `tank`'s fluid into the world at `pos`: into
/// the room its block leaves (see `fluid_room`), alone or onto the same
/// fluid, as much as fits in the voxel. Returns how much it poured.
pub fn pour(
    voxels: &mut Voxels,
    content: &Content,
    pos: IVec3,
    want: u64,
    tank: &mut crate::machine::Tank,
) -> u64 {
    let Some(fluid) = tank.fluid.clone() else {
        return 0;
    };
    let Some(kind) = content.default_state(&fluid) else {
        return 0;
    };
    let want = want.min(tank.amount).min(FULL_FLUID as u64);
    let poured = voxels.pour(pos, kind, want as u16) as u64;
    if poured == 0 {
        return 0;
    }
    tank.drain(poured)
}

/// Takes up to `want` of the fluid in the voxel at `pos`, and no other,
/// into `tank`, if the tank takes it: as much as the voxel holds and the
/// tank has room for. Returns how much it took.
pub fn take(
    voxels: &mut Voxels,
    content: &Content,
    pos: IVec3,
    want: u64,
    tank: &mut crate::machine::Tank,
) -> u64 {
    let Some((_, Some(here))) = voxels.get_with_fluid(pos) else {
        return 0;
    };
    let fluid = content.block_of(here.kind).id.clone();
    if !tank.takes(&fluid) {
        return 0;
    }
    let taken = (here.amount as u64).min(want).min(tank.room());
    if taken == 0 {
        return 0;
    }
    let Some(taken) = voxels.drain(pos, taken as u16) else {
        return 0;
    };
    tank.fill(&fluid, taken.amount as u64)
}

/// Runs the pumps and outlets for this network tick.
pub(super) fn run_pumps_and_outlets(
    clock: Res<NetworkClock>,
    content: Res<Content>,
    mut machines: Query<(&BlockEntity, &mut Tanks)>,
    mut voxels: Voxels,
    mut sim: ResMut<FluidSim>,
) {
    for (block, mut tanks) in &mut machines {
        let Some(state) = voxels.get(block.pos) else {
            continue;
        };
        let info = content.state(state);
        let (pump, outlet) = (info.stat("pump_rate"), info.stat("outlet_rate"));
        if pump.is_none() && outlet.is_none() {
            continue;
        }
        let Some(side) = front(&content, state) else {
            continue;
        };
        let Some(tank) = tanks.0.first() else {
            continue;
        };
        let mut tank = tank.clone();
        let target = block.pos + side.normal();
        if let Some(rate) = pump {
            let want = units_this_tick(&clock, rate as f64);
            draw(
                &mut voxels,
                &content,
                &mut sim.sim.reservoirs,
                target,
                want,
                &mut tank,
            );
        }
        if let Some(rate) = outlet {
            let want = units_this_tick(&clock, rate as f64);
            pour(&mut voxels, &content, target, want, &mut tank);
        }
        if tanks.0[0] != tank {
            tanks.0[0] = tank;
        }
    }
}
