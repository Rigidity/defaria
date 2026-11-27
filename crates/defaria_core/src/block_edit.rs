//! The crosshair, its target, and placing blocks. Breaking them is in
//! `mining`.
//!
//! - While the cursor is locked, holding left click breaks the block under
//!   the crosshair (see `mining`) and right click places the active block
//!   against the face you're looking at, into air or a fluid. The crosshair
//!   passes through fluids, except with a fluid container in hand, when it
//!   stops at them ([`fluid_target`]). A property the block sets on
//!   placement (a log's axis) comes from that face or your view. The click
//!   that locks the cursor doesn't edit anything. The active block is the
//!   active hotbar slot's (see `hotbar`); an item that places no block
//!   places nothing. A block that needs support (a plant, see `support`)
//!   is only placed on a block that supports it.
//! - A `replaceable` block (a plant) is replaced as air is: the crosshair
//!   still picks it (by its model's smaller `target` box), so you can
//!   break it, but a block placed against it goes into its voxel, and
//!   one placed beside another block can go where it stands. In survival
//!   its item goes into the inventory, if it fits. The same plant placed
//!   against one goes beside it, as against any other block.
//! - The crosshair and the outline of the targeted block are drawn by
//!   `crosshair`.
//! - Nothing is edited while a screen (the inventory) is open: no use
//!   happens then (see `use_action`).
//! - Right click is a use (see `use_action`), and placing is its last
//!   step ([`UseStep::Place`]): a block that does something when used (a
//!   machine opens its screen, a script's `on_used` runs) or an item that
//!   does (the wrench, a script's `on_use`) claims the use first, so
//!   nothing is placed.
//!
//! Edits go through [`Voxels`], and the terrain remeshes the changed chunks
//! on the same frame.

use bevy::prelude::*;

use crate::{
    content::{BlockBox, BlockState, Content, Direction, ItemState},
    game_mode::{GameMode, harvest, use_up},
    hotbar::{ActiveSlot, Hotbar},
    inventory::Inventory,
    physics::{Aabb, Body},
    use_action::{UseAction, UseStep},
    world::{FLUID_STEPS, RemeshChunks, VoxelFluid, Voxels, filled, fluid_step, fluid_surface},
};

/// How far away, in blocks, you can break or place.
pub const REACH: f32 = 8.0;

pub struct BlockEditPlugin;

impl Plugin for BlockEditPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            edit_blocks
                .in_set(BlockEditSystems)
                .in_set(UseStep::Place)
                .before(RemeshChunks),
        );
    }
}

/// Where blocks are placed.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockEditSystems;

/// A block a ray ran into.
#[derive(Reflect, Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub block: IVec3,
    /// The face the ray entered through, pointing back toward the ray's
    /// origin. Zero if the ray started inside the block.
    pub normal: IVec3,
    /// Which of the block's boxes it hit, in the order its shape lists
    /// them: in a combined state, which half (see [`Content::half_at`]).
    pub shape_box: usize,
    /// Where the ray met it, in the block's own coordinates (a full block
    /// runs from `(0, 0, 0)` to `(1, 1, 1)`): on the face it entered
    /// through, or the ray's origin if it started inside.
    pub at: Vec3,
}

impl Hit {
    /// Where a block placed against the hit face goes, and which side of
    /// the hit block that face is. `None` if the ray started inside the
    /// block, so there's no face to place against.
    pub fn place_at(self) -> Option<(IVec3, Direction)> {
        let face = Direction::from_normal(self.normal)?;
        Some((self.block + self.normal, face))
    }
}

