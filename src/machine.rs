//! Machines: blocks with a `machine` field (see "Machines" in
//! `docs/modding.md`), which take part in networks through their ports and
//! keep energy, fluid and items in their block entity.
//!
//! This module gives machines' block entities their parts, each from the
//! block's definition baked for the voxel's state ([`Content::machine`]):
//!
//! - [`Ports`]: the faces conduits join, each with its network kind, role
//!   and the network it's on now;
//! - [`EnergyBuffer`]: stored energy, capacity and rates;
//! - [`Tanks`]: named tanks of one fluid each;
//! - [`Slots`]: named groups of item slots;
//! - [`MachineActive`]: whether it's working, which puts the voxel in its
//!   block's `active_state`.
//!
//! It also keeps [`MachinePorts`]: for each network, its machine ports and
//! their roles, which the solvers (energy, fluid, items) iterate. Nothing
//! moves here; the solvers fill and drain the parts.

mod parts;
mod ports;

use bevy::prelude::*;

pub use parts::{
    EnergyBuffer, MachineActive, PortLink, Ports, SLOT_SIZE, SlotGroup, Slots, Tank, Tanks,
    slot_size,
};
pub use ports::{MachinePortRef, MachinePorts};

use crate::{
    block_entity::{BlockEntities, BlockEntity, BlockEntitySystems, RegisterBlockEntityPart},
    connections::ConnectionSystems,
    content::Content,
    network::NetworkSystems,
    terrain::{PropertiesUpdated, VoxelChanged, Voxels},
};

pub struct MachinePlugin;

impl Plugin for MachinePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MachinePorts>()
            .register_block_entity_part::<Ports>()
            .register_block_entity_part::<MachineActive>()
            .register_block_entity_part::<EnergyBuffer>()
            .register_block_entity_part::<Tanks>()
            .register_block_entity_part::<Slots>()
            .add_systems(
                Update,
                (
                    apply_active.before(ConnectionSystems),
                    follow_states.in_set(BlockEntitySystems),
                    ports::index_ports
                        .after(NetworkSystems)
                        .after(BlockEntitySystems),
                ),
            );
    }
}

/// Puts each machine whose [`MachineActive`] changed in its active or idle
/// state. An update of the block's own properties, not an edit, as the
/// engine's connection properties are.
fn apply_active(
    machines: Query<(&BlockEntity, &MachineActive), Changed<MachineActive>>,
    content: Res<Content>,
    mut voxels: Voxels,
) {
    for (entity, active) in &machines {
        let Some(state) = voxels.get(entity.pos) else {
            continue;
        };
        if let Some(machine) = content.machine(state) {
            voxels.update(entity.pos, machine.state(active.0));
        }
    }
}

/// Brings a machine's parts up to date when its voxel changes to another
/// state of the same block: its ports turn with it, and its sizes may
/// change with its materials. What it holds is kept.
fn follow_states(
    mut edits: MessageReader<VoxelChanged>,
    mut updates: MessageReader<PropertiesUpdated>,
    content: Res<Content>,
    index: Res<BlockEntities>,
    mut machines: Query<(
        Option<&mut Ports>,
        Option<&mut EnergyBuffer>,
        Option<&mut Tanks>,
    )>,
) {
    let changes = edits
        .read()
        .map(|edit| (edit.pos, edit.before, edit.after))
        .chain(
            updates
                .read()
                .map(|update| (update.pos, update.before, update.after)),
        );
    for (pos, before, after) in changes {
        if content.state(before).block != content.state(after).block {
            continue;
        }
        let (Some(machine), Some(entity)) = (content.machine(after), index.get(pos)) else {
            continue;
        };
        let Ok((ports, energy, tanks)) = machines.get_mut(entity) else {
            continue;
        };
        if let Some(mut ports) = ports {
            let mut new = ports.clone();
            new.follow(machine, &content);
            ports.set_if_neq(new);
        }
        if let (Some(mut energy), Some(spec)) = (energy, &machine.energy) {
            let mut new = *energy;
            new.follow(spec);
            energy.set_if_neq(new);
        }
        if let Some(mut tanks) = tanks {
            let mut new = tanks.clone();
            new.follow(&machine.tanks);
            tanks.set_if_neq(new);
        }
    }
}

#[cfg(test)]
mod tests;
