//! The item solver: tubes carry items from machines' slots to other
//! machines' slots, on networks whose kind has `solver: "item"`.
//!
//! Items move instantly (see `docs/decisions.md`): a network moves up to
//! its rate in items a second, the throughput of its slowest tube, and
//! nothing is ever in a tube. Each network tick (20 a second, the energy
//! solver's [`EnergyClock`], after it) it gets its share of that rate as
//! credit, and moves one item for each whole one:
//!
//! - from a port that gives ([`PortRole::gives`]: `output` and `store`)
//!   to one that takes (`input` and `store`), never between two `store`s
//!   (which would pass items back and forth for ever) or within one slot
//!   group;
//! - to `input`s before `store`s, so machines are fed first and storage
//!   takes what's left;
//! - round-robin: each move starts looking after the giver and the taker
//!   the last one used, so items spread across every port that takes them;
//! - through the ports' filters ([`ItemFilter`]): an output offers, and an
//!   input takes, only what passes its filter;
//! - into a recipe machine's input groups only what one of its recipes
//!   takes ([`Content::slot_group_takes`]), so a furnace never fills with
//!   glass. An item nothing takes stays where it is, and the next one is
//!   tried.
//!
//! A frozen network moves nothing. Items are only ever taken out of one
//! slot and put in another in the same step, so none are made or lost.
//!
//! Creative machines with `emits` ([`ItemSource`]) keep a slot group full
//! of an item, out of nothing, for networks to carry away.

use bevy::{platform::collections::HashMap, prelude::*};

use super::{
    NetworkAppExt, NetworkId, NetworkMap, NetworkState, NetworkSystems, Networks, Share,
    energy::{EnergyClock, EnergySystems},
};
use crate::{
    block_entity::{BlockEntityPart, RegisterBlockEntityPart, Value},
    content::{BlockState, Content, ItemFilter, Machine, PortRole, Solver},
    inventory::ItemStack,
    machine::{MachinePortRef, MachinePorts, SlotGroup, Slots},
    world::{Chunk, ChunkCoord, ChunkMap},
};

/// The most items one network moves in one tick, however fast its tubes.
pub const MAX_MOVES_PER_TICK: u32 = 64;

pub struct ItemNetworkPlugin;

impl Plugin for ItemNetworkPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_state::<ItemRouting>()
            .init_resource::<ItemRates>()
            .register_block_entity_part::<ItemSource>()
            .add_systems(Update, measure_rates.after(NetworkSystems))
            .add_systems(
                FixedUpdate,
                (move_items, emit_items)
                    .chain()
                    .after(EnergySystems)
                    // So items that arrive in a machine are there for its
                    // recipe on the same tick.
                    .before(crate::recipe::run_recipes)
                    .run_if(|clock: Res<EnergyClock>| clock.due)
                    .in_set(ItemSystems),
            );
    }
}

/// Where the item solver runs, in `FixedUpdate`: after the energy solver
/// and before recipe machines, on network ticks.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemSystems;

/// Where an item network's round-robin is, and the credit it has saved up
/// toward its next move. Nothing in it is an item, so merging keeps one
/// network's and splitting starts the parts afresh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemRouting {
    /// Items it may move, in thousandths, counted out a tick at a time as
    /// energy is ([`EnergyClock::share`]), so a rate holds exactly over
    /// each second.
    pub credit: u64,
    pub cursor: Cursor,
}

/// The round-robin: the giver and taker after which the next move starts
/// looking, as positions among a network's givers and takers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub giver: usize,
    pub taker: usize,
}

impl NetworkState for ItemRouting {
    fn merge(&mut self, other: Self) {
        self.credit = self.credit.max(other.credit);
    }

    fn split(self, parts: &[Share], _: Share) -> Vec<Self> {
        vec![Self::default(); parts.len()]
    }
}

/// Each item network's rate, in items a second: the throughput of its
/// slowest tube.
#[derive(Resource, Debug, Default)]
pub struct ItemRates(pub HashMap<NetworkId, f32>);

