//! Keeping what pipes hold through everything that isn't the solver: a
//! pipe broken spills its share into the world, and a pipe's share is
//! written into its block entity ([`PipeFluid`]) just before its chunk is
//! saved, and taken back into its network when its chunk loads.
//!
//! A network's fluid lives in its [`FluidNet`], not in its pipes, so each
//! pipe's share is worked out when it's needed: its network's fluid divided
//! by volume, exactly (see [`FluidNet::take_shares`]). When a chunk
//! unloads, its pipes' shares are taken out of their networks and saved in
//! them; when the game exits or autosaves, every pipe gets a copy of its
//! share, which the network keeps. A pipe loaded with fluid in it gives it
//! to its network (and is emptied) as soon as the network is known.

use bevy::{
    platform::collections::{HashMap, HashSet},
    prelude::*,
};

use super::{FluidNet, FluidNetworkStats, contents::FluidAmount};
use crate::{
    block_entity::{
        BlockEntities, BlockEntity, BlockEntityChanged, BlockEntityPart, SavedPart, SavedParts,
        Value,
    },
    content::{BlockState, Content, FULL_FLUID, Solver},
    network::{NetworkChanged, NetworkId, NetworkMap, Networks},
    streaming::ChunksSaving,
    world::{Chunk, ChunkCoord, ChunkMap, VoxelChanged, Voxels},
};

/// What a fluid pipe's block entity holds of its network's fluid: nothing
/// while it's loaded and its network has it, its share while its chunk is
/// saved (see the module docs).
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq, Eq)]
#[reflect(Component)]
pub struct PipeFluid(pub Vec<FluidAmount>);

/// Whether `state` is a conduit on a network the fluid solver runs.
pub fn is_fluid_conduit(content: &Content, state: BlockState) -> bool {
    content
        .conduit(state)
        .is_some_and(|conduit| content.networks[conduit.network].solver == Solver::Fluid)
}

/// How many blocks of fluid the conduit `state` holds.
pub(super) fn capacity(content: &Content, state: BlockState) -> f64 {
    content
        .conduit(state)
        .map_or(0.0, |conduit| conduit.capacity as f64)
}

impl BlockEntityPart for PipeFluid {
    const KEY: &'static str = "pipe";

    fn new(state: BlockState, content: &Content) -> Option<Self> {
        is_fluid_conduit(content, state).then(Self::default)
    }

    /// A list of `{ fluid, amount }`, empty when it holds nothing.
    fn save(&self, _: &Content) -> Value {
        Value::List(
            self.0
                .iter()
                .map(|held| {
                    Value::Table(
                        [
                            ("fluid".to_string(), Value::Text(held.fluid.clone())),
                            (
                                "amount".to_string(),
                                Value::Int(held.amount.min(i64::MAX as u64) as i64),
                            ),
                        ]
                        .into(),
                    )
                })
                .collect(),
        )
    }

    fn load(value: &Value, version: u32, _: BlockState, content: &Content) -> Option<Self> {
        if version != 1 {
            return None;
        }
        let Value::List(list) = value else {
            return None;
        };
        let mut held = Vec::new();
        for entry in list {
            let entry = entry.as_table()?;
            let Some(Value::Text(fluid)) = entry.get("fluid") else {
                return None;
            };
            let amount = u64::try_from(entry.get("amount")?.as_int()?).ok()?;
            if content.default_state(fluid).is_none() {
                warn!("a pipe held `{fluid}`, which isn't there any more; it's lost");
                continue;
            }
            if amount > 0 {
                held.push(FluidAmount {
                    fluid: fluid.clone(),
                    amount,
                });
            }
        }
        Some(Self(held))
    }
}

/// Pipes loaded holding fluid, waiting for their network to be known to
/// give it back.
#[derive(Resource, Debug, Default)]
pub struct Reclaim(pub(super) HashSet<Entity>);

/// Notes a pipe that came with fluid in it.
pub(super) fn note_loaded_pipe(
    add: On<Add<PipeFluid>>,
    pipes: Query<&PipeFluid>,
    mut reclaim: ResMut<Reclaim>,
) {
    if pipes.get(add.entity).is_ok_and(|pipe| !pipe.0.is_empty()) {
        reclaim.0.insert(add.entity);
    }
}