/// The first block within `reach` along `ray` whose shape the ray hits,
/// where `shape` gives each block's boxes (empty for nothing to hit).
/// Steps through every block the ray touches, in order (Amanatides & Woo),
/// so it can't skip through corners, and tries the ray against the boxes
/// of each, so it passes over a slab's top half to the block behind it.
pub fn raycast<'a>(ray: Ray3d, reach: f32, shape: impl Fn(IVec3) -> &'a [BlockBox]) -> Option<Hit> {
    let (origin, dir) = (ray.origin, *ray.direction);
    let mut block = origin.floor().as_ivec3();
    // Boxes lie inside their block, so the first block the ray hits a box
    // in holds the nearest hit.
    let hit_in = |block: IVec3| {
        let local = origin - block.as_vec3();
        shape(block)
            .iter()
            .enumerate()
            .filter_map(|(i, b)| Some((ray_box(local, dir, b)?, i)))
            .filter(|&((t, _), _)| t <= reach)
            .min_by(|a, b| a.0.0.total_cmp(&b.0.0))
            .map(|((t, normal), shape_box)| Hit {
                block,
                normal,
                shape_box,
                at: local + dir * t,
            })
    };
    if let Some(hit) = hit_in(block) {
        return Some(hit);
    }
    // An axis the ray doesn't move along is never stepped: its `next` is
    // infinite.
    let step = dir.signum().as_ivec3();
    // Ray distance to cross one whole block along each axis, and to the
    // next block boundary along each axis.
    let delta = dir.recip().abs();
    let mut next = Vec3::from_array(std::array::from_fn(|axis| {
        let (o, d) = (origin[axis], dir[axis]);
        if d > 0.0 {
            (o.floor() + 1.0 - o) / d
        } else if d < 0.0 {
            (o - o.floor()) / -d
        } else {
            f32::INFINITY
        }
    }));
    loop {
        let axis = if next.x <= next.y && next.x <= next.z {
            0
        } else if next.y <= next.z {
            1
        } else {
            2
        };
        if next[axis] > reach {
            return None;
        }
        block[axis] += step[axis];
        next[axis] += delta[axis];
        if let Some(hit) = hit_in(block) {
            return Some(hit);
        }
    }
}

/// Where a ray from `origin` along `dir` first meets a box, as the
/// distance along it and the normal of the face it goes in through (zero
/// if it starts inside). Both in the box's block's coordinates.
pub fn ray_box(origin: Vec3, dir: Vec3, b: &BlockBox) -> Option<(f32, IVec3)> {
    let (mut enter, mut exit) = (f32::NEG_INFINITY, f32::INFINITY);
    let mut normal = IVec3::ZERO;
    for axis in 0..3 {
        let (o, d) = (origin[axis], dir[axis]);
        if d == 0.0 {
            if o < b.min[axis] || o > b.max[axis] {
                return None;
            }
            continue;
        }
        let (t0, t1) = ((b.min[axis] - o) / d, (b.max[axis] - o) / d);
        let (near, far) = (t0.min(t1), t0.max(t1));
        if near > enter {
            enter = near;
            normal = IVec3::ZERO;
            normal[axis] = -(d.signum() as i32);
        }
        exit = exit.min(far);
    }
    if enter > exit || exit < 0.0 {
        return None;
    }
    if enter < 0.0 {
        return Some((0.0, IVec3::ZERO));
    }
    Some((enter, normal))
}

/// What the crosshair picks in a block: its target boxes (its model's
/// `target`, or its collision boxes, or the whole block for one you walk
/// through). Nothing for air or a fluid, which the crosshair passes
/// through.
pub fn target_shape(content: &Content, state: BlockState) -> &[BlockBox] {
    &content.state(state).target
}

/// Fluid as the crosshair picks it while aiming at fluid (with a fluid
/// container in hand): a box as
/// high as the fluid is drawn, at one of the [`FLUID_STEPS`] (at least
/// one, so a thin film is still something to aim at), the `n`th `n + 1`
/// steps high.
const FLUID_BOXES: [BlockBox; FLUID_STEPS as usize] = {
    let mut boxes = [BlockBox::FULL; FLUID_STEPS as usize];
    let mut n = 0;
    while n < boxes.len() {
        boxes[n] = BlockBox {
            min: Vec3::ZERO,
            max: Vec3::new(1.0, (n + 1) as f32 / FLUID_STEPS as f32, 1.0),
        };
        n += 1;
    }
    boxes
};

