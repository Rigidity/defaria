//! The wrench: right click with it on a conduit (a wire, pipe or tube) to
//! block one of its faces, so it never joins what's next to it there, and
//! again to open it.
//!
//! The face is the one aimed at: on one of the conduit's arms, the side
//! that arm reaches out to (so clicking the arm between two pipes parts
//! them); on its core, or a conduit drawn as a whole block, the face
//! clicked (so a side can be closed before anything is put next to it).
//! Which side an arm reaches is its part's: the one connection its `when`
//! needs joined ([`Target::reach`]), bends drawn by the neighbours
//! included, so the wrench never guesses from the boxes' shapes.
//! Aimed at another block's face (a machine's port) where a conduit next
//! to it has its face blocked, it opens that face again, since the arm
//! that was there to click is gone.
//!
//! An item does this when its file says `action: "wrench"`. The faces are
//! kept in the conduit's block entity, as [`BlockedFaces`] (a part this
//! plugin adds), which `connections` reads, so the connection properties, the arms drawn and
//! the networks all follow. A wrench places nothing. Its use comes first
//! ([`UseStep::Wrench`]): with a wrench in hand, right click on any block
//! is the wrench's, so it never opens a machine's screen.

use bevy::prelude::*;

use crate::{
    block_entity::{BlockEntities, RegisterBlockEntityPart},
    connections::{Blocked, BlockedFaces},
};
use defaria_core::{
    block_edit::{Hit, ray_box, target_at},
    content::{Content, Direction, ItemAction, Target},
    hotbar::{ActiveSlot, Hotbar},
    use_action::{UseAction, UseStep},
    world::Voxels,
};

pub struct WrenchPlugin;

impl Plugin for WrenchPlugin {
    fn build(&self, app: &mut App) {
        app.register_block_entity_part::<BlockedFaces>()
            .add_systems(Update, use_wrench.in_set(UseStep::Wrench));
    }
}

/// Whether `item`, in the hand, is a wrench.
fn is_wrench(content: &Content, hotbar: &Hotbar, active: ActiveSlot) -> bool {
    hotbar
        .item(active)
        .and_then(|item| content.item_state(item))
        .is_some_and(|info| content.items[info.item].action == Some(ItemAction::Wrench))
}

/// The side of a block with `target` (what the crosshair picks in it, see
/// [`target_at`]) a wrench aimed along `ray` at `hit` works on: the side
/// the part of the box it hits first reaches (an arm's), or, for a box
/// whose part reaches none (a core, a whole block), the face it went in
/// through. `None` if the ray started inside the block.
pub fn wrench_side(target: &Target, ray: Ray3d, hit: Hit) -> Option<Direction> {
    let face = Direction::from_normal(hit.normal)?;
    let local = ray.origin - hit.block.as_vec3();
    let first = target
        .boxes
        .iter()
        .enumerate()
        .filter_map(|(i, b)| ray_box(local, *ray.direction, b).map(|(t, _)| (t, i)))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    Some(first.and_then(|(_, i)| target.reach(i)).unwrap_or(face))
}

/// A conduit's blocked faces with `side` turned: blocked if it was open,
/// opened if it was blocked. `None` when that leaves every face open, as a
/// conduit then has no [`BlockedFaces`].
pub fn turned(faces: Option<BlockedFaces>, side: Direction) -> Option<BlockedFaces> {
    let mut faces = faces.unwrap_or_default();
    faces.toggle(side);
    (!faces.is_empty()).then_some(faces)
}

/// The conduit face a wrench aimed along `ray` at `hit` turns: one of the
/// conduit's own ([`wrench_side`]), or, on another block, the face of the
/// conduit next to the face clicked, if that's blocked (it can only be
/// opened from there). On a conduit's open face toward another conduit
/// that has blocked its face back, it's that one's, so a face blocked
/// between two conduits opens from either.
fn wrench_target(
    content: &Content,
    voxels: &Voxels,
    blocked: impl Fn(IVec3) -> BlockedFaces,
    ray: Ray3d,
    hit: Hit,
) -> Option<(IVec3, Direction)> {
    let state = voxels.get(hit.block)?;
    if content.block_of(state).is_conduit() {
        let side = wrench_side(&target_at(content, voxels, hit.block), ray, hit)?;
        let next = hit.block + side.normal();
        let across = voxels
            .get(next)
            .is_some_and(|state| content.block_of(state).is_conduit());
        if !blocked(hit.block).has(side) && across && blocked(next).has(side.opposite()) {
            return Some((next, side.opposite()));
        }
        return Some((hit.block, side));
    }
    let face = Direction::from_normal(hit.normal)?;
    let next = hit.block + face.normal();
    let side = face.opposite();
    (content.block_of(voxels.get(next)?).is_conduit() && blocked(next).has(side))
        .then_some((next, side))
}

/// Blocks or opens the face of the conduit that the wrench is aimed at
/// ([`wrench_target`]), on a use. It claims every use on a block while in
/// hand, whether it turns a face or not.
fn use_wrench(
    mut commands: Commands,
    mut players: Query<(&mut UseAction, &Hotbar, &ActiveSlot)>,
    content: Res<Content>,
    voxels: Voxels,
    index: Res<BlockEntities>,
    blocked: Blocked,
) {
    for (mut action, hotbar, active) in &mut players {
        let Some(using) = action.pending() else {
            continue;
        };
        if !is_wrench(&content, hotbar, *active) {
            continue;
        }
        let Some(hit) = using.target else {
            continue;
        };
        action.claim();
        let ray = Ray3d::new(using.eyes.translation, using.eyes.forward());
        let at = |pos| blocked.at(pos);
        let Some((pos, side)) = wrench_target(&content, &voxels, at, ray, hit) else {
            continue;
        };
        let Some(entity) = index.get(pos) else {
            continue;
        };
        // Taken off whole, still blocked, so what's around it is put right.
        match turned(Some(blocked.at(pos)), side) {
            Some(faces) => commands.entity(entity).insert(faces),
            None => commands.entity(entity).remove::<BlockedFaces>(),
        };
    }
}

#[cfg(test)]
mod tests;
