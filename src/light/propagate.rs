//! The light flood fill: Minecraft-style light propagation, kept up to date
//! one change at a time.
//!
//! Each channel spreads on its own. Work is queued per chunk, as nodes that
//! name a voxel by its index in the chunk, so the fill indexes the chunk's
//! arrays directly and only visits chunks that have something to do. A step
//! that crosses a chunk border queues a node in the chunk across it, which
//! is handled when that chunk's turn comes; nothing crosses into a chunk that
//! isn't loaded.
//!
//! A change runs in two passes per channel, as in the well-known algorithm:
//!
//! 1. **Removal.** Starting at a changed voxel, clear the light that may
//!    have come through it: a neighbour dimmer than the light being removed
//!    was (or may have been) lit by it, so it's cleared too and the removal
//!    carries on from there. A neighbour at least as bright was lit some
//!    other way; it's queued to spread its light back into what was cleared.
//! 2. **Addition.** Spread light outward from every queued voxel, dropping a
//!    level per block (and more into a block with a light filter, such as
//!    water), raising each voxel it reaches that's darker. Each
//!    chunk's nodes are handled brightest first, so a voxel is usually set
//!    once, to its final level.
//!
//! The order chunks are handled in doesn't change the result, only how much
//! work it takes: every voxel ends at the brightest level any source can
//! reach it with.
//!
//! Sky light enters the loaded chunks at the top layer of each chunk with
//! no loaded chunk above it, where [`OutsideSky`] says it does, and from
//! there it's like any other source. So it moves when chunks load and
//! unload above others: a chunk loading on top of another takes away the
//! sky that shone into the one below (where it doesn't pass full sky light
//! down itself), and one unloading gives it back.

use std::collections::VecDeque;

use bevy::{
    platform::collections::{HashMap, HashSet},
    prelude::*,
};

use super::{CHANNELS, ChunkLight, Light, MAX_LIGHT, SKY, sky::OutsideSky};
use crate::{
    content::{BlockState, Content, Direction},
    world::{CHUNK_SIZE, CHUNK_VOLUME, Chunk, ChunkCoord, axis_of, touching_offsets},
};

/// Access to the loaded chunks, one at a time.
pub trait ChunkStore {
    /// Calls `f` with the chunk at `coord` and its light. False, without
    /// calling it, if that chunk isn't loaded.
    fn with_chunk(&mut self, coord: IVec3, f: impl FnOnce(&Chunk, &mut ChunkLight)) -> bool;

    /// Whether the chunk at `coord` is loaded.
    fn is_loaded(&self, coord: IVec3) -> bool;
}

/// What each block state does to light, looked up once from [`Content`].
#[derive(Resource, Debug, Clone)]
pub struct LightTable {
    /// Per state, a bit per [`DIRECTIONS`] entry: the sides it covers
    /// completely, which light can't pass through in either direction.
    closed: Vec<u8>,
    /// The sky channel is always 0: blocks don't give off sky light.
    emits: Vec<Light>,
    /// Per state, the levels light loses coming into it, on top of the one
    /// per block.
    filter: Vec<u8>,
}

impl LightTable {
    pub fn new(content: &Content) -> Self {
        Self {
            closed: content
                .states
                .iter()
                .map(|state| {
                    DIRECTIONS
                        .iter()
                        .enumerate()
                        .filter(|&(_, &d)| {
                            Direction::from_normal(d).is_some_and(|side| state.blocks_light(side))
                        })
                        .fold(0, |bits, (i, _)| bits | 1 << i)
                })
                .collect(),
            emits: content
                .states
                .iter()
                .map(|s| Light::new(0, s.light))
                .collect(),
            filter: content.states.iter().map(|s| s.light_filter).collect(),
        }
    }

    /// Whether light can't leave `state` through its side toward
    /// `DIRECTIONS[direction]` (or come in through it).
    fn closed(&self, state: BlockState, direction: usize) -> bool {
        self.closed
            .get(state.0 as usize)
            .is_some_and(|&bits| bits & 1 << direction != 0)
    }