/// What the crosshair picks in a voxel of `state` holding `fluid` while
/// aiming at fluid: the fluid there, as high as it stands up the room the
/// block leaves it ([`FLUID_BOXES`]), or what it always picks
/// ([`target_shape`]).
pub fn fluid_shape(content: &Content, state: BlockState, fluid: Option<VoxelFluid>) -> &[BlockBox] {
    let Some(fluid) = fluid else {
        return target_shape(content, state);
    };
    let room = content.state(state).fluid_room;
    let step = fluid_step(filled(fluid.amount, room));
    let height = fluid_surface(step, room);
    let sixteenths = (height * FLUID_STEPS as f32).round() as u16;
    std::slice::from_ref(&FLUID_BOXES[sixteenths.clamp(1, FLUID_STEPS) as usize - 1])
}

/// The block or fluid under the crosshair of a camera at `transform`,
/// within [`REACH`], aiming at fluid.
pub fn fluid_target(transform: &Transform, content: &Content, voxels: &Voxels) -> Option<Hit> {
    let ray = Ray3d::new(transform.translation, transform.forward());
    raycast(ray, REACH, |pos| match voxels.get_with_fluid(pos) {
        Some((state, partial)) => fluid_shape(content, state, partial),
        None => &[],
    })
}

/// Whether `item`, in hand, aims at fluid rather than through it: a fluid
/// container does (see `fluid_container`).
pub fn aims_at_fluid(content: &Content, item: Option<ItemState>) -> bool {
    item.and_then(|item| content.item_of(item))
        .is_some_and(|item| item.fluid_capacity.is_some())
}

/// The ray from the camera through the crosshair.
fn camera_ray(transform: &Transform) -> Ray3d {
    Ray3d::new(transform.translation, transform.forward())
}

/// The block under the crosshair of a camera at `transform`, within
/// [`REACH`].
pub fn crosshair_target(transform: &Transform, content: &Content, voxels: &Voxels) -> Option<Hit> {
    raycast(camera_ray(transform), REACH, |pos| {
        target_shape(content, voxels.get(pos).unwrap_or(BlockState::AIR))
    })
}

/// Places the block in hand against the face aimed at, for each use no
/// earlier step claimed, if it can go there and, in survival, the player
/// has one. As the last step, it claims every use it sees, so none
/// outlasts its frame.
fn edit_blocks(
    mut players: Query<(
        &mut UseAction,
        &Hotbar,
        &ActiveSlot,
        Option<&mut Inventory>,
        Option<&GameMode>,
    )>,
    content: Res<Content>,
    bodies: Query<&Body>,
    mut voxels: Voxels,
) {
    for (mut action, hotbar, active, inventory, mode) in &mut players {
        let Some(using) = action.pending() else {
            continue;
        };
        action.claim();
        let camera = using.eyes;
        let Some(hit) = using.target else {
            continue;
        };
        let Some(item) = hotbar.item(*active) else {
            continue;
        };
        let Some(block) = hotbar.block(*active, &content) else {
            continue;
        };
        let Some(Placing {
            pos,
            block,
            added,
            replaced,
        }) = placing(&content, &voxels, hit, block, &camera)
        else {
            continue;
        };
        // Not into the camera, outside the world, over another block (but
        // over one placing replaces: air, which may hold fluid, or a
        // plant), or where what it adds would overlap someone.
        let overlaps = content.state(added).collision.iter().any(|b| {
            let b = Aabb::of_block(pos, b);
            bodies.iter().any(|body| body.aabb().intersects(&b))
        });
        // Nor where it can't stand (a flower off the grass).
        let supported = !content.needs_support(block)
            || voxels
                .get(pos - IVec3::Y)
                .is_some_and(|below| content.supports(block, below));
        if pos == camera.translation.floor().as_ivec3() || overlaps || !supported {
            continue;
        }
        // In survival, only what you have, and placing uses it up. A fluid
        // block is creative only: placing one would make fluid from
        // nothing, so in survival fluid comes only from containers (a
        // bucket).
        let mode = mode.copied().unwrap_or_default();
        if !mode.is_creative() && content.state(block).is_fluid() {
            continue;
        }
        let mut inventory = inventory.map(Mut::into_inner);
        if !use_up(mode, inventory.as_deref_mut(), item) {
            continue;
        }
        // A plant built over goes into the inventory in survival, if it
        // fits (otherwise it's lost, as when it loses its support).
        if let Some(inventory) = inventory {
            harvest(mode, inventory, &content, replaced);
        }
        voxels.set(pos, block);
    }
}

