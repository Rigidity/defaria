//! The `energy` solver: moves energy between the machines on each network
//! whose kind's solver is `energy` (the base game's electric network).
//!
//! Energy is counted in whole joules (J), rates in watts (J/s); see
//! `docs/decisions.md`. It lives only in machines' [`EnergyBuffer`]s:
//! wires hold none, so a network merging or splitting moves no energy.
//!
//! Each network tick ([`NetworkClock`], [`TICKS_PER_SECOND`] a second,
//! counted in `FixedUpdate`):
//!
//! 1. Machines that `generate` fill their buffers by that much.
//! 2. Each running network balances its machines' ports ([`balance`]):
//!    producers offer (up to their `max_out` and what they hold), consumers
//!    ask (up to their `max_in` and their room), storage does both.
//!    Consumers are served from producers first, then storage; what
//!    producers have left over charges storage. What consumers need to
//!    run this tick (their `use`, and any [`EnergyNeed`]), less what
//!    their buffers hold, is served before their buffers are topped up; in a shortage those needs are
//!    served whole, the smallest first ([`serve_smallest_first`]), so
//!    some run and the rest wait. Everything moved on a
//!    tick, to consumers and to storage, is capped by the network's
//!    throughput: its weakest conduit's. A frozen network is skipped, so
//!    its machines keep what they hold.
//! 3. Machines that `use` energy pay for the tick from their buffer, and
//!    are active ([`MachineActive`]) while they can.
//!
//! So energy enters the world only at generators and leaves only at
//! machines that use it. [`EnergyReport`] keeps what each network stored,
//! produced and consumed, for the headless `network` command.

use bevy::{
    platform::collections::{HashMap, HashSet},
    prelude::*,
};

use super::{
    NetworkId, NetworkSystems, Networks,
    tick::{NetworkClock, NetworkTick, TICKS_PER_SECOND, on_network_tick},
};
use crate::{
    content::{BlockState, Content, Solver},
    machine::{EnergyBuffer, MachineActive, MachinePorts},
    world::{Chunk, ChunkCoord, ChunkMap},
};

pub struct EnergyPlugin;

impl Plugin for EnergyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EnergyLimits>()
            .init_resource::<EnergyReport>()
            .add_systems(Update, find_limits.after(NetworkSystems))
            .add_systems(
                FixedUpdate,
                (generate, solve, use_energy)
                    .chain()
                    .run_if(on_network_tick)
                    .in_set(NetworkTick::Solve)
                    .in_set(EnergySystems),
            );
    }
}

/// Where the energy solver runs, in `FixedUpdate`: in
/// [`NetworkTick::Solve`], before the fluid and item solvers.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnergySystems;

/// Each energy network's throughput, in watts: its weakest conduit's.
#[derive(Resource, Debug, Default)]
pub struct EnergyLimits(pub HashMap<NetworkId, u64>);

/// Works out [`EnergyLimits`] again when the networks change.
fn find_limits(
    networks: Res<Networks>,
    content: Res<Content>,
    map: Res<ChunkMap>,
    chunks: Query<&Chunk>,
    mut limits: ResMut<EnergyLimits>,
) {
    if !networks.is_changed() {
        return;
    }
    let world = |pos: IVec3| -> Option<BlockState> {
        let (coord, local) = ChunkCoord::split(pos);
        Some(chunks.get(map.get(coord)?).ok()?.at(local))
    };
    limits.0.clear();
    for (id, network) in networks.iter() {
        if content.networks[network.kind()].solver != Solver::Energy {
            continue;
        }
        let weakest = network
            .conduits()
            .iter()
            .filter_map(|&pos| content.conduit(world(pos)?))
            .map(|conduit| conduit.throughput)
            .fold(f32::INFINITY, f32::min);
        let limit = if weakest.is_finite() {
            weakest.max(0.0).floor() as u64
        } else {
            0
        };
        limits.0.insert(id, limit);
    }
}

/// What a machine does on a network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Gives energy: a generator.
    Producer,
    /// Takes energy: a lamp.
    Consumer,
    /// Takes the network's surplus and covers its shortage: a battery.
    Storage,
}