/// Writes `held` into a pipe's block entity: the part and its saved form
/// (as this may be just before its chunk is saved, after changes are
/// noted), and notes the change so its chunk is saved.
fn write_pipe(
    pos: IVec3,
    held: Vec<FluidAmount>,
    part: &mut Mut<PipeFluid>,
    saved: &mut SavedParts,
    content: &Content,
    changes: &mut MessageWriter<BlockEntityChanged>,
) {
    if part.0 == held {
        return;
    }
    let part = part.bypass_change_detection();
    part.0 = held;
    saved.0.insert(
        PipeFluid::KEY.to_string(),
        SavedPart {
            version: PipeFluid::VERSION,
            value: part.save(content),
        },
    );
    changes.write(BlockEntityChanged { pos });
}

/// Gives the fluid pipes loaded with back to their networks, once the
/// networks are known, emptying the pipes.
pub(super) fn reclaim(
    mut reclaim: ResMut<Reclaim>,
    networks: Res<Networks>,
    mut nets: ResMut<NetworkMap<FluidNet>>,
    mut pipes: Query<(&BlockEntity, &mut PipeFluid, &mut SavedParts)>,
    content: Res<Content>,
    mut changes: MessageWriter<BlockEntityChanged>,
) {
    reclaim_pending(
        &mut reclaim,
        &networks,
        &mut nets,
        &mut pipes,
        &content,
        &mut changes,
    );
}

fn reclaim_pending(
    reclaim: &mut Reclaim,
    networks: &Networks,
    nets: &mut NetworkMap<FluidNet>,
    pipes: &mut Query<(&BlockEntity, &mut PipeFluid, &mut SavedParts)>,
    content: &Content,
    changes: &mut MessageWriter<BlockEntityChanged>,
) {
    if reclaim.0.is_empty() {
        return;
    }
    let mut pending: Vec<Entity> = reclaim.0.iter().copied().collect();
    pending.sort();
    for entity in pending {
        let Ok((block, mut part, mut saved)) = pipes.get_mut(entity) else {
            // Unloaded again first: its saved form still has it.
            reclaim.0.remove(&entity);
            continue;
        };
        let Some(id) = networks.at(block.pos) else {
            continue;
        };
        let pos = block.pos;
        let net = nets.get_mut(id);
        for held in &part.0 {
            net.add(&held.fluid, held.amount);
        }
        write_pipe(pos, Vec::new(), &mut part, &mut saved, content, changes);
        reclaim.0.remove(&entity);
    }
}

/// Writes the share of each pipe in `pipes` (by network) into its block
/// entity. With `take`, its chunk is unloading: the share comes out of the
/// network. Without, the network keeps it and the pipe gets a copy, for a
/// save of the loaded world.
///
/// The network's fluid is shared between its pipes that still hold some
/// of it: those in loaded chunks, other than the chunks in `taken`, whose
/// pipes' shares were taken out already this frame. A pipe whose chunk has
/// gone (unloaded earlier in the frame, before the networks see it) or is
/// in `taken` gets nothing and counts for nothing.
fn record_shares(
    pipes: HashMap<NetworkId, Vec<IVec3>>,
    take: bool,
    taken: &HashSet<IVec3>,
    world: &impl Fn(IVec3) -> Option<BlockState>,
    networks: &Networks,
    nets: &mut NetworkMap<FluidNet>,
    entities: &BlockEntities,
    parts: &mut Query<(&BlockEntity, &mut PipeFluid, &mut SavedParts)>,
    content: &Content,
    changes: &mut MessageWriter<BlockEntityChanged>,
) {
    let mut ids: Vec<NetworkId> = pipes.keys().copied().collect();
    ids.sort();
    for id in ids {
        let Some(network) = networks.get(id) else {
            continue;
        };
        let holds = |pos: IVec3| {
            let (chunk, _) = ChunkCoord::split(pos);
            !taken.contains(&chunk.0) && world(pos).is_some()
        };
        let volume = |pos: IVec3| world(pos).map_or(0.0, |state| capacity(content, state));
        let mut positions: Vec<IVec3> = pipes[&id].iter().copied().filter(|&p| holds(p)).collect();
        positions.sort_by_key(|pos| pos.to_array());
        let weights: Vec<f64> = positions.iter().map(|&pos| volume(pos)).collect();
        let ours: HashSet<IVec3> = positions.iter().copied().collect();
        let others = network
            .conduits()
            .iter()
            .copied()
            .filter(|pos| !ours.contains(pos) && holds(*pos));
        let rest = loaded_volume(others, world, content);
        let mut left = nets.get(id).cloned().unwrap_or_default();
        let shares = left.take_shares(&weights, rest);
        for (&pos, share) in positions.iter().zip(shares) {
            let found = entities
                .get(pos)
                .and_then(|entity| parts.get_mut(entity).ok());
            match found {
                Some((_, mut part, mut saved)) => {
                    write_pipe(pos, share, &mut part, &mut saved, content, changes);
                }
                // No block entity to keep it yet (placed this frame): it
                // stays in the network.
                None if take => {
                    for held in share {
                        left.add(&held.fluid, held.amount);
                    }
                }
                None => {}
            }
        }
        if take {
            *nets.get_mut(id) = left;
        }
    }
}