    /// Whether light treats the two states the same: they close the same
    /// sides, give off the same light and dim it as much.
    pub fn same_light(&self, a: BlockState, b: BlockState) -> bool {
        self.closed.get(a.0 as usize) == self.closed.get(b.0 as usize)
            && self.emits.get(a.0 as usize) == self.emits.get(b.0 as usize)
            && self.filter.get(a.0 as usize) == self.filter.get(b.0 as usize)
    }

    /// The level light offered at `level` to a voxel of `state` from
    /// outside it gets in with: less the levels `state` filters out.
    fn entering(&self, state: BlockState, level: u8) -> u8 {
        let filter = self.filter.get(state.0 as usize).copied().unwrap_or(0);
        level.saturating_sub(filter)
    }

    /// Whether light can't come into `state` through all of its sides.
    fn closed_all(&self, state: BlockState) -> bool {
        self.closed.get(state.0 as usize) == Some(&ALL_DIRECTIONS)
    }

    /// The level `channel`'s light leaves a voxel of `state` with toward
    /// `DIRECTIONS[direction]`, where it would be `next` through an open
    /// side. Through a covered side only the block's own light gets out (a
    /// light source shines out of every side), so 0 for a block that gives
    /// off none of that colour.
    fn leaving(&self, state: BlockState, direction: usize, channel: usize, next: u8) -> u8 {
        if self.closed(state, direction) {
            next.min(self.emits(state, channel).saturating_sub(1))
        } else {
            next
        }
    }

    /// The level `channel`'s light reaches a voxel of `to` with, stepping
    /// in from its neighbour of `from` toward `DIRECTIONS[direction]`,
    /// where it would be `next` with nothing in the way: 0 through a side
    /// either covers, and less what `to` filters out.
    fn passing(
        &self,
        from: BlockState,
        to: BlockState,
        direction: usize,
        channel: usize,
        next: u8,
    ) -> u8 {
        if self.closed(to, opposite(direction)) {
            0
        } else {
            self.entering(to, self.leaving(from, direction, channel, next))
        }
    }

    fn emits(&self, state: BlockState, channel: usize) -> u8 {
        self.emits
            .get(state.0 as usize)
            .map_or(0, |light| light.get(channel))
    }
}

/// [`LightTable::closed`] bits for every side.
const ALL_DIRECTIONS: u8 = (1 << DIRECTIONS.len()) - 1;

/// The index in [`DIRECTIONS`] of the opposite direction.
const fn opposite(direction: usize) -> usize {
    direction ^ 1
}

/// Lights chunks by themselves, as if nothing else were loaded: their own
/// light sources and the sky shining into each one with none of the others
/// above it, spread through all of them. Returns each one's light, in
/// order. Loading uses this to light chunks on a task, then
/// [`LightQueue::lit_chunk_loaded`] joins them to the world.
pub fn light_alone(
    chunks: &[(IVec3, &Chunk)],
    table: &LightTable,
    sky: &dyn OutsideSky,
) -> Vec<ChunkLight> {
    struct Alone<'a> {
        chunks: &'a [(IVec3, &'a Chunk)],
        lights: Vec<ChunkLight>,
    }
    impl ChunkStore for Alone<'_> {
        fn with_chunk(&mut self, coord: IVec3, f: impl FnOnce(&Chunk, &mut ChunkLight)) -> bool {
            let Some(i) = self.chunks.iter().position(|&(c, _)| c == coord) else {
                return false;
            };
            f(self.chunks[i].1, &mut self.lights[i]);
            true
        }

        fn is_loaded(&self, coord: IVec3) -> bool {
            self.chunks.iter().any(|&(c, _)| c == coord)
        }
    }
    let mut queue = LightQueue::default();
    for &(coord, chunk) in chunks {
        queue.sources(coord, chunk, table);
        if !chunks.iter().any(|&(c, _)| c == coord + IVec3::Y) {
            queue.outside_sky(coord, sky);
        }
    }
    let mut store = Alone {
        chunks,
        lights: vec![ChunkLight::default(); chunks.len()],
    };
    queue.run(&mut store, table, sky);
    // Most are open sky or solid rock, lit evenly throughout.
    for light in &mut store.lights {
        light.compact();
    }
    store.lights
}