/// Whether placing `placed` where `replaced` is replaces it: air (which
/// may hold fluid) or a `replaceable` block (a plant), but not another
/// state of the same block, which would change nothing worth a block.
fn replaces(content: &Content, replaced: BlockState, placed: BlockState) -> bool {
    let replaced = content.state(replaced);
    replaced.replaceable && replaced.block != content.state(placed).block
}

/// Where `block` placed at `hit` goes, and the side of the block hit
/// that it's placed against: into the block hit if placing replaces it
/// (a plant), or else against the face hit (beside a plant of the same
/// kind, so a row of them can be planted). `None` for a ray that
/// started inside a block it doesn't replace.
fn placement(
    content: &Content,
    voxels: &Voxels,
    hit: Hit,
    block: BlockState,
) -> Option<(IVec3, Direction)> {
    let into = voxels
        .get(hit.block)
        .is_some_and(|replaced| replaces(content, replaced, block));
    if !into {
        return hit.place_at();
    }
    // As though placed against the face hit; from inside, the block
    // below.
    let face = Direction::from_normal(hit.normal).unwrap_or(Direction::Up);
    Some((hit.block, face))
}

/// Where a right click places a block, and what it puts there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Placing {
    pos: IVec3,
    /// The voxel's new state.
    block: BlockState,
    /// What's new in it: `block`, or the half it adds to a combined one.
    added: BlockState,
    /// What it replaces, which a survival player gets: a plant, or air
    /// (which may hold fluid). Air too when it fills a half.
    replaced: BlockState,
}