/// Works out the item networks' rates when the networks change.
fn measure_rates(
    networks: Res<Networks>,
    content: Res<Content>,
    map: Res<ChunkMap>,
    chunks: Query<&Chunk>,
    mut rates: ResMut<ItemRates>,
) {
    if !networks.is_changed() {
        return;
    }
    let world = |pos: IVec3| -> Option<BlockState> {
        let (coord, local) = ChunkCoord::split(pos);
        Some(chunks.get(map.get(coord)?).ok()?.at(local))
    };
    rates.0.clear();
    for (id, network) in networks.iter() {
        if content.networks[network.kind()].solver != Solver::Item {
            continue;
        }
        let rate = network
            .conduits()
            .iter()
            .filter_map(|&pos| content.conduit(world(pos)?))
            .map(|conduit| conduit.throughput)
            .reduce(f32::min);
        if let Some(rate) = rate {
            rates.0.insert(id, rate.max(0.0));
        }
    }
}

/// A creative machine's endless supply (its block's `emits`): its slot
/// group `slots` is kept full of `item`. Worked out from the block, so
/// not saved.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Eq, Default)]
#[reflect(Component)]
pub struct ItemSource {
    /// The item's id.
    pub item: String,
    /// The slot group, an index into the machine's [`Slots`].
    pub slots: usize,
}

impl BlockEntityPart for ItemSource {
    const KEY: &'static str = "item_source";
    const SAVED: bool = false;

    fn new(state: BlockState, content: &Content) -> Option<Self> {
        let emits = content.machine(state)?.emits.as_ref()?;
        Some(Self {
            item: emits.item.clone(),
            slots: emits.slots,
        })
    }

    fn save(&self, _: &Content) -> Value {
        Value::Table(Default::default())
    }

    fn load(_: &Value, _: u32, state: BlockState, content: &Content) -> Option<Self> {
        Self::new(state, content)
    }
}

/// Tops up every [`ItemSource`]'s slot group.
fn emit_items(content: Res<Content>, mut sources: Query<(&ItemSource, &mut Slots)>) {
    for (source, mut slots) in &mut sources {
        let Some(item) = content.default_item(&source.item) else {
            continue;
        };
        let Some(group) = slots.0.get(source.slots) else {
            continue;
        };
        let room = group.room_for(&content, &ItemStack::one(item));
        if room > 0 {
            slots.0[source.slots].insert(&content, ItemStack::new(item, room));
        }
    }
}

/// Moves items along every running item network, as far as its credit
/// goes.
fn move_items(
    content: Res<Content>,
    clock: Res<EnergyClock>,
    networks: Res<Networks>,
    ports: Res<MachinePorts>,
    rates: Res<ItemRates>,
    mut routing: ResMut<NetworkMap<ItemRouting>>,
    mut slots: Query<&mut Slots>,
) {
    let mut ids: Vec<NetworkId> = ports.iter().map(|(id, _)| id).collect();
    ids.sort();
    for id in ids {
        let Some(network) = networks.get(id) else {
            continue;
        };
        if content.networks[network.kind()].solver != Solver::Item || network.is_frozen() {
            continue;
        }
        let Some(&rate) = rates.0.get(&id) else {
            continue;
        };
        let state = routing.get_mut(id);
        let step = clock.share((rate as f64 * 1000.0).round() as u64);
        // Saving up no more than a tick and one item, so a network that
        // waited doesn't then move a burst.
        state.credit = (state.credit + step).min(step + 1000);
        let budget = ((state.credit / 1000) as u32).min(MAX_MOVES_PER_TICK);
        if budget == 0 {
            continue;
        }
        let (keys, mut groups, endpoints) = gather(&content, ports.on(id), &slots);
        let moved = route(&content, &mut groups, &endpoints, &mut state.cursor, budget);
        state.credit -= moved as u64 * 1000;
        if moved == 0 {
            continue;
        }
        for ((entity, index), group) in keys.into_iter().zip(groups) {
            if let Ok(mut slots) = slots.get_mut(entity)
                && slots.0[index] != group
            {
                slots.0[index] = group;
            }
        }
    }
}