/// The volume of the conduits at `positions` that hold fluid: those in
/// loaded chunks (`world` gives their state; a chunk that has unloaded
/// took its pipes' shares with it), summed in the same order every time.
fn loaded_volume(
    positions: impl Iterator<Item = IVec3>,
    world: &impl Fn(IVec3) -> Option<BlockState>,
    content: &Content,
) -> f64 {
    let mut found: Vec<(IVec3, f64)> = positions
        .filter_map(|pos| Some((pos, capacity(content, world(pos)?))))
        .collect();
    found.sort_by_key(|(pos, _)| pos.to_array());
    found.iter().map(|(_, volume)| volume).sum()
}

/// The fluid conduits in `chunks`, by network, or in every fluid network
/// if `chunks` is `None`.
fn fluid_pipes(
    networks: &Networks,
    content: &Content,
    chunks: Option<&[IVec3]>,
) -> HashMap<NetworkId, Vec<IVec3>> {
    let fluid = |id: NetworkId| {
        networks
            .get(id)
            .is_some_and(|network| content.networks[network.kind()].solver == Solver::Fluid)
    };
    let mut pipes: HashMap<NetworkId, Vec<IVec3>> = HashMap::new();
    match chunks {
        Some(chunks) => {
            for &chunk in chunks {
                for pos in networks.in_chunk(chunk) {
                    if let Some(id) = networks.at(pos).filter(|&id| fluid(id)) {
                        pipes.entry(id).or_default().push(pos);
                    }
                }
            }
        }
        None => {
            for (id, network) in networks.iter() {
                if fluid(id) {
                    pipes.insert(id, network.conduits().iter().copied().collect());
                }
            }
        }
    }
    pipes
}

/// Just before chunks are saved: the pipes of those unloading take their
/// share out of their networks, and on an autosave or exit every pipe
/// gets a copy of its share. Pipes waiting to give theirs back do first.
pub(super) fn record_before_save(
    saving: Res<ChunksSaving>,
    mut reclaim_list: ResMut<Reclaim>,
    networks: Res<Networks>,
    mut nets: ResMut<NetworkMap<FluidNet>>,
    entities: Res<BlockEntities>,
    mut parts: Query<(&BlockEntity, &mut PipeFluid, &mut SavedParts)>,
    content: Res<Content>,
    map: Res<ChunkMap>,
    chunks: Query<&Chunk>,
    mut changes: MessageWriter<BlockEntityChanged>,
) {
    if saving.unloading.is_empty() && !saving.loaded {
        return;
    }
    let world = |pos: IVec3| -> Option<BlockState> {
        let (coord, local) = ChunkCoord::split(pos);
        Some(chunks.get(map.get(coord)?).ok()?.at(local))
    };
    // Pipes loaded with fluid whose networks are known give it back first,
    // so a share written now counts it.
    reclaim_pending(
        &mut reclaim_list,
        &networks,
        &mut nets,
        &mut parts,
        &content,
        &mut changes,
    );
    let unloading: HashSet<IVec3> = saving.unloading.iter().copied().collect();
    if !saving.unloading.is_empty() {
        let pipes = fluid_pipes(&networks, &content, Some(&saving.unloading));
        record_shares(
            pipes,
            true,
            &HashSet::new(),
            &world,
            &networks,
            &mut nets,
            &entities,
            &mut parts,
            &content,
            &mut changes,
        );
    }
    if saving.loaded {
        // The unloading pipes' shares are theirs already: the loaded ones
        // share what's left.
        let pipes = fluid_pipes(&networks, &content, None);
        record_shares(
            pipes,
            false,
            &unloading,
            &world,
            &networks,
            &mut nets,
            &entities,
            &mut parts,
            &content,
            &mut changes,
        );
    }
}