/// What placing `item_block` (the state the item places) at `hit` from a
/// camera at `camera` does, if anything. It goes where [`placement`] says,
/// with properties such as a log's axis from the face clicked, the view
/// or where on the face the click is, and then:
///
/// - placed against a face (not into a plant), it fills the other half of
///   a half: the one clicked, if the click is on its open side (the top
///   of a bottom slab), or else one in the voxel it goes into, if the
///   item's block has a half that fits;
/// - otherwise it goes over what's there if placing replaces it (air, or
///   a plant).
fn placing(
    content: &Content,
    voxels: &Voxels,
    hit: Hit,
    item_block: BlockState,
    camera: &Transform,
) -> Option<Placing> {
    let (pos, face) = placement(content, voxels, hit, item_block)?;
    // Where the click is, in the placed block's voxel. Into a plant, it's
    // as though on the middle of the face hit, since the click is on the
    // plant, not the side of the voxel.
    let placed = if pos == hit.block {
        content.placed(item_block, face, *camera.forward())
    } else {
        let at = (hit.at + (hit.block - pos).as_vec3()).clamp(Vec3::ZERO, Vec3::ONE);
        content.placed_at(item_block, face, *camera.forward(), at)
    };
    if pos != hit.block {
        let fill = |pos: IVec3| {
            let (block, added) = content.fill_half(voxels.get(pos)?, placed)?;
            Some(Placing {
                pos,
                block,
                added,
                replaced: BlockState::AIR,
            })
        };
        let clicked = voxels.get(hit.block).unwrap_or(BlockState::AIR);
        let open_side = content.half(clicked) == Some(face.opposite());
        if let Some(filled) = open_side
            .then(|| fill(hit.block))
            .flatten()
            .or_else(|| fill(pos))
        {
            return Some(filled);
        }
    }
    let replaced = voxels.get(pos)?;
    replaces(content, replaced, placed).then_some(Placing {
        pos,
        block: placed,
        added: placed,
        replaced,
    })
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::content::{base, block, test_mod};

    fn solid_at(blocks: &[IVec3]) -> impl Fn(IVec3) -> &'static [BlockBox] + '_ {
        move |pos| {
            if blocks.contains(&pos) {
                std::slice::from_ref(&BlockBox::FULL)
            } else {
                &[]
            }
        }
    }

    fn ray(origin: Vec3, dir: Vec3) -> Ray3d {
        Ray3d::new(origin, Dir3::new(dir).unwrap())
    }

    #[test]
    fn ray_hits_the_first_block_and_the_face_it_entered() {
        let blocks = [IVec3::new(3, 0, 0), IVec3::new(5, 0, 0)];
        let hit = raycast(ray(Vec3::splat(0.5), Vec3::X), 8.0, solid_at(&blocks));
        assert_eq!(
            hit,
            Some(Hit {
                block: IVec3::new(3, 0, 0),
                normal: IVec3::NEG_X,
                shape_box: 0,
                at: Vec3::new(0.0, 0.5, 0.5),
            })
        );
        assert_eq!(
            hit.unwrap().place_at(),
            Some((IVec3::new(2, 0, 0), Direction::West))
        );
    }

    #[test]
    fn ray_looking_down_hits_the_top_face() {
        let blocks = [IVec3::new(2, 1, 2)];
        let hit = raycast(
            ray(Vec3::new(2.5, 5.2, 2.5), Vec3::NEG_Y),
            8.0,
            solid_at(&blocks),
        )
        .unwrap();
        assert_eq!(hit.normal, IVec3::Y);
        assert_eq!(hit.place_at(), Some((IVec3::new(2, 2, 2), Direction::Up)));
    }

    /// From x = 0.5, block 8's near face is 7.5 away and block 9's is 8.5.
    #[test]
    fn ray_stops_at_its_reach() {
        let origin = Vec3::splat(0.5);
        let near = [IVec3::new(8, 0, 0)];
        let far = [IVec3::new(9, 0, 0)];
        assert!(raycast(ray(origin, Vec3::X), 8.0, solid_at(&near)).is_some());
        assert_eq!(raycast(ray(origin, Vec3::X), 8.0, solid_at(&far)), None);
    }

    #[test]
    fn ray_works_at_negative_coordinates() {
        let blocks = [IVec3::new(-3, -1, 0)];
        let hit = raycast(
            ray(Vec3::new(-0.5, -0.5, 0.5), Vec3::NEG_X),
            8.0,
            solid_at(&blocks),
        )
        .unwrap();
        assert_eq!(hit.block, IVec3::new(-3, -1, 0));
        assert_eq!(hit.normal, IVec3::X);
    }

    /// A diagonal ray steps through every block it touches, so it can't
    /// slip between two blocks that only meet at an edge.
    #[test]
    fn diagonal_ray_visits_every_block_it_crosses() {
        let visited = std::cell::RefCell::new(Vec::new());
        raycast(
            ray(Vec3::new(0.2, 0.5, 0.7), Vec3::new(1.0, 0.0, -1.0)),
            3.0,
            |pos| {
                visited.borrow_mut().push(pos);
                &[]
            },
        );
        let visited = visited.into_inner();
        for pair in visited.windows(2) {
            let step = (pair[1] - pair[0]).abs();
            assert_eq!(step.x + step.y + step.z, 1, "{pair:?}");
        }
        assert!(visited.len() >= 4);
    }

    /// A ray passes over a slab's empty top half to the block behind, and
    /// hits its top face from above, at y = 0.5.
    #[test]
    fn rays_hit_the_shape_of_the_block() {
        let content = base();
        let slab = block("stone_slab");
        let stone = block("stone");
        let at = |pos: IVec3| match pos.to_array() {
            [2, 0, 0] => slab,
            [4, 0, 0] => stone,
            _ => BlockState::AIR,
        };
        let shape = |pos| target_shape(content, at(pos));
        // Over the slab, at y = 0.75.
        let hit = raycast(ray(Vec3::new(0.5, 0.75, 0.5), Vec3::X), 8.0, shape).unwrap();
        assert_eq!(hit.block, IVec3::new(4, 0, 0));
        // Into its side, at y = 0.25.
        let hit = raycast(ray(Vec3::new(0.5, 0.25, 0.5), Vec3::X), 8.0, shape).unwrap();
        assert_eq!((hit.block, hit.normal), (IVec3::new(2, 0, 0), IVec3::NEG_X));
        // Straight down onto its top, which places on the block above.
        let hit = raycast(ray(Vec3::new(2.5, 3.0, 0.5), Vec3::NEG_Y), 8.0, shape).unwrap();
        assert_eq!((hit.block, hit.normal), (IVec3::new(2, 0, 0), IVec3::Y));
        assert_eq!(hit.place_at(), Some((IVec3::new(2, 1, 0), Direction::Up)));
        // The top is 2.5 below, so a reach of 2.4 doesn't get there, though
        // the slab's block starts 2 below.
        assert_eq!(
            raycast(ray(Vec3::new(2.5, 3.0, 0.5), Vec3::NEG_Y), 2.4, shape),
            None
        );
        // From inside the slab's empty half, the ray still finds the slab
        // below it.
        let hit = raycast(ray(Vec3::new(2.5, 0.75, 0.5), Vec3::NEG_Y), 8.0, shape).unwrap();
        assert_eq!((hit.block, hit.normal), (IVec3::new(2, 0, 0), IVec3::Y));
    }

    /// A block you walk through is still picked, as a whole block; air
    /// isn't picked at all.
    #[test]
    fn blocks_with_no_collision_are_picked_whole() {
        let content = test_mod(&[
            ("models/ghost.json5", r#"{ collision: [], faces: [] }"#),
            ("blocks/ghost.json5", r#"{ model: "ghost" }"#),
        ]);
        assert!(content.errors.is_empty(), "{:?}", content.errors);
        let ghost = content.default_state("test:ghost").unwrap();
        assert!(content.state(ghost).collision.is_empty());
        assert_eq!(target_shape(&content, ghost), [BlockBox::FULL]);
        assert_eq!(target_shape(&content, BlockState::AIR), []);
        let at = |pos: IVec3| {
            if pos == IVec3::new(3, 0, 0) {
                ghost
            } else {
                BlockState::AIR
            }
        };
        let hit = raycast(ray(Vec3::splat(0.5), Vec3::X), 8.0, |pos| {
            target_shape(&content, at(pos))
        })
        .unwrap();
        assert_eq!((hit.block, hit.normal), (IVec3::new(3, 0, 0), IVec3::NEG_X));
    }

    #[test]
    fn ray_starting_inside_a_block_hits_it_with_no_face() {
        let blocks = [IVec3::ZERO];
        let hit = raycast(ray(Vec3::splat(0.5), Vec3::X), 8.0, solid_at(&blocks)).unwrap();
        assert_eq!(hit.block, IVec3::ZERO);
        assert_eq!(hit.place_at(), None);
    }

    /// The crosshair picks a plant by its small box: a ray through the
    /// top of its voxel, or past its corner, goes on to the block behind.
    #[test]
    fn plants_are_picked_by_a_smaller_box() {
        let content = base();
        let tuft = block("tuft");
        let at = |pos: IVec3| match pos.to_array() {
            [2, 0, 0] => tuft,
            [4, 0, 0] => block("stone"),
            _ => BlockState::AIR,
        };
        let shape = |pos| target_shape(content, at(pos));
        let hit = |origin: Vec3| raycast(ray(origin, Vec3::X), 8.0, shape).unwrap();
        // Through its middle, it's hit on the face of its box.
        let middle = hit(Vec3::new(0.5, 0.5, 0.5));
        assert_eq!(
            (middle.block, middle.normal),
            (IVec3::new(2, 0, 0), IVec3::NEG_X)
        );
        // Over its top, and past its side, the stone behind.
        assert_eq!(hit(Vec3::new(0.5, 0.95, 0.5)).block, IVec3::new(4, 0, 0));
        assert_eq!(hit(Vec3::new(0.5, 0.5, 0.05)).block, IVec3::new(4, 0, 0));
    }
}