/// One machine port on an item network, for [`route`].
#[derive(Debug, Clone, Copy)]
pub struct Endpoint<'a> {
    /// Its slot group: an index into the groups `route` is given.
    pub group: usize,
    pub role: PortRole,
    pub filter: Option<&'a ItemFilter>,
    /// Its machine and the slot group's index in it, for what the group
    /// takes ([`Content::slot_group_takes`]: a recipe machine's input
    /// groups take only what one of its recipes takes).
    pub machine: Option<(&'a Machine, usize)>,
}

/// Copies the slot groups a network's ports use (each once, however many
/// ports share it), with the ports as [`Endpoint`]s on them.
fn gather<'a>(
    content: &'a Content,
    ports: &[MachinePortRef],
    slots: &Query<&mut Slots>,
) -> (Vec<(Entity, usize)>, Vec<SlotGroup>, Vec<Endpoint<'a>>) {
    let mut keys: Vec<(Entity, usize)> = Vec::new();
    let mut groups = Vec::new();
    let mut endpoints = Vec::new();
    for port in ports {
        let (Some(entity), Some(index)) = (port.entity, port.slots) else {
            continue;
        };
        let group = match keys.iter().position(|&key| key == (entity, index)) {
            Some(group) => group,
            None => {
                let Some(group) = slots.get(entity).ok().and_then(|s| s.0.get(index)) else {
                    continue;
                };
                keys.push((entity, index));
                groups.push(group.clone());
                groups.len() - 1
            }
        };
        let machine = content.machine(port.state);
        let filter = port
            .filter
            .and_then(|filter| machine.and_then(|machine| machine.filters.get(filter)));
        endpoints.push(Endpoint {
            group,
            role: port.role,
            filter,
            machine: machine.map(|machine| (machine, index)),
        });
    }
    (keys, groups, endpoints)
}

/// Moves up to `budget` items, one at a time, between `groups` through
/// `endpoints` (see the module's docs for which way), and returns how many
/// it moved: fewer when nothing more can go anywhere.
pub fn route(
    content: &Content,
    groups: &mut [SlotGroup],
    endpoints: &[Endpoint],
    cursor: &mut Cursor,
    budget: u32,
) -> u32 {
    let givers: Vec<usize> = (0..endpoints.len())
        .filter(|&i| endpoints[i].role.gives())
        .collect();
    let takers: Vec<usize> = (0..endpoints.len())
        .filter(|&i| endpoints[i].role.takes())
        .collect();
    if givers.is_empty() || takers.is_empty() {
        return 0;
    }
    let passes = |endpoint: &Endpoint, stack: &ItemStack| {
        endpoint
            .filter
            .is_none_or(|filter| filter.passes(content, stack.item))
    };
    let mut moved = 0;
    'moves: while moved < budget {
        for g in 0..givers.len() {
            let g = (cursor.giver + g) % givers.len();
            let giver = &endpoints[givers[g]];
            for slot in 0..groups[giver.group].slots.len() {
                let Some(stack) = &groups[giver.group].slots[slot] else {
                    continue;
                };
                let one = ItemStack {
                    count: 1,
                    ..stack.clone()
                };
                if !passes(giver, &one) {
                    continue;
                }
                // Inputs first, then stores.
                for store in [false, true] {
                    for t in 0..takers.len() {
                        let t = (cursor.taker + t) % takers.len();
                        let taker = &endpoints[takers[t]];
                        if (taker.role == PortRole::Store) != store
                            || taker.group == giver.group
                            || (giver.role == PortRole::Store && taker.role == PortRole::Store)
                            || !passes(taker, &one)
                            || taker.machine.is_some_and(|(machine, index)| {
                                !content.slot_group_takes(machine, index, one.item)
                            })
                            || groups[taker.group].room_for(content, &one) == 0
                        {
                            continue;
                        }
                        let taken = groups[giver.group].take(slot, 1).expect("the slot has one");
                        let left = groups[taker.group].insert(content, taken);
                        debug_assert!(left.is_none(), "it had room");
                        moved += 1;
                        cursor.giver = (g + 1) % givers.len();
                        cursor.taker = (t + 1) % takers.len();
                        continue 'moves;
                    }
                }
            }
        }
        break;
    }
    moved
}

#[cfg(test)]
mod tests;
