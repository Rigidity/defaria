//! Voxels made of two halves: a double slab, or a stone bottom under a
//! planks top. A state that fills exactly one half of its voxel is a
//! *half* ([`State::half`]); two halves of opposite sides combine into a
//! *combined state* ([`State::combined`]), baked at load for every such
//! pair, so meshing, culling, light, physics and fluid read it as they
//! read any state: a full opaque block. Placing, the crosshair, mining
//! and saves know its two parts. See "Halves" in `docs/modding.md` and
//! `docs/decisions.md`.

use bevy::{platform::collections::HashMap, prelude::*};

use super::{
    ALL_SIDES, BlockBox, BlockState, Content, Direction, Render, State, bake::side_texture,
};
use crate::content::Block;

/// The id of the engine's block that combined states belong to. Not
/// defined by any mod, and not looked up by id: a combined state is named
/// by its parts, joined by [`PART_SEPARATOR`].
pub const COMBINED: &str = "combined";

/// Between the names of a combined state's two parts where it's written
/// down: `defaria:stone_slab[half=bottom,…]+defaria:planks_slab[half=top,…]`.
pub const PART_SEPARATOR: char = '+';

/// The sides a combined state's first part can fill: the low side of
/// each axis. Its second part fills the opposite side.
const LOW_SIDES: [Direction; 3] = [Direction::Down, Direction::North, Direction::West];

/// The half of a voxel on `side`.
pub fn half_box(side: Direction) -> BlockBox {
    let (mut min, mut max) = (Vec3::ZERO, Vec3::ONE);
    let axis = axis(side);
    if side.normal()[axis] < 0 {
        max[axis] = 0.5;
    } else {
        min[axis] = 0.5;
    }
    BlockBox { min, max }
}

/// The index of the axis `side` is on: 0 for X, 1 for Y, 2 for Z.
fn axis(side: Direction) -> usize {
    side.normal()
        .abs()
        .to_array()
        .iter()
        .position(|&c| c != 0)
        .unwrap()
}

/// The side of its voxel `state` fills half of, if it's a half that can
/// share its voxel with another: its only box is that half, it covers that
/// side completely, the crosshair picks it by that box, it's drawn
/// opaque, and it's a plain block (no fluid, block entity, script,
/// connections, support or parts drawn by their neighbours, and not
/// replaceable), whose voxel holds
/// nothing but its state.
fn half_side(state: &State, block: &Block) -> Option<Direction> {
    let plain = state.render == Render::Opaque
        && state.fluid.is_none()
        && state.conduit.is_none()
        && state.machine.is_none()
        && block.block_entity.is_none()
        && block.script.is_none()
        && block.connections.is_none()
        && block.support.is_none()
        && state.next_parts.is_empty()
        && !state.replaceable;
    let &[only] = &state.collision[..] else {
        return None;
    };
    // The crosshair picks it by that box too, so it can tell the halves
    // of a combined state apart.
    if state.target != [only] {
        return None;
    }
    Direction::ALL
        .into_iter()
        .find(|&side| only == half_box(side) && state.occludes(side))
        .filter(|_| plain)
}

/// Whether `vertex` lies on the plane between the two halves of the axis
/// `side` is on.
fn on_middle(pos: Vec3, side: Direction) -> bool {
    (pos[axis(side)] - 0.5).abs() < 1e-4
}

/// The combined state of `low`, filling `side`, and `high`, filling the
/// opposite side, as part of `block`: both halves' faces but those between
/// them, both boxes (`low`'s first), and every side covered.
fn combined_state(content: &Content, low: BlockState, high: BlockState, block: usize) -> State {
    let (a, b) = (content.state(low), content.state(high));
    let side = a.half.expect("a half");
    let faces: Vec<_> = [a, b]
        .iter()
        .flat_map(|part| &part.faces)
        .filter(|face| !face.vertices.iter().all(|v| on_middle(v.pos, side)))
        .cloned()
        .collect();
    let mut state = State {
        block,
        name: format!("{} and {}", a.name, b.name),
        collision: [a.collision[0], b.collision[0]].to_vec(),
        target: [a.collision[0], b.collision[0]].to_vec(),
        target_reaches: vec![None; 2],
        occludes: ALL_SIDES,
        light: std::array::from_fn(|i| a.light[i].max(b.light[i])),
        light_filter: a.light_filter.max(b.light_filter),
        render: Render::Opaque,
        hardness: a.hardness.max(b.hardness),
        weight: a.weight + b.weight,
        combined: Some([low, high]),
        ..default()
    };
    for side in Direction::ALL {
        state.side_textures[side as usize] = side_texture(&faces, side);
    }
    state.faces = faces;
    state
}

