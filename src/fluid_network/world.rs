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
    machine::Tanks,
    network::energy::EnergyClock,
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
/// the tank takes, from the front first, then from the top down. Returns
/// how much it drew.
pub fn draw(
    voxels: &mut Voxels,
    content: &Content,
    pos: IVec3,
    want: u64,
    tank: &mut crate::machine::Tank,
) -> u64 {
    let mut want = want.min(tank.room());
    if want == 0 {
        return 0;
    }
    let open = |state: BlockState| state.is_air() || content.state(state).is_fluid();
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
    let Some(state) = reached.iter().find_map(|&at| {
        let state = voxels.get(at)?;
        (content.state(state).is_fluid() && tank.takes(&content.block_of(state).id))
            .then_some(state)
    }) else {
        return 0;
    };
    let fluid = content.block_of(state).id.clone();
    // The front first, then from the top down, so a pond drains as it
    // would: its surface falls.
    reached[1..].sort_by_key(|p| (-p.y, (*p - pos).abs().element_sum()));
    let mut drawn = 0;
    for at in reached {
        if want == 0 {
            break;
        }
        let Some((here, partial)) = voxels.get_with_fluid(at) else {
            continue;
        };
        if here != state {
            continue;
        }
        let amount = partial.unwrap_or(FULL_FLUID) as u64;
        let taken = amount.min(want);
        if taken == amount {
            voxels.set(at, BlockState::AIR);
        } else {
            voxels.set_fluid(at, (amount - taken) as u16);
        }
        want -= taken;
        drawn += taken;
    }
    tank.fill(&fluid, drawn);
    drawn
}

/// Pours up to `want` of `tank`'s fluid into the world at `pos`: into air,
/// or onto the same fluid, as much as fits in the voxel. Returns how much
/// it poured.
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
    let Some(state) = content.default_state(&fluid) else {
        return 0;
    };
    let Some((here, partial)) = voxels.get_with_fluid(pos) else {
        return 0;
    };
    let want = want.min(tank.amount);
    if here.is_air() {
        let amount = want.min(FULL_FLUID as u64);
        if amount > 0 && voxels.flow_into(pos, state, amount as u16) {
            return tank.drain(amount);
        }
    } else if here == state {
        let there = partial.unwrap_or(FULL_FLUID) as u64;
        let amount = want.min(FULL_FLUID as u64 - there);
        if amount > 0 && voxels.set_fluid(pos, (there + amount) as u16) {
            return tank.drain(amount);
        }
    }
    0
}

/// Takes up to `want` of the fluid in the voxel at `pos`, and no other,
/// into `tank`, if the tank takes it: as much as the voxel holds and the
/// tank has room for. A voxel emptied becomes air. Returns how much it
/// took.
pub fn take(
    voxels: &mut Voxels,
    content: &Content,
    pos: IVec3,
    want: u64,
    tank: &mut crate::machine::Tank,
) -> u64 {
    let Some((here, partial)) = voxels.get_with_fluid(pos) else {
        return 0;
    };
    if !content.state(here).is_fluid() {
        return 0;
    }
    let fluid = content.block_of(here).id.clone();
    if !tank.takes(&fluid) {
        return 0;
    }
    let amount = partial.unwrap_or(FULL_FLUID) as u64;
    let taken = amount.min(want).min(tank.room());
    if taken == 0 {
        return 0;
    }
    let changed = if taken == amount {
        voxels.set(pos, BlockState::AIR)
    } else {
        voxels.set_fluid(pos, (amount - taken) as u16)
    };
    if !changed {
        return 0;
    }
    tank.fill(&fluid, taken)
}

/// Runs the pumps and outlets for this network tick.
pub(super) fn run_pumps_and_outlets(
    clock: Res<EnergyClock>,
    content: Res<Content>,
    mut machines: Query<(&BlockEntity, &mut Tanks)>,
    mut voxels: Voxels,
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
            draw(&mut voxels, &content, target, want, &mut tank);
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
