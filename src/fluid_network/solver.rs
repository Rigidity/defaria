//! The fluid solver: moves fluid between machines' tanks and the network
//! they're on, each network tick, for every running fluid network.
//!
//! A network is one shared tank ([`FluidNet`]) as big as its pipes
//! together. Each tick, in order:
//!
//! 1. `output` ports (a pump) give their tank's fluid to the network, as
//!    much as there's room for;
//! 2. `input` ports (an outlet) take from it, as much as their tanks hold;
//! 3. if they still want more, `store` ports (a tank) give to the network
//!    and the inputs take again; otherwise the stores take what's left in
//!    the pipes.
//!
//! What goes in and what comes out in a tick are each limited by the
//! network's throughput: its slowest pipe's, in blocks a second. A network
//! takes in only the fluid it holds (any, when empty), and none at all
//! while it holds two (see [`FluidNet`]), and never a fluid hotter than its
//! coolest pipe's `max_temperature`.

use bevy::{platform::collections::HashMap, prelude::*};

use super::FluidNet;
use crate::{
    content::{BlockState, Content, FULL_FLUID, PortRole, Solver},
    machine::{MachinePortRef, MachinePorts, Tanks},
    network::{Network, NetworkId, NetworkMap, Networks, energy::EnergyClock},
    terrain::{ChunkCoord, ChunkMap},
    voxel::Chunk,
};

/// What a fluid network's pipes allow: worked out from them when needed,
/// and kept until the networks change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// The slowest pipe's throughput, in blocks of fluid a second.
    pub throughput: f64,
    /// The coolest pipe's `max_temperature`, if any pipe has one.
    pub max_temperature: Option<f32>,
}

/// Each fluid network's [`Limits`]: forgotten whenever the networks
/// change, so it holds only networks there are.
#[derive(Resource, Debug, Default)]
pub struct FluidLimits(HashMap<NetworkId, Limits>);

impl FluidLimits {
    /// The limits worked out for network `id`, if they're kept.
    pub fn known(&self, id: NetworkId) -> Option<Limits> {
        self.0.get(&id).copied()
    }

    /// The limits of `network`, worked out if they aren't kept (they're
    /// forgotten whenever the networks change).
    pub fn of(
        &mut self,
        id: NetworkId,
        network: &Network,
        content: &Content,
        world: impl Fn(IVec3) -> Option<BlockState>,
    ) -> Limits {
        if let Some(limits) = self.0.get(&id) {
            return *limits;
        }
        let mut limits = Limits {
            throughput: f64::INFINITY,
            max_temperature: None,
        };
        for &pos in network.conduits() {
            let Some(state) = world(pos) else {
                continue;
            };
            if let Some(conduit) = content.conduit(state) {
                limits.throughput = limits.throughput.min(conduit.throughput as f64);
            }
            if let Some(max) = content.state(state).stat("max_temperature") {
                limits.max_temperature =
                    Some(limits.max_temperature.map_or(max, |was: f32| was.min(max)));
            }
        }
        if !limits.throughput.is_finite() {
            limits.throughput = 0.0;
        }
        self.0.insert(id, limits);
        limits
    }
}

/// How many whole fluid units a rate of `blocks_per_second` moves on the
/// current network tick: the ticks of each second add up to the rate
/// exactly (see [`EnergyClock::share`]).
pub fn units_this_tick(clock: &EnergyClock, blocks_per_second: f64) -> u64 {
    if blocks_per_second <= 0.0 || !blocks_per_second.is_finite() {
        return 0;
    }
    clock.share((blocks_per_second * FULL_FLUID as f64).floor() as u64)
}

/// The temperature of the fluid block `fluid`, if it has one.
pub fn temperature(content: &Content, fluid: &str) -> Option<f32> {
    content
        .default_state(fluid)
        .and_then(|state| content.state(state).stat("temperature"))
}

/// One step of one network: the ports are its machine ports, with each
/// one's tank.
pub struct Step<'a> {
    pub net: &'a mut FluidNet,
    /// How much it holds at most, in fluid units.
    pub capacity: u64,
    /// The most that goes in, and the most that comes out, this step.
    pub budget_in: u64,
    pub budget_out: u64,
    /// Whether it may take a fluid in, by its block id.
    pub carries: &'a dyn Fn(&str) -> bool,
}