/// One machine on a network, for a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Member {
    pub role: Role,
    /// The most it can give this tick (0 for a consumer).
    pub offer: u64,
    /// The most it can take this tick (0 for a producer).
    pub demand: u64,
    /// Of a consumer's demand, what it needs this tick to keep running
    /// and doesn't hold already: served before any consumer's buffer is
    /// topped up.
    pub need: u64,
}

/// What a network tick moved: each member's `given` and `taken`, in order,
/// and the totals.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Flow {
    pub given: Vec<u64>,
    pub taken: Vec<u64>,
    /// Given by producers.
    pub produced: u64,
    /// Taken by consumers.
    pub consumed: u64,
    /// Taken by storage.
    pub charged: u64,
    /// Given by storage.
    pub discharged: u64,
}

/// Balances one network for a tick, moving at most `limit` in all.
/// Everything given is taken: `produced + discharged == consumed +
/// charged`.
pub fn balance(members: &[Member], limit: u64) -> Flow {
    let of = |role: Role, field: fn(&Member) -> u64| -> Vec<u64> {
        members
            .iter()
            .map(|m| if m.role == role { field(m) } else { 0 })
            .collect()
    };
    let producer_offers = of(Role::Producer, |m| m.offer);
    let consumer_demands = of(Role::Consumer, |m| m.demand);
    let consumer_needs = of(Role::Consumer, |m| m.need.min(m.demand));
    let storage_offers = of(Role::Storage, |m| m.offer);
    let storage_demands = of(Role::Storage, |m| m.demand);
    let supply: u64 = producer_offers.iter().sum();
    let demand: u64 = consumer_demands.iter().sum();
    let reserve: u64 = storage_offers.iter().sum();
    let room: u64 = storage_demands.iter().sum();

    let consumed = demand.min(supply.saturating_add(reserve)).min(limit);
    let from_producers = supply.min(consumed);
    let discharged = consumed - from_producers;
    let charged = (supply - from_producers).min(room).min(limit - consumed);
    let produced = from_producers + charged;

    let mut given = share_out(produced, &producer_offers);
    // Needs first, whole and the smallest first; then what's left tops
    // buffers up, in proportion to the room each has left.
    let needed: u64 = consumer_needs.iter().sum();
    let mut taken = serve_smallest_first(consumed.min(needed), &consumer_needs);
    let top_ups: Vec<u64> = consumer_demands
        .iter()
        .zip(&taken)
        .map(|(demand, served)| demand - served)
        .collect();
    for (t, s) in taken
        .iter_mut()
        .zip(share_out(consumed - consumed.min(needed), &top_ups))
    {
        *t += s;
    }
    for (g, s) in given.iter_mut().zip(share_out(discharged, &storage_offers)) {
        *g += s;
    }
    for (t, s) in taken.iter_mut().zip(share_out(charged, &storage_demands)) {
        *t += s;
    }
    Flow {
        given,
        taken,
        produced,
        consumed,
        charged,
        discharged,
    }
}

/// Serves `total` out to `demands` whole, the smallest first (the first of
/// equal ones first), until it runs out: the one it runs out on gets what's
/// left. `total` must be at most the demands' sum. A shortage leaves some
/// consumers running and the rest waiting, rather than every one short and
/// none able to pay for its tick.
pub fn serve_smallest_first(total: u64, demands: &[u64]) -> Vec<u64> {
    let mut order: Vec<usize> = (0..demands.len()).collect();
    order.sort_by_key(|&i| (demands[i], i));
    let mut out = vec![0; demands.len()];
    let mut left = total;
    for i in order {
        let served = demands[i].min(left);
        out[i] = served;
        left -= served;
    }
    debug_assert_eq!(left, 0, "serving {total} over demands of {demands:?}");
    out
}

