//! Parts drawn by what's next to them: a part's `next` says what the
//! block on some of its sides must (or mustn't) be for it to be there, so
//! a wire can bend to meet a neighbour resting on another side of its
//! voxel. Whatever reads a voxel's look or shape reads them from the
//! voxels around: meshing its faces ([`Content::next_faces`]), physics its
//! collision boxes ([`Content::collision`]) and the crosshair and the
//! wrench its target boxes ([`Content::target`]). They cover no sides, so
//! light and culling don't change with the neighbours.
//!
//! A state keeps such parts' faces and boxes apart from its own
//! ([`State::next_faces`], [`State::next_collision`],
//! [`State::next_target`]), each part with the number of its rule on the
//! block ([`Block::next`]). The rules name other blocks, so they're worked
//! out once every block is in, as `support` is.

use std::{borrow::Cow, ops::Range};

use super::{
    BlockBox, BlockState, Content, Direction, State,
    bake::When,
    index_of, qualify,
    schema::{BlockDef, NextDef},
    state_count,
};

/// A part of a state drawn only where its rule matches the neighbours.
#[derive(Debug, Clone, PartialEq)]
pub struct NextPart {
    /// Its rule, in [`Block::next`](super::Block::next).
    pub rule: usize,
    /// Its faces, in [`State::next_faces`].
    pub faces: Range<usize>,
    /// Its collision boxes, in [`State::next_collision`].
    pub collision: Range<usize>,
    /// Its target boxes, in [`State::next_target`] (and their sides in
    /// [`State::next_target_reaches`]).
    pub target: Range<usize>,
}

/// What the crosshair picks in a voxel ([`Content::target`]): its boxes,
/// and for each the side its part reaches out to, if any (an arm's).
#[derive(Debug, Clone, PartialEq)]
pub struct Target<'a> {
    pub boxes: Cow<'a, [BlockBox]>,
    pub reaches: Cow<'a, [Option<Direction>]>,
}

impl Target<'_> {
    /// The side the box at `index` reaches, if any.
    pub fn reach(&self, index: usize) -> Option<Direction> {
        self.reaches.get(index).copied().flatten()
    }
}

/// What a part's neighbours must be: for each side it names, the states
/// that match, sorted, and whether the neighbour must be one of them or
/// anything else.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NextRule {
    pub sides: Vec<(Direction, Vec<BlockState>, bool)>,
}

impl NextRule {
    /// Whether the neighbours `next` gives on each side match.
    pub fn matches(&self, mut next: impl FnMut(Direction) -> BlockState) -> bool {
        self.sides
            .iter()
            .all(|(side, states, is)| states.binary_search(&next(*side)).is_ok() == *is)
    }
}

/// Works out the rules of the parts with a `next` of the block at `index`
/// in [`Content::blocks`], in part order, and keeps them on the block. An
/// error leaves it with none, and those parts aren't drawn.
pub(super) fn bake(
    index: usize,
    def: &BlockDef,
    namespace: &str,
    content: &mut Content,
) -> Result<(), String> {
    let mut rules = Vec::new();
    for (i, part) in def.parts.iter().enumerate() {
        if part.next.is_empty() {
            continue;
        }
        let rule = rule(&part.next, namespace, content)
            .map_err(|error| format!("part {}: `next`: {error}", i + 1))?;
        rules.push(rule);
    }
    content.blocks[index].next = rules;
    Ok(())
}

fn rule(
    next: &std::collections::BTreeMap<Direction, NextDef>,
    namespace: &str,
    content: &Content,
) -> Result<NextRule, String> {
    let mut sides = Vec::new();
    for (&side, def) in next {
        let (matching, is) = match def {
            NextDef::Is(matching) => (matching, true),
            NextDef::Not(not) => (&not.not, false),
        };
        let id = qualify(&matching.block, namespace)?;
        let block = content
            .block(&id)
            .ok_or_else(|| format!("`{id}` isn't a block"))?;
        let when = When::new(&matching.when, &block.properties, namespace, content)
            .map_err(|error| format!("`{id}`: {error}"))?;
        let states = (0..state_count(&block.properties))
            .filter(|&offset| when.matches(&index_of(&block.properties, offset)))
            .map(|offset| BlockState(block.first_state + offset as u16))
            .collect();
        sides.push((side, states, is));
    }
    Ok(NextRule { sides })
}

impl Content {
    /// The parts of `state` drawn by their neighbours whose rules match
    /// the blocks `next` gives on each side.
    fn next_parts_matching(
        &self,
        state: BlockState,
        mut next: impl FnMut(Direction) -> BlockState,
    ) -> impl Iterator<Item = &NextPart> {
        let rules = &self.block_of(state).next;
        self.state(state).next_parts.iter().filter(move |part| {
            rules
                .get(part.rule)
                .is_some_and(|rule| rule.matches(&mut next))
        })
    }

    /// What things bump into in a voxel of `state`: its collision boxes,
    /// and those of its parts drawn by their neighbours whose rules match
    /// the blocks `next` gives on each side. Borrowed for a block without
    /// such parts.
    pub fn collision(
        &self,
        state: BlockState,
        next: impl FnMut(Direction) -> BlockState,
    ) -> Cow<'_, [BlockBox]> {
        let baked: &State = self.state(state);
        if baked.next_parts.is_empty() {
            return Cow::Borrowed(&baked.collision);
        }
        let mut boxes = baked.collision.clone();
        for part in self.next_parts_matching(state, next) {
            boxes.extend_from_slice(&baked.next_collision[part.collision.clone()]);
        }
        Cow::Owned(boxes)
    }

    /// What the crosshair picks in a voxel of `state`: its target boxes,
    /// and those of its parts drawn by their neighbours whose rules match
    /// the blocks `next` gives on each side, each with the side its part
    /// reaches; the whole block if that leaves none, as for any block with
    /// nothing else to aim at. Borrowed for a block without such parts.
    pub fn target(
        &self,
        state: BlockState,
        next: impl FnMut(Direction) -> BlockState,
    ) -> Target<'_> {
        let baked: &State = self.state(state);
        if baked.next_parts.is_empty() {
            return Target {
                boxes: Cow::Borrowed(&baked.target),
                reaches: Cow::Borrowed(&baked.target_reaches),
            };
        }
        let mut boxes = baked.target.clone();
        let mut reaches = baked.target_reaches.clone();
        reaches.resize(boxes.len(), None);
        for part in self.next_parts_matching(state, next) {
            boxes.extend_from_slice(&baked.next_target[part.target.clone()]);
            reaches.extend_from_slice(&baked.next_target_reaches[part.target.clone()]);
        }
        if boxes.is_empty() {
            boxes.push(BlockBox::FULL);
            reaches.push(None);
        }
        Target {
            boxes: Cow::Owned(boxes),
            reaches: Cow::Owned(reaches),
        }
    }

    /// The faces of `state`'s parts drawn by their neighbours whose rules
    /// match the blocks `next` gives on each side, each with its index in
    /// [`State::next_faces`].
    pub fn next_faces(
        &self,
        state: BlockState,
        next: impl FnMut(Direction) -> BlockState,
    ) -> impl Iterator<Item = (usize, &super::BakedFace)> {
        let baked: &State = self.state(state);
        self.next_parts_matching(state, next)
            .flat_map(move |part| part.faces.clone().map(move |i| (i, &baked.next_faces[i])))
    }
}

#[cfg(test)]
mod tests;