/// Shares of broken pipes, waiting to be poured where they were.
#[derive(Resource, Debug, Default)]
pub struct Spills(Vec<(IVec3, Vec<FluidAmount>)>);

/// A fluid pipe broken (or replaced by anything but another fluid conduit)
/// takes its share out of its network, for [`pour_spills`]. Runs before
/// the networks see the change, while the pipe is still on its network.
/// The pipes of one network broken in the same frame take their shares
/// together, so each gets its own by volume. A pipe broken while it's
/// still waiting to give back what it was loaded with ([`Reclaim`]) spills
/// that too; one replaced by a fluid conduit of another block (whose block
/// entity starts empty) gives it back to its network first.
pub(super) fn spill_broken_pipes(
    mut edits: MessageReader<VoxelChanged>,
    networks: Res<Networks>,
    mut nets: ResMut<NetworkMap<FluidNet>>,
    content: Res<Content>,
    mut spills: ResMut<Spills>,
    index: Res<BlockEntities>,
    mut reclaim: ResMut<Reclaim>,
    loaded: Query<&PipeFluid>,
    map: Res<ChunkMap>,
    chunks: Query<&Chunk>,
) {
    let mut done = HashSet::new();
    let mut broken: Vec<(NetworkId, Vec<(IVec3, f64)>)> = Vec::new();
    let mut pending: Vec<(IVec3, Vec<FluidAmount>)> = Vec::new();
    for edit in edits.read() {
        if !is_fluid_conduit(&content, edit.before) {
            continue;
        }
        let replaced = is_fluid_conduit(&content, edit.after);
        // The same block keeps its block entity, fluid and all.
        if replaced && content.state(edit.before).block == content.state(edit.after).block {
            continue;
        }
        let waiting = index
            .get(edit.pos)
            .filter(|entity| reclaim.0.remove(entity))
            .and_then(|entity| loaded.get(entity).ok());
        if replaced {
            // Its network counts it like the rest of what it holds.
            match (waiting, networks.at(edit.pos)) {
                (Some(pipe), Some(id)) => {
                    let net = nets.get_mut(id);
                    for held in &pipe.0 {
                        net.add(&held.fluid, held.amount);
                    }
                }
                (Some(pipe), None) => pending.push((edit.pos, pipe.0.clone())),
                (None, _) => {}
            }
            continue;
        }
        if !done.insert(edit.pos) {
            continue;
        }
        if let Some(pipe) = waiting {
            pending.push((edit.pos, pipe.0.clone()));
        }
        let Some(id) = networks.at(edit.pos) else {
            continue;
        };
        let pipe = (edit.pos, capacity(&content, edit.before));
        match broken.iter_mut().find(|(network, _)| *network == id) {
            Some((_, pipes)) => pipes.push(pipe),
            None => broken.push((id, vec![pipe])),
        }
    }
    let world = |pos: IVec3| -> Option<BlockState> {
        let (coord, local) = ChunkCoord::split(pos);
        Some(chunks.get(map.get(coord)?).ok()?.at(local))
    };
    let mut shares: Vec<(IVec3, Vec<FluidAmount>)> = Vec::new();
    for (id, pipes) in broken {
        let Some(network) = networks.get(id) else {
            continue;
        };
        let weights: Vec<f64> = pipes.iter().map(|&(_, capacity)| capacity).collect();
        // Shared with the network's other pipes that hold fluid (not those
        // in a chunk that's unloaded and taken its share).
        let gone: HashSet<IVec3> = pipes.iter().map(|&(pos, _)| pos).collect();
        let others = network
            .conduits()
            .iter()
            .copied()
            .filter(|pos| !gone.contains(pos));
        let rest = loaded_volume(others, &world, &content);
        let taken = nets.get_mut(id).take_shares(&weights, rest);
        shares.extend(pipes.into_iter().map(|(pos, _)| pos).zip(taken));
    }
    // One spill a voxel: what it was loaded with joins its share.
    for (pos, held) in pending {
        let spill = match shares.iter_mut().find(|(at, _)| *at == pos) {
            Some((_, share)) => share,
            None => {
                shares.push((pos, Vec::new()));
                &mut shares.last_mut().unwrap().1
            }
        };
        for amount in held {
            match spill.iter_mut().find(|there| there.fluid == amount.fluid) {
                Some(there) => there.amount += amount.amount,
                None => spill.push(amount),
            }
        }
    }
    for (pos, share) in shares {
        if !share.is_empty() {
            spills.0.push((pos, share));
        }
    }
}