/// One step of the fill, in one chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Node {
    /// The voxel's index in its chunk.
    index: u16,
    /// For an addition, the level offered to the voxel; 0 means "spread
    /// whatever it has". For a removal, the level of the light being
    /// removed next to it.
    level: u8,
    flags: u8,
}

/// A removal arriving from the voxel above, for the sky's straight-down rule.
const FROM_ABOVE: u8 = 1;
/// An addition at a light source, which may be opaque.
const SOURCE: u8 = 2;
/// A removal at a voxel that changed: always cleared, and its neighbours
/// spread back into it.
const CHANGED: u8 = 4;
/// An addition of sky light from outside the loaded chunks, into a top
/// layer. Dropped if a chunk has loaded above by the time it runs: queued
/// before that chunk loaded, it's out of date (and the chunk loading took
/// the outside sky away).
const OUTSIDE: u8 = 8;
/// An addition's top bits: 1 + the [`DIRECTIONS`] index it's travelling
/// in, for light stepping in from a neighbour (or the sky outside), which
/// the voxel's side facing back may stop. 0 for light that starts here.
const HEADING_SHIFT: u8 = 4;

/// The flags for an addition heading in `DIRECTIONS[direction]`.
const fn heading(direction: usize) -> u8 {
    (direction as u8 + 1) << HEADING_SHIFT
}

/// The direction an addition's flags say it's heading in, if any.
fn heading_of(flags: u8) -> Option<usize> {
    (flags >> HEADING_SHIFT).checked_sub(1).map(usize::from)
}

/// The six neighbour directions, each next to its opposite. [`DOWN`]
/// indexes the one below.
const DIRECTIONS: [IVec3; 6] = [
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Y,
    IVec3::NEG_Y,
    IVec3::Z,
    IVec3::NEG_Z,
];
const DOWN: usize = 3;

/// Light work waiting to be done, per channel and chunk.
#[derive(Resource, Debug, Default)]
pub struct LightQueue {
    removals: [HashMap<IVec3, Vec<Node>>; CHANNELS],
    additions: [HashMap<IVec3, Vec<Node>>; CHANNELS],
}

impl LightQueue {
    pub fn is_empty(&self) -> bool {
        self.removals.iter().all(HashMap::is_empty) && self.additions.iter().all(HashMap::is_empty)
    }

