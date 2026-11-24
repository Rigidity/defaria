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
//!    level per block, raising each voxel it reaches that's darker. Each
//!    chunk's nodes are handled brightest first, so a voxel is usually set
//!    once, to its final level.
//!
//! The order chunks are handled in doesn't change the result, only how much
//! work it takes: every voxel ends at the brightest level any source can
//! reach it with.

use std::collections::VecDeque;

use bevy::{
    platform::collections::{HashMap, HashSet},
    prelude::*,
};

use super::{CHANNELS, ChunkLight, Light, MAX_LIGHT, SKY};
use crate::{
    content::{BlockState, Content},
    voxel::{CHUNK_SIZE, CHUNK_VOLUME, Chunk},
};

/// Access to the loaded chunks, one at a time.
pub trait ChunkStore {
    /// Calls `f` with the chunk at `coord` and its light. False, without
    /// calling it, if that chunk isn't loaded.
    fn with_chunk(&mut self, coord: IVec3, f: impl FnOnce(&Chunk, &mut ChunkLight)) -> bool;
}

/// What each block state does to light, looked up once from [`Content`].
#[derive(Resource, Debug, Clone)]
pub struct LightTable {
    opaque: Vec<bool>,
    /// The sky channel is always 0: blocks don't give off sky light.
    emits: Vec<Light>,
}

impl LightTable {
    pub fn new(content: &Content) -> Self {
        Self {
            opaque: content.states.iter().map(|s| s.is_opaque()).collect(),
            emits: content
                .states
                .iter()
                .map(|s| Light::new(0, s.light))
                .collect(),
        }
    }

    fn opaque(&self, state: BlockState) -> bool {
        self.opaque.get(state.0 as usize).copied().unwrap_or(false)
    }

    fn emits(&self, state: BlockState, channel: usize) -> u8 {
        self.emits
            .get(state.0 as usize)
            .map_or(0, |light| light.get(channel))
    }
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

/// The six neighbour directions. [`DOWN`] indexes the one below.
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
    /// sources, sky light from above if it's `under_sky`, and the light of
    /// each loaded neighbour (`loaded` says which are) spreading in.
    pub fn chunk_loaded(
        &mut self,
        coord: IVec3,
        chunk: &Chunk,
        table: &LightTable,
        under_sky: bool,
        loaded: impl Fn(IVec3) -> bool,
    ) {
        for index in 0..CHUNK_VOLUME {
            let state = chunk.get_index(index);
            for channel in 1..CHANNELS {
                let level = table.emits(state, channel);
                if level > 0 {
                    self.add(channel, coord, index, level, SOURCE);
                }
            }
        }
        let last = CHUNK_SIZE as i32 - 1;
        if under_sky {
            for (x, z) in square() {
                let index = Chunk::index_of(IVec3::new(x, last, z));
                self.add(SKY, coord, index, MAX_LIGHT, 0);
            }
        }
        for direction in DIRECTIONS {
            let neighbour = coord + direction;
            if !loaded(neighbour) {
                continue;
            }
            // The neighbour's layer facing this chunk.
            let axis = if direction.x != 0 {
                0
            } else if direction.y != 0 {
                1
            } else {
                2
            };
            let layer = if direction[axis] > 0 { 0 } else { last };
            for (u, v) in square() {
                let mut pos = IVec3::ZERO;
                pos[axis] = layer;
                pos[(axis + 1) % 3] = u;
                pos[(axis + 2) % 3] = v;
                let index = Chunk::index_of(pos);
                for channel in 0..CHANNELS {
                    self.add(channel, neighbour, index, 0, 0);
                }
            }
        }
    }

    /// Queues the light around a voxel that changed, by world position.
    pub fn voxel_changed(&mut self, pos: IVec3) {
        let (coord, local) = split(pos);
        let index = Chunk::index_of(local) as u16;
        for channel in self.removals.iter_mut() {
            channel.entry(coord).or_default().push(Node {
                index,
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
    pub fn run(&mut self, store: &mut impl ChunkStore, table: &LightTable) -> HashSet<IVec3> {
        let mut remesh = HashSet::new();
        for channel in 0..CHANNELS {
            let (removals, additions) = (&mut self.removals[channel], &mut self.additions[channel]);
            while let Some(coord) = removals.keys().next().copied() {
                let nodes = removals.remove(&coord).unwrap_or_default();
                let mut fill = Fill {
                    coord,
                    channel,
                    table,
                    removals,
                    additions,
                    remesh: &mut remesh,
                };
                store.with_chunk(coord, |chunk, light| fill.remove(nodes, chunk, light));
            }
            while let Some(coord) = additions.keys().next().copied() {
                let nodes = additions.remove(&coord).unwrap_or_default();
                let mut fill = Fill {
                    coord,
                    channel,
                    table,
                    removals,
                    additions,
                    remesh: &mut remesh,
                };
                store.with_chunk(coord, |chunk, light| fill.add(nodes, chunk, light));
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
    removals: &'a mut HashMap<IVec3, Vec<Node>>,
    additions: &'a mut HashMap<IVec3, Vec<Node>>,
    remesh: &'a mut HashSet<IVec3>,
}

impl Fill<'_> {
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
                if current < level {
                    if node.flags & SOURCE == 0 && self.table.opaque(chunk.get_index(index)) {
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
                        self.queue_addition(coord, neighbour, next, 0);
                        continue;
                    }
                    let n = neighbour as usize;
                    if light.get(n).get(self.channel) < next
                        && !self.table.opaque(chunk.get_index(n))
                    {
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
        self.remesh.insert(self.coord);
        let pos = Chunk::position_of(index);
        let last = CHUNK_SIZE as i32 - 1;
        let side = |v: i32| match v {
            0 => -1,
            v if v == last => 1,
            _ => 0,
        };
        let edge = pos.map(side);
        if edge == IVec3::ZERO {
            return;
        }
        // Every chunk across the edges and corners this voxel touches.
        for x in [0, edge.x] {
            for y in [0, edge.y] {
                for z in [0, edge.z] {
                    self.remesh.insert(self.coord + IVec3::new(x, y, z));
                }
            }
        }
    }
}

/// A position in chunks and voxels: the chunk offset and the position in it.
fn split(pos: IVec3) -> (IVec3, IVec3) {
    let size = IVec3::splat(CHUNK_SIZE as i32);
    (pos.div_euclid(size), pos.rem_euclid(size))
}

/// Every position on a chunk face, as two coordinates.
fn square() -> impl Iterator<Item = (i32, i32)> {
    let size = CHUNK_SIZE as i32;
    (0..size).flat_map(move |u| (0..size).map(move |v| (u, v)))
}

#[cfg(test)]
mod tests;