/// Shares `total` out in proportion to `caps`, in whole units, none more
/// than its cap, adding up to `total` exactly (which must be at most the
/// caps' sum): each rounded down, then one more to those with the largest
/// remainders.
pub fn share_out(total: u64, caps: &[u64]) -> Vec<u64> {
    let sum: u128 = caps.iter().map(|&c| c as u128).sum();
    if total == 0 || sum == 0 {
        return vec![0; caps.len()];
    }
    assert!(total as u128 <= sum, "sharing {total} over caps of {sum}");
    let total = total as u128;
    let mut out: Vec<u64> = caps
        .iter()
        .map(|&c| (total * c as u128 / sum) as u64)
        .collect();
    let mut left = total as u64 - out.iter().sum::<u64>();
    let mut order: Vec<(u128, usize)> = caps
        .iter()
        .enumerate()
        .map(|(i, &c)| (total * c as u128 % sum, i))
        .filter(|&(remainder, _)| remainder > 0)
        .collect();
    order.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, i) in order {
        if left == 0 {
            break;
        }
        out[i] += 1;
        left -= 1;
    }
    debug_assert_eq!(left, 0);
    out
}

/// What the energy networks did: each one's last second, and the world's
/// totals.
#[derive(Resource, Debug, Default)]
pub struct EnergyReport {
    pub networks: HashMap<NetworkId, NetworkEnergy>,
    /// All the energy generators have made.
    pub generated: u64,
    /// All the energy machines have used up.
    pub used: u64,
}

/// One energy network's report.
#[derive(Debug, Clone, Default)]
pub struct NetworkEnergy {
    /// What its storage holds, and could.
    pub stored: u64,
    pub capacity: u64,
    /// Its throughput, in watts.
    pub limit: u64,
    pub frozen: bool,
    /// Produced and consumed on each of the last second's ticks, by tick
    /// within the second.
    window: [(u64, u64); TICKS_PER_SECOND as usize],
}

impl NetworkEnergy {
    /// What producers gave in the last second.
    pub fn produced(&self) -> u64 {
        self.window.iter().map(|w| w.0).sum()
    }

    /// What consumers took in the last second.
    pub fn consumed(&self) -> u64 {
        self.window.iter().map(|w| w.1).sum()
    }
}

/// Energy a machine needs on the next network tick to keep running, on
/// top of its `use`: a running recipe's share, say. Set by whatever runs
/// the machine; the solver serves it before topping buffers up, so a
/// machine filling its buffer doesn't starve one that's working.
#[derive(Component, Reflect, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[reflect(Component)]
pub struct EnergyNeed(pub u64);

/// Fills machines that generate.
fn generate(
    clock: Res<NetworkClock>,
    mut buffers: Query<&mut EnergyBuffer>,
    mut report: ResMut<EnergyReport>,
) {
    for mut buffer in &mut buffers {
        if buffer.generate == 0 || buffer.room() == 0 {
            continue;
        }
        let due = clock.share(buffer.generate);
        let made = buffer.fill(due);
        report.generated += made;
    }
}