    /// Queues the light for a chunk that just loaded, dark: its own light
    /// sources, sky light from outside if the chunk above isn't loaded, and
    /// the light of each loaded neighbour spreading in. `neighbour` gives a
    /// loaded chunk's blocks and light by coordinate, and `None` for one
    /// that isn't loaded.
    pub fn chunk_loaded<'a>(
        &mut self,
        coord: IVec3,
        chunk: &Chunk,
        table: &LightTable,
        sky: &dyn OutsideSky,
        neighbour: impl Fn(IVec3) -> Option<(&'a Chunk, &'a ChunkLight)>,
    ) {
        self.sources(coord, chunk, table);
        if neighbour(coord + IVec3::Y).is_none() {
            self.outside_sky(coord, sky);
        }
        self.borders(coord, |c| neighbour(c).is_some(), false);
        self.cover_below(coord, None, sky, table, &neighbour);
    }

    /// Queues the light for a chunk that loaded already lit by
    /// [`light_alone`] (`light`): each loaded neighbour's light spreading
    /// in, and its own spreading out into them. Its own sources and the sky
    /// from above are in its light already, so this is much less work than
    /// [`chunk_loaded`] (and it's all that's left for the main thread when
    /// chunks are lit on a task). If the chunk above is loaded after all,
    /// the sky `light_alone` let in at the top is taken away again.
    ///
    /// [`chunk_loaded`]: LightQueue::chunk_loaded
    pub fn lit_chunk_loaded<'a>(
        &mut self,
        coord: IVec3,
        chunk: &Chunk,
        light: &ChunkLight,
        table: &LightTable,
        sky: &dyn OutsideSky,
        neighbour: impl Fn(IVec3) -> Option<(&'a Chunk, &'a ChunkLight)>,
    ) {
        self.borders(coord, |c| neighbour(c).is_some(), true);
        if let Some(above) = neighbour(coord + IVec3::Y) {
            self.cover(coord, Some(above), sky, table);
        }
        self.cover_below(coord, Some((chunk, light)), sky, table, &neighbour);
    }

    /// Queues the light for a chunk that's been unloaded, whose light was
    /// `light`: the light it gave the loaded chunks around it is taken away
    /// (as if its voxels against them had gone dark), and the chunk below
    /// it gets sky light from outside again. So the light always depends
    /// only on what's loaded and the outside sky.
    pub fn chunk_unloaded(
        &mut self,
        coord: IVec3,
        light: &ChunkLight,
        sky: &dyn OutsideSky,
        loaded: impl Fn(IVec3) -> bool,
    ) {
        let last = CHUNK_SIZE as i32 - 1;
        for (d, direction) in DIRECTIONS.into_iter().enumerate() {
            let neighbour = coord + direction;
            if !loaded(neighbour) {
                continue;
            }
            let axis = axis_of(direction);
            let ours = if direction[axis] > 0 { last } else { 0 };
            let theirs = last - ours;
            for (u, v) in square() {
                let mut pos = IVec3::ZERO;
                pos[(axis + 1) % 3] = u;
                pos[(axis + 2) % 3] = v;
                pos[axis] = ours;
                let level = light.at(pos);
                pos[axis] = theirs;
                let index = Chunk::index_of(pos) as u16;
                for (channel, removals) in self.removals.iter_mut().enumerate() {
                    let level = level.get(channel);
                    // Full sky into the top of the chunk below is what the
                    // outside sky gives it now anyway.
                    let unchanged = d == DOWN
                        && channel == SKY
                        && level == MAX_LIGHT
                        && sky.shines_into(neighbour, pos.x, pos.z);
                    if level == 0 || unchanged {
                        continue;
                    }
                    removals.entry(neighbour).or_default().push(Node {
                        index,
                        level,
                        flags: if d == DOWN { FROM_ABOVE } else { 0 },
                    });
                }
            }
        }
        let below = coord - IVec3::Y;
        if loaded(below) {
            self.outside_sky(below, sky);
        }
    }

    /// Queues a chunk's light sources.
    fn sources(&mut self, coord: IVec3, chunk: &Chunk, table: &LightTable) {
        for index in 0..CHUNK_VOLUME {
            let state = chunk.get_index(index);
            for channel in 1..CHANNELS {
                let level = table.emits(state, channel);
                if level > 0 {
                    self.add(channel, coord, index, level, SOURCE);
                }
            }
        }
    }

    /// Queues the sky shining into a chunk's top layer from outside the
    /// loaded chunks, where `sky` says it does.
    fn outside_sky(&mut self, coord: IVec3, sky: &dyn OutsideSky) {
        let last = CHUNK_SIZE as i32 - 1;
        for (x, z) in square() {
            if sky.shines_into(coord, x, z) {
                let index = Chunk::index_of(IVec3::new(x, last, z));
                self.add(SKY, coord, index, MAX_LIGHT, OUTSIDE | heading(DOWN));
            }
        }
    }

    /// The chunk at `coord` has a chunk above it now, so the sky from
    /// outside no longer shines into its top layer: takes it away from
    /// every top voxel it shone into, except where the chunk above passes
    /// full sky light straight down anyway (by its bottom layer in
    /// `above`'s blocks and light, if known), since nothing would change
    /// there.
    fn cover(
        &mut self,
        coord: IVec3,
        above: Option<(&Chunk, &ChunkLight)>,
        sky: &dyn OutsideSky,
        table: &LightTable,
    ) {
        let last = CHUNK_SIZE as i32 - 1;
        for (x, z) in square() {
            if !sky.shines_into(coord, x, z) {
                continue;
            }
            let passes = above.is_some_and(|(blocks, light)| {
                let bottom = IVec3::new(x, 0, z);
                light.at(bottom).get(SKY) == MAX_LIGHT && !table.closed(blocks.at(bottom), DOWN)
            });
            if !passes {
                let index = Chunk::index_of(IVec3::new(x, last, z));
                self.removals[SKY].entry(coord).or_default().push(Node {
                    index: index as u16,
                    level: 0,
                    flags: CHANGED,
                });
            }
        }
    }

    /// Covers the loaded chunk below a chunk that just loaded with these
    /// blocks and light (`None` if it's still dark).
    fn cover_below<'a>(
        &mut self,
        coord: IVec3,
        chunk: Option<(&Chunk, &ChunkLight)>,
        sky: &dyn OutsideSky,
        table: &LightTable,
        neighbour: &impl Fn(IVec3) -> Option<(&'a Chunk, &'a ChunkLight)>,
    ) {
        let below = coord - IVec3::Y;
        if neighbour(below).is_some() {
            self.cover(below, chunk, sky, table);
        }
    }

    /// Queues each loaded neighbour's layer against the chunk to spread its
    /// light in, and with `outward`, the chunk's own layer against it to
    /// spread out.
    fn borders(&mut self, coord: IVec3, loaded: impl Fn(IVec3) -> bool, outward: bool) {
        let last = CHUNK_SIZE as i32 - 1;
        for direction in DIRECTIONS {
            let neighbour = coord + direction;
            if !loaded(neighbour) {
                continue;
            }
            let axis = axis_of(direction);
            // The neighbour's layer facing this chunk, and this chunk's
            // facing it.
            let theirs = if direction[axis] > 0 { 0 } else { last };
            let ours = last - theirs;
            for (u, v) in square() {
                let mut pos = IVec3::ZERO;
                pos[(axis + 1) % 3] = u;
                pos[(axis + 2) % 3] = v;
                pos[axis] = theirs;
                let index = Chunk::index_of(pos);
                pos[axis] = ours;
                let own = Chunk::index_of(pos);
                for channel in 0..CHANNELS {
                    self.add(channel, neighbour, index, 0, 0);
                    if outward {
                        self.add(channel, coord, own, 0, 0);
                    }
                }
            }
        }
    }

    /// Queues the light around a voxel that changed, by world position.
    /// The removal offers it the sky from outside again if it's in a top
    /// layer (see [`Fill::offer_outside_sky`]).
    pub fn voxel_changed(&mut self, pos: IVec3) {
        let (coord, local) = split(pos);
        let index = Chunk::index_of(local);
        for channel in self.removals.iter_mut() {
            channel.entry(coord).or_default().push(Node {
                index: index as u16,
                level: 0,
                flags: CHANGED,
            });
        }
    }

    fn add(&mut self, channel: usize, coord: IVec3, index: usize, level: u8, flags: u8) {
        self.additions[channel]
            .entry(coord)
            .or_default()
            .push(Node {
                index: index as u16,
                level,
                flags,
            });
    }

    /// Does all the queued work. Returns the chunks whose meshes show light
    /// that changed: each chunk whose light changed, and the chunks across
    /// the border from each voxel on its edge that changed.
    pub fn run(
        &mut self,
        store: &mut impl ChunkStore,
        table: &LightTable,
        sky: &dyn OutsideSky,
    ) -> HashSet<IVec3> {
        let mut remesh = HashSet::new();
        for channel in 0..CHANNELS {
            let (removals, additions) = (&mut self.removals[channel], &mut self.additions[channel]);
            while let Some(coord) = removals.keys().next().copied() {
                let nodes = removals.remove(&coord).unwrap_or_default();
                let mut fill = Fill::new(coord, channel, table, sky, removals, additions);
                store.with_chunk(coord, |chunk, light| fill.remove(nodes, chunk, light));
                fill.report(&mut remesh);
            }
            while let Some(coord) = additions.keys().next().copied() {
                let mut nodes = additions.remove(&coord).unwrap_or_default();
                if store.is_loaded(coord + IVec3::Y) {
                    nodes.retain(|node| node.flags & OUTSIDE == 0);
                }
                let mut fill = Fill::new(coord, channel, table, sky, removals, additions);
                store.with_chunk(coord, |chunk, light| fill.add(nodes, chunk, light));
                fill.report(&mut remesh);
            }
        }
        remesh
    }
}