/// Marks every half ([`State::half`]) and adds a combined state for each
/// pair of halves on opposite sides of an axis, in the engine's
/// [`COMBINED`] block, after every mod's blocks. Reports an error, and
/// combines nothing, if there are too many to fit.
pub(super) fn add_combinations(content: &mut Content, errors: &mut Vec<String>) {
    for i in 1..content.states.len() {
        let half = half_side(&content.states[i], &content.blocks[content.states[i].block]);
        content.states[i].half = half;
    }
    let halves = |side: Direction| -> Vec<BlockState> {
        (0..content.states.len())
            .filter(|&i| content.states[i].half == Some(side))
            .map(|i| BlockState(i as u16))
            .collect()
    };
    let pairs: Vec<(BlockState, BlockState)> = LOW_SIDES
        .into_iter()
        .flat_map(|side| {
            let (low, high) = (halves(side), halves(side.opposite()));
            low.into_iter()
                .flat_map(move |a| high.clone().into_iter().map(move |b| (a, b)))
        })
        .collect();
    if pairs.is_empty() {
        return;
    }
    let first = content.states.len();
    if first + pairs.len() > u16::MAX as usize + 1 {
        errors.push(format!(
            "out of block states: the {} ways to put two halves (slabs) together don't fit, \
             so halves don't combine",
            pairs.len()
        ));
        return;
    }
    let block = content.blocks.len();
    content.blocks.push(Block {
        id: COMBINED.into(),
        properties: Vec::new(),
        first_state: first as u16,
        state_count: pairs.len() as u16,
        connections: None,
        block_entity: None,
        script: None,
        support: None,
        next: Vec::new(),
    });
    for (low, high) in pairs {
        let state = combined_state(content, low, high, block);
        let combined = BlockState(content.states.len() as u16);
        content.states.push(state);
        content.combinations.insert((low, high), combined);
    }
}

impl Content {
    /// The side of its voxel `state` fills half of, if it's a half that
    /// can combine with another.
    pub fn half(&self, state: BlockState) -> Option<Direction> {
        self.state(state).half
    }

    /// The two halves `state` is made of, the one on the low side (down,
    /// north or west) first, if it's a combined state.
    pub fn halves(&self, state: BlockState) -> Option<[BlockState; 2]> {
        self.state(state).combined
    }

    /// The combined state of halves `a` and `b`, in either order, if they
    /// fill opposite halves of a voxel.
    pub fn combine(&self, a: BlockState, b: BlockState) -> Option<BlockState> {
        let low = |s: BlockState| self.half(s).is_some_and(|side| LOW_SIDES.contains(&side));
        let key = if low(a) { (a, b) } else { (b, a) };
        self.combinations.get(&key).copied()
    }

    /// What's left of `state` when its half `part` goes: the other half of
    /// a combined state, or air for anything else.
    pub fn without(&self, state: BlockState, part: BlockState) -> BlockState {
        match self.halves(state) {
            Some([low, high]) if low == part => high,
            Some([low, high]) if high == part => low,
            _ => BlockState::AIR,
        }
    }

    /// The half of `state` in its crosshair box number `index` (see
    /// [`State::target`]), which a combined state has one of for each
    /// half; `state` itself for anything else.
    pub fn half_at(&self, state: BlockState, index: usize) -> BlockState {
        self.halves(state)
            .and_then(|halves| halves.get(index).copied())
            .unwrap_or(state)
    }

    /// The state that placing `placing` (the state its item places, with
    /// the properties placing picks from the face and view) into a voxel
    /// holding `existing` makes, if `existing` is a half and `placing`'s
    /// block can fill the other half: the two combined, and the new half,
    /// the state of `placing`'s block on that side with as many of
    /// `placing`'s values as it can keep (all but its half).
    pub fn fill_half(
        &self,
        existing: BlockState,
        placing: BlockState,
    ) -> Option<(BlockState, BlockState)> {
        let side = self.half(existing)?.opposite();
        let half = self.half_like(placing, side)?;
        Some((self.combine(existing, half)?, half))
    }

    /// The state of `state`'s block that fills the half on `side`, with
    /// the same values as `state` for every property placing doesn't pick
    /// (its item's: a slab's rock), and as many of the rest as it can keep.
    fn half_like(&self, state: BlockState, side: Direction) -> Option<BlockState> {
        if self.half(state) == Some(side) {
            return Some(state);
        }
        let block = self.block_of(state);
        let own = &self.state(state).values;
        // How many of `state`'s values `other` keeps, if it keeps every
        // one placing doesn't pick.
        let kept = |other: &State| {
            let mut same = 0;
            let pairs = own.iter().zip(&other.values);
            for (property, (mine, theirs)) in block.properties.iter().zip(pairs) {
                if mine == theirs {
                    same += 1;
                } else if property.place.is_none() {
                    return None;
                }
            }
            Some(same)
        };
        (block.first_state..block.first_state + block.state_count)
            .map(BlockState)
            .filter(|&s| self.state(s).half == Some(side))
            .filter_map(|s| Some((kept(self.state(s))?, s)))
            .max_by_key(|&(same, s)| (same, std::cmp::Reverse(s)))
            .map(|(_, s)| s)
    }

    /// The combined states, by their parts: the low half first.
    pub fn combinations(&self) -> &HashMap<(BlockState, BlockState), BlockState> {
        &self.combinations
    }
}

#[cfg(test)]
mod tests;