/// Balances every running energy network.
fn solve(
    clock: Res<NetworkClock>,
    networks: Res<Networks>,
    content: Res<Content>,
    ports: Res<MachinePorts>,
    limits: Res<EnergyLimits>,
    mut buffers: Query<&mut EnergyBuffer>,
    needs: Query<&EnergyNeed>,
    mut report: ResMut<EnergyReport>,
) {
    let slot = (clock.ticks.saturating_sub(1) % TICKS_PER_SECOND as u64) as usize;
    let mut ids: Vec<NetworkId> = networks
        .iter()
        .filter(|(_, n)| content.networks[n.kind()].solver == Solver::Energy)
        .map(|(id, _)| id)
        .collect();
    ids.sort();
    report.networks.retain(|id, _| networks.get(*id).is_some());
    // What each machine has given and taken this tick, over every network
    // it's on, so its rates hold across them all.
    let mut used: HashMap<Entity, (u64, u64)> = HashMap::new();
    for id in ids {
        let network = networks.get(id).unwrap();
        let limit = limits.0.get(&id).copied();
        // Each machine once, with every role its ports here have.
        let mut machines: Vec<(Entity, bool, bool)> = Vec::new();
        let mut seen = HashSet::new();
        for port in ports.on(id) {
            let Some(entity) = port.entity else { continue };
            if seen.insert(entity) {
                machines.push((entity, port.role.gives(), port.role.takes()));
            } else if let Some(m) = machines.iter_mut().find(|m| m.0 == entity) {
                m.1 |= port.role.gives();
                m.2 |= port.role.takes();
            }
        }
        machines.retain(|m| buffers.contains(m.0));
        let entry = report.networks.entry(id).or_default();
        entry.frozen = network.is_frozen();
        entry.limit = limit.unwrap_or(0);
        let (frozen, Some(limit)) = (network.is_frozen(), limit) else {
            entry.window[slot] = (0, 0);
            continue;
        };
        if frozen {
            entry.window[slot] = (0, 0);
        } else {
            let members: Vec<Member> = machines
                .iter()
                .map(|&(entity, gives, takes)| {
                    let buffer = buffers.get(entity).unwrap();
                    let (given, taken) = used.get(&entity).copied().unwrap_or_default();
                    let role = match (gives, takes) {
                        (true, true) => Role::Storage,
                        (true, false) => Role::Producer,
                        _ => Role::Consumer,
                    };
                    // What it can't pay for this tick from what it
                    // holds, so a machine that can't run on its own is
                    // served before one that can.
                    let need = (clock.share(buffer.uses)
                        + needs.get(entity).map_or(0, |need| need.0))
                    .saturating_sub(buffer.stored);
                    Member {
                        role,
                        need,
                        offer: if gives {
                            clock
                                .share(buffer.max_out)
                                .saturating_sub(given)
                                .min(buffer.stored)
                        } else {
                            0
                        },
                        demand: if takes {
                            clock
                                .share(buffer.max_in)
                                .saturating_sub(taken)
                                .min(buffer.room())
                        } else {
                            0
                        },
                    }
                })
                .collect();
            let flow = balance(&members, clock.share(limit));
            for (i, &(entity, _, _)) in machines.iter().enumerate() {
                let (give, take) = (flow.given[i], flow.taken[i]);
                if give == 0 && take == 0 {
                    continue;
                }
                let mut buffer = buffers.get_mut(entity).unwrap();
                let drained = buffer.drain(give);
                let filled = buffer.fill(take);
                debug_assert_eq!((drained, filled), (give, take));
                let entry = used.entry(entity).or_default();
                entry.0 += give;
                entry.1 += take;
            }
            entry.window[slot] = (flow.produced, flow.consumed);
        }
        entry.stored = 0;
        entry.capacity = 0;
        for &(entity, gives, takes) in &machines {
            if gives && takes {
                let buffer = buffers.get(entity).unwrap();
                entry.stored += buffer.stored;
                entry.capacity += buffer.capacity;
            }
        }
    }
}

/// Machines that use energy pay for the tick, and work while they can.
fn use_energy(
    clock: Res<NetworkClock>,
    mut machines: Query<(&mut EnergyBuffer, Option<&mut MachineActive>)>,
    mut report: ResMut<EnergyReport>,
) {
    for (mut buffer, active) in &mut machines {
        if buffer.uses == 0 {
            continue;
        }
        let due = clock.share(buffer.uses);
        if due == 0 {
            // Nothing owed on this tick: it stays as it was.
            continue;
        }
        let paid = buffer.stored >= due;
        if paid {
            buffer.drain(due);
            report.used += due;
        }
        if let Some(mut active) = active {
            active.set_if_neq(MachineActive(paid));
        }
    }
}

/// The energy part of the headless `network` line, for an energy network.
/// One the solver hasn't run yet (just split off, or placed this tick)
/// reads as nothing moved.
pub fn describe(report: &EnergyReport, id: NetworkId) -> String {
    let fresh = NetworkEnergy::default();
    let energy = report.networks.get(&id).unwrap_or(&fresh);
    let mut line = format!(
        "energy: stored {} of {} J, produced {} J and consumed {} J in the last second, throughput {} W",
        energy.stored,
        energy.capacity,
        energy.produced(),
        energy.consumed(),
        energy.limit,
    );
    if energy.frozen {
        line += ", frozen";
    }
    line
}

#[cfg(test)]
mod tests;
