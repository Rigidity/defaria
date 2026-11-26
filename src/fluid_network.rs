//! Fluid networks: pipes carrying fluid between machines, and the pumps
//! and outlets that join them to the world's fluid (#113).
//!
//! Each network run by the `fluid` solver is one shared tank, a
//! [`FluidNet`] in a [`NetworkMap`], as big as its pipes' `capacity`
//! together, merged and split by volume as the network is (see
//! [`network`](crate::network)). The [`solver`] moves fluid between it and
//! the tanks of machines on it, up to its pipes' throughput; [`world`] has
//! pumps draw from the world and outlets pour into it; [`gauge`] shows how
//! full tanks are. All of that runs in [`FixedUpdate`], on network ticks
//! (20 Hz, [`NetworkClock`](crate::network::NetworkClock)), after the
//! energy solver.
//!
//! Nothing is made or lost (only fluid poured into a voxel that a block
//! then takes, as in the world): a broken pipe spills its share where it
//! was, and the share of a pipe whose chunk unloads, or is saved, goes in
//! its block entity ([`PipeFluid`]) and comes back to its network when it
//! loads ([`pipes`]). See "Fluid networks" in `docs/modding.md` and
//! `docs/decisions.md`.

mod contents;
pub mod gauge;
pub mod pipes;
pub mod solver;
pub mod world;

use bevy::prelude::*;

pub use contents::{FluidAmount, FluidNet};
pub use pipes::{PipeFluid, held_in_pipes};

use crate::{
    block_entity::RegisterBlockEntityPart,
    connections::ConnectionSystems,
    network::{
        NetworkAppExt, NetworkStateSystems, NetworkSystems,
        tick::{NetworkTick, on_network_tick},
    },
    streaming::{BeforeChunkSave, SaveOnExit},
};

pub struct FluidNetworkPlugin;

impl Plugin for FluidNetworkPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_state::<FluidNet>()
            .init_resource::<FluidNetworkStats>()
            .init_resource::<pipes::Reclaim>()
            .init_resource::<pipes::Spills>()
            .init_resource::<solver::FluidLimits>()
            .register_block_entity_part::<PipeFluid>()
            .add_observer(pipes::note_loaded_pipe)
            .add_systems(
                Update,
                (
                    // While the broken pipe is still on its network, and
                    // its block entity still indexed.
                    pipes::spill_broken_pipes
                        .after(ConnectionSystems)
                        .before(NetworkSystems)
                        .before(crate::block_entity::BlockEntitySystems),
                    pipes::pour_spills.after(pipes::spill_broken_pipes),
                    // Before the fluid states follow the split that ends
                    // a network.
                    pipes::count_ended_networks
                        .after(NetworkSystems)
                        .before(NetworkStateSystems),
                    pipes::record_before_save
                        .in_set(BeforeChunkSave)
                        .after(NetworkStateSystems),
                    gauge::show_levels.before(ConnectionSystems),
                ),
            )
            .add_systems(
                FixedUpdate,
                (
                    pipes::reclaim,
                    world::run_pumps_and_outlets,
                    solver::run_networks,
                )
                    .chain()
                    // So fluid that arrives in a machine's tank is there
                    // for its recipe on the same tick: recipes run in
                    // `NetworkTick::Machines`, after this.
                    .in_set(NetworkTick::Solve)
                    .run_if(on_network_tick)
                    .in_set(FluidSystems),
            )
            .add_systems(
                SaveOnExit,
                pipes::record_before_save.in_set(BeforeChunkSave),
            );
    }
}

/// Where the fluid solver runs, in `FixedUpdate`: in
/// [`NetworkTick::Solve`], after the energy solver and before recipe
/// machines, on network ticks.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct FluidSystems;

/// What fluid networks have done with fluid outside networks and tanks,
/// in fluid units, since the start.
#[derive(Resource, Reflect, Debug, Clone, Default, PartialEq)]
#[reflect(Resource)]
pub struct FluidNetworkStats {
    /// Poured into the world by pipes that were broken.
    pub spilled: u64,
    /// Lost by pipes broken where it couldn't be poured (into a block put
    /// in the pipe's place, or a second fluid), or held by a network whose
    /// pipes were all replaced by ones that didn't join it.
    pub lost: u64,
}

#[cfg(test)]
mod tests;