/// The fill for one channel in one chunk, and where it queues work for
/// other chunks.
struct Fill<'a> {
    coord: IVec3,
    channel: usize,
    table: &'a LightTable,
    sky: &'a dyn OutsideSky,
    removals: &'a mut HashMap<IVec3, Vec<Node>>,
    additions: &'a mut HashMap<IVec3, Vec<Node>>,
    /// The chunks whose meshes show a voxel whose light changed, as a bit
    /// per offset from this chunk (see [`offset_bit`]).
    touched: u32,
}

impl<'a> Fill<'a> {
    fn new(
        coord: IVec3,
        channel: usize,
        table: &'a LightTable,
        sky: &'a dyn OutsideSky,
        removals: &'a mut HashMap<IVec3, Vec<Node>>,
        additions: &'a mut HashMap<IVec3, Vec<Node>>,
    ) -> Self {
        Self {
            coord,
            channel,
            table,
            sky,
            removals,
            additions,
            touched: 0,
        }
    }

    fn remove(&mut self, nodes: Vec<Node>, chunk: &Chunk, light: &mut ChunkLight) {
        let mut queue = VecDeque::from(nodes);
        let mut spread = Vec::new();
        while let Some(node) = queue.pop_front() {
            let index = node.index as usize;
            let level = light.get(index).get(self.channel);
            let changed = node.flags & CHANGED != 0;
            let sky_column = self.channel == SKY
                && node.flags & FROM_ABOVE != 0
                && node.level == MAX_LIGHT
                && level == MAX_LIGHT;
            let lit_by_removed = level > 0 && (level < node.level || sky_column);
            if !changed && !lit_by_removed {
                // Lit some other way: spread back into what was cleared.
                if level > 0 {
                    spread.push(node.index);
                }
                continue;
            }
            let emitted = self.table.emits(chunk.get_index(index), self.channel);
            if emitted != level {
                self.set(light, index, emitted);
            }
            if emitted > 0 {
                spread.push(node.index);
            }
            let pos = Chunk::position_of(index);
            self.offer_outside_sky(pos);
            for (i, direction) in DIRECTIONS.into_iter().enumerate() {
                let (coord, neighbour) = self.step(pos + direction);
                if changed {
                    // Whatever is around a changed voxel may light it now.
                    self.queue_addition(coord, neighbour, 0, 0);
                }
                if emitted < level {
                    let node = Node {
                        index: neighbour,
                        level,
                        flags: if i == DOWN { FROM_ABOVE } else { 0 },
                    };
                    if coord == self.coord {
                        queue.push_back(node);
                    } else {
                        self.removals.entry(coord).or_default().push(node);
                    }
                }
            }
        }
        for index in spread {
            self.queue_addition(self.coord, index, 0, 0);
        }
    }

