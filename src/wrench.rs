//! The wrench: right click with it on a conduit (a wire, pipe or tube) to
//! block one of its faces, so it never joins what's next to it there, and
//! again to open it.
//!
//! The face is the one aimed at: on one of the conduit's arms, the side
//! that arm reaches out to (so clicking the arm between two pipes parts
//! them); on its core, or a conduit drawn as a whole block, the face
//! clicked (so a side can be closed before anything is put next to it).
//! Aimed at another block's face (a machine's port) where a conduit next
//! to it has its face blocked, it opens that face again, since the arm
//! that was there to click is gone.
//!
//! An item does this when its file says `action: "wrench"`. The faces are
//! kept in the conduit's block entity, as [`BlockedFaces`] (a part this
//! plugin adds), which `connections` reads, so the connection properties, the arms drawn and
//! the networks all follow. A wrench places nothing. Its click is a use
//! of the block, in [`BlockUseSystems`] before the others: with a wrench
//! in hand, right click on any block is the wrench's, so it never opens a
//! machine's screen.

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use crate::{
    block_edit::{BlockUseSystems, Hit, crosshair_target, ray_box, target_shape},
    block_entity::{BlockEntities, RegisterBlockEntityPart},
    connections::{Blocked, BlockedFaces},
    content::{BlockBox, BlockState, Content, Direction, ItemAction},
    fly_camera::FlyCamera,
    hotbar::{ActiveSlot, Hotbar},
    machine::screen::open_machine_screen,
    player::LocalPlayer,
    terrain::Voxels,
};

pub struct WrenchPlugin;

impl Plugin for WrenchPlugin {
    fn build(&self, app: &mut App) {
        app.register_block_entity_part::<BlockedFaces>()
            .add_systems(
                Update,
                use_wrench
                    .in_set(BlockUseSystems)
                    // Without a sync point, as the set has none.
                    .before_ignore_deferred(open_machine_screen),
            );
    }
}

/// Whether `item`, in the hand, is a wrench.
fn is_wrench(content: &Content, hotbar: &Hotbar, active: ActiveSlot) -> bool {
    hotbar
        .item(active)
        .and_then(|item| content.item_state(item))
        .is_some_and(|info| content.items[info.item].action == Some(ItemAction::Wrench))
}

/// The side of the block of `state` a wrench aimed along `ray` at `hit`
/// works on: the side the box it hits first reaches (an arm), or, for a
/// box that reaches no side or more than one (a core, a whole block), the
/// face it went in through. `None` if the ray started inside the block.
pub fn wrench_side(
    content: &Content,
    state: BlockState,
    ray: Ray3d,
    hit: Hit,
) -> Option<Direction> {
    let face = Direction::from_normal(hit.normal)?;
    let local = ray.origin - hit.block.as_vec3();
    let first = target_shape(content, state)
        .iter()
        .filter_map(|b| ray_box(local, *ray.direction, b).map(|(t, _)| (t, b)))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    Some(first.and_then(|(_, b)| arm_of(b)).unwrap_or(face))
}

/// The one side of its block that a box touches, if it touches exactly
/// one: the way an arm reaches.
fn arm_of(b: &BlockBox) -> Option<Direction> {
    const EDGE: f32 = 1e-4;
    let mut touching = Direction::ALL.into_iter().filter(|side| {
        let n = side.normal().as_vec3();
        let axis = n.abs().max_position();
        if n[axis] > 0.0 {
            b.max[axis] >= 1.0 - EDGE
        } else {
            b.min[axis] <= EDGE
        }
    });
    let side = touching.next()?;
    touching.next().is_none().then_some(side)
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
        let side = wrench_side(content, state, ray, hit)?;
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
/// ([`wrench_target`]), on right click. It takes every right click on a
/// block while in hand, whether it turns a face or not.
pub(crate) fn use_wrench(
    mut commands: Commands,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    camera: Single<&Transform, With<FlyCamera>>,
    player: Option<Single<(&Hotbar, &ActiveSlot), With<LocalPlayer>>>,
    content: Res<Content>,
    voxels: Voxels,
    index: Res<BlockEntities>,
    blocked: Blocked,
) {
    if cursor.grab_mode == CursorGrabMode::None || !mouse.just_pressed(MouseButton::Right) {
        return;
    }
    let Some(player) = player else {
        return;
    };
    let (hotbar, active) = player.into_inner();
    if !is_wrench(&content, hotbar, *active) {
        return;
    }
    let Some(hit) = crosshair_target(&camera, &content, &voxels) else {
        return;
    };
    mouse.clear_just_pressed(MouseButton::Right);
    let ray = Ray3d::new(camera.translation, camera.forward());
    let at = |pos| blocked.at(pos);
    let Some((pos, side)) = wrench_target(&content, &voxels, at, ray, hit) else {
        return;
    };
    let Some(entity) = index.get(pos) else {
        return;
    };
    // Taken off whole, still blocked, so what's around it is put right.
    match turned(Some(blocked.at(pos)), side) {
        Some(faces) => commands.entity(entity).insert(faces),
        None => commands.entity(entity).remove::<BlockedFaces>(),
    };
}

#[cfg(test)]
mod tests;