impl Step<'_> {
    fn room(&self) -> u64 {
        self.capacity.saturating_sub(self.net.total())
    }

    /// Takes what it can from `tank` into the network.
    fn take_from(&mut self, tank: &mut crate::machine::Tank) {
        let Some(fluid) = tank.fluid.clone() else {
            return;
        };
        if !self.net.takes(&fluid) || !(self.carries)(&fluid) {
            return;
        }
        let amount = tank.amount.min(self.room()).min(self.budget_in);
        let moved = tank.drain(amount);
        self.net.add(&fluid, moved);
        self.budget_in -= moved;
    }

    /// Gives what it can from the network into `tank`.
    fn give_to(&mut self, tank: &mut crate::machine::Tank) {
        let held: Vec<String> = self.net.held.iter().map(|h| h.fluid.clone()).collect();
        for fluid in held {
            if self.budget_out == 0 {
                return;
            }
            if !tank.takes(&fluid) {
                continue;
            }
            let amount = self.net.amount(&fluid).min(self.budget_out);
            let moved = tank.fill(&fluid, amount);
            self.net.take(&fluid, moved);
            self.budget_out -= moved;
        }
    }

    /// Moves fluid between the network and `ports`, by role, as the module
    /// docs say. `tank` gives each port's tank.
    pub fn run(&mut self, ports: &[(PortRole, usize)], tanks: &mut [&mut crate::machine::Tank]) {
        for &(role, i) in ports {
            if role == PortRole::Output {
                self.take_from(tanks[i]);
            }
        }
        let inputs: Vec<usize> = ports
            .iter()
            .filter(|(role, _)| *role == PortRole::Input)
            .map(|&(_, i)| i)
            .collect();
        let stores: Vec<usize> = ports
            .iter()
            .filter(|(role, _)| *role == PortRole::Store)
            .map(|&(_, i)| i)
            .collect();
        for &i in &inputs {
            self.give_to(tanks[i]);
        }
        let hungry = self.budget_out > 0
            && inputs
                .iter()
                .any(|&i| tanks[i].room() > 0 && self.net.total() == 0);
        if hungry {
            for &i in &stores {
                self.take_from(tanks[i]);
            }
            for &i in &inputs {
                self.give_to(tanks[i]);
            }
        } else {
            for &i in &stores {
                self.give_to(tanks[i]);
            }
        }
    }
}

/// Runs every fluid network that isn't frozen and has machine ports, and
/// whose chunks are all still loaded.
pub(super) fn run_networks(
    clock: Res<EnergyClock>,
    networks: Res<Networks>,
    ports: Res<MachinePorts>,
    content: Res<Content>,
    mut nets: ResMut<NetworkMap<FluidNet>>,
    mut limits: ResMut<FluidLimits>,
    mut machines: Query<&mut Tanks>,
    map: Res<ChunkMap>,
    chunks: Query<&Chunk>,
) {
    // A pipe placed, broken or changed in place (another metal) may have
    // changed a network's limits without changing its size.
    if networks.is_changed() {
        limits.0.clear();
    }
    let world = |pos: IVec3| -> Option<BlockState> {
        let (coord, local) = ChunkCoord::split(pos);
        Some(chunks.get(map.get(coord)?).ok()?.at(local))
    };
    let mut ids: Vec<(NetworkId, &[MachinePortRef])> = ports.iter().collect();
    ids.sort_by_key(|(id, _)| *id);
    for (id, on) in ids {
        let Some(network) = networks.get(id) else {
            continue;
        };
        if network.is_frozen() || content.networks[network.kind()].solver != Solver::Fluid {
            continue;
        }
        // Pipes whose chunk unloaded last frame are on it until the
        // networks next update; their shares are saved in them already, so
        // it holds less than it counts them for. It waits a tick.
        if network
            .chunks()
            .any(|chunk| map.get(ChunkCoord(chunk)).is_none())
        {
            continue;
        }
        let limit = limits.of(id, network, &content, world);
        let budget = units_this_tick(&clock, limit.throughput);
        if budget == 0 {
            continue;
        }
        // Each machine's tank once, however many of its faces are joined.
        let mut seen: Vec<(Entity, usize, PortRole)> = Vec::new();
        for port in on {
            let (Some(entity), Some(tank)) = (port.entity, port.tank) else {
                continue;
            };
            if !seen.iter().any(|&(e, t, _)| e == entity && t == tank) {
                seen.push((entity, tank, port.role));
            }
        }
        let mut taken: Vec<(Entity, usize, crate::machine::Tank)> = Vec::new();
        for &(entity, tank, _) in &seen {
            if let Ok(tanks) = machines.get(entity)
                && let Some(tank_now) = tanks.0.get(tank)
            {
                taken.push((entity, tank, tank_now.clone()));
            }
        }
        if taken.is_empty() {
            continue;
        }
        let roles: Vec<(PortRole, usize)> = seen
            .iter()
            .filter_map(|&(entity, tank, role)| {
                let i = taken
                    .iter()
                    .position(|&(e, t, _)| e == entity && t == tank)?;
                Some((role, i))
            })
            .collect();
        let max_temperature = limit.max_temperature;
        let carries = |fluid: &str| match (max_temperature, temperature(&content, fluid)) {
            (Some(max), Some(hot)) => hot <= max,
            _ => true,
        };
        let net = nets.get_mut(id);
        let mut step = Step {
            net,
            capacity: (network.capacity() * FULL_FLUID as f64).floor() as u64,
            budget_in: budget,
            budget_out: budget,
            carries: &carries,
        };
        let mut tanks: Vec<&mut crate::machine::Tank> =
            taken.iter_mut().map(|(_, _, tank)| tank).collect();
        step.run(&roles, &mut tanks);
        for (entity, index, tank) in taken {
            if let Ok(mut tanks) = machines.get_mut(entity)
                && tanks.0[index] != tank
            {
                tanks.0[index] = tank;
            }
        }
    }
}