    fn add(&mut self, nodes: Vec<Node>, chunk: &Chunk, light: &mut ChunkLight) {
        // One bucket per level, handled brightest first.
        let mut buckets: [Vec<Node>; MAX_LIGHT as usize + 1] = Default::default();
        for mut node in nodes {
            if node.level == 0 {
                node.level = light.get(node.index as usize).get(self.channel);
            } else if heading_of(node.flags).is_some() {
                // Light stepping in from another chunk, or the sky outside,
                // loses what this voxel filters out.
                node.level = self
                    .table
                    .entering(chunk.get_index(node.index as usize), node.level);
            }
            buckets[node.level as usize].push(node);
        }
        for level in (1..=MAX_LIGHT).rev() {
            while let Some(node) = buckets[level as usize].pop() {
                let index = node.index as usize;
                let current = light.get(index).get(self.channel);
                if current > level {
                    // Something brighter got here first and spreads itself.
                    continue;
                }
                let state = chunk.get_index(index);
                if current < level {
                    // Light from outside the voxel stops at a side it
                    // covers; a source shines even from inside a solid one.
                    let stopped = match heading_of(node.flags) {
                        Some(direction) => self.table.closed(state, opposite(direction)),
                        None => node.flags & SOURCE == 0 && self.table.closed_all(state),
                    };
                    if stopped {
                        continue;
                    }
                    self.set(light, index, level);
                }
                let pos = Chunk::position_of(index);
                for (i, direction) in DIRECTIONS.into_iter().enumerate() {
                    let next = if self.channel == SKY && i == DOWN && level == MAX_LIGHT {
                        MAX_LIGHT
                    } else {
                        level - 1
                    };
                    if next == 0 {
                        continue;
                    }
                    let (coord, neighbour) = self.step(pos + direction);
                    if coord != self.coord {
                        // The other chunk checks the side it comes in by.
                        let next = self.table.leaving(state, i, self.channel, next);
                        if next > 0 {
                            self.queue_addition(coord, neighbour, next, heading(i));
                        }
                        continue;
                    }
                    let n = neighbour as usize;
                    let next = self
                        .table
                        .passing(state, chunk.get_index(n), i, self.channel, next);
                    if next > 0 && light.get(n).get(self.channel) < next {
                        self.set(light, n, next);
                        buckets[next as usize].push(Node {
                            index: neighbour,
                            level: next,
                            flags: 0,
                        });
                    }
                }
            }
        }
    }