/// Counts what a network held when it lost every conduit, as lost: its
/// pipes were replaced by fluid conduits that didn't join it (a broken
/// pipe's share was spilled, an unloading pipe's saved, before), and the
/// split that ends it has nowhere to put it.
pub(super) fn count_ended_networks(
    mut changes: MessageReader<NetworkChanged>,
    nets: Res<NetworkMap<FluidNet>>,
    mut stats: ResMut<FluidNetworkStats>,
) {
    for change in changes.read() {
        if let NetworkChanged::Split { from, parts, .. } = change
            && parts.is_empty()
            && let Some(net) = nets.get(*from)
        {
            stats.lost += net.total();
        }
    }
}

/// Pours each broken pipe's share into its voxel, as much as the block
/// there now leaves room for, for flow to spread. What doesn't fit (or has nowhere to go) is lost, and
/// counted in [`FluidNetworkStats::lost`].
pub(super) fn pour_spills(
    mut spills: ResMut<Spills>,
    content: Res<Content>,
    mut voxels: Voxels,
    mut stats: ResMut<FluidNetworkStats>,
) {
    for (pos, mut share) in std::mem::take(&mut spills.0) {
        let first = &mut share[0];
        if let Some(state) = content.default_state(&first.fluid) {
            let want = first.amount.min(FULL_FLUID as u64) as u16;
            let poured = voxels.pour(pos, state, want) as u64;
            first.amount -= poured;
            stats.spilled += poured;
        }
        stats.lost += share.iter().map(|held| held.amount).sum::<u64>();
    }
}

/// How much of each fluid is in the pipes, by fluid block id: what the
/// networks hold and what loaded pipes are waiting to give back.
pub fn held_in_pipes(world: &World) -> std::collections::BTreeMap<String, u64> {
    held_in_pipes_within(world, |_| true)
}

/// [`held_in_pipes`], for the networks with a pipe where `within` says
/// and the loaded pipes there.
pub fn held_in_pipes_within(
    world: &World,
    within: impl Fn(IVec3) -> bool,
) -> std::collections::BTreeMap<String, u64> {
    let mut totals = std::collections::BTreeMap::new();
    let networks = world.get_resource::<Networks>();
    if let Some(nets) = world.get_resource::<NetworkMap<FluidNet>>() {
        for (id, net) in nets.iter() {
            let network = networks.and_then(|networks| networks.get(id));
            if !network.is_none_or(|network| network.conduits().iter().any(|&pos| within(pos))) {
                continue;
            }
            for held in &net.held {
                *totals.entry(held.fluid.clone()).or_default() += held.amount;
            }
        }
    }
    let pending: Vec<Entity> = world
        .get_resource::<Reclaim>()
        .map(|reclaim| reclaim.0.iter().copied().collect())
        .unwrap_or_default();
    for entity in pending {
        if world
            .get::<BlockEntity>(entity)
            .is_some_and(|block_entity| !within(block_entity.pos))
        {
            continue;
        }
        if let Some(pipe) = world.get::<PipeFluid>(entity) {
            for held in &pipe.0 {
                *totals.entry(held.fluid.clone()).or_default() += held.amount;
            }
        }
    }
    totals
}