    /// Offers the sky from outside the loaded chunks again to a voxel whose
    /// sky light was cleared, if it's in the top layer and the sky shines
    /// into it (dropped in [`LightQueue::run`] if a chunk is loaded above),
    /// since no neighbour spreads that light back. That's a voxel that
    /// changed, or one that filters the sky and so is lit dimmer than full
    /// sky, which a removal coming from beside it can clear.
    fn offer_outside_sky(&mut self, pos: IVec3) {
        let top = pos.y == CHUNK_SIZE as i32 - 1;
        if self.channel == SKY && top && self.sky.shines_into(self.coord, pos.x, pos.z) {
            let index = Chunk::index_of(pos) as u16;
            self.queue_addition(self.coord, index, MAX_LIGHT, OUTSIDE | heading(DOWN));
        }
    }

    /// The chunk and index of a chunk-local position one step from inside
    /// this chunk.
    fn step(&self, pos: IVec3) -> (IVec3, u16) {
        let (offset, local) = split(pos);
        (self.coord + offset, Chunk::index_of(local) as u16)
    }

    fn queue_addition(&mut self, coord: IVec3, index: u16, level: u8, flags: u8) {
        self.additions.entry(coord).or_default().push(Node {
            index,
            level,
            flags,
        });
    }

    fn set(&mut self, light: &mut ChunkLight, index: usize, level: u8) {
        let mut cell = light.get(index);
        cell.set(self.channel, level);
        light.set(index, cell);
        for offset in touching_offsets(Chunk::position_of(index)) {
            self.touched |= offset_bit(offset);
        }
    }

    /// Adds the chunks [`Fill::set`] touched to `remesh`.
    fn report(&self, remesh: &mut HashSet<IVec3>) {
        for bit in 0..27 {
            if self.touched & (1 << bit) != 0 {
                let offset = IVec3::new(bit % 3, bit / 3 % 3, bit / 9) - IVec3::ONE;
                remesh.insert(self.coord + offset);
            }
        }
    }
}

/// A bit for each offset to a chunk around (or at) this one.
fn offset_bit(offset: IVec3) -> u32 {
    let i = offset + IVec3::ONE;
    1 << (i.x + i.y * 3 + i.z * 9)
}

/// A position in chunks and voxels: the chunk and the position in it.
fn split(pos: IVec3) -> (IVec3, IVec3) {
    let (coord, local) = ChunkCoord::split(pos);
    (coord.0, local)
}

/// Every position on a chunk face, as two coordinates.
fn square() -> impl Iterator<Item = (i32, i32)> {
    let size = CHUNK_SIZE as i32;
    (0..size).flat_map(move |u| (0..size).map(move |v| (u, v)))
}

#[cfg(test)]
mod tests;
