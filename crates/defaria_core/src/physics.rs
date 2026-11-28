//! Axis-aligned box physics against the voxel world, Minecraft-style.
//!
//! A [`Body`] is a box that moves through the world's block shapes (each
//! block state's collision boxes, see `content`). Moving one ([`move_box`])
//! works the way Minecraft's entities move:
//!
//! - **One axis at a time**: up/down first, then the horizontal axis with
//!   the larger motion, then the other. Each axis's motion is cut short at
//!   the first box in its way, so running into a wall at an angle keeps the
//!   motion along it: you slide.
//! - **Swept, so nothing tunnels**: an axis's motion is checked against
//!   every box in the whole stretch it covers, not just where it ends up, so
//!   no speed is too fast to pass through a block. Long moves are also split
//!   into steps of at most [`MAX_STEP`] per axis, so a fast diagonal move
//!   goes round corners the way a slow one would, rather than all of one
//!   axis and then all of the other.
//! - **Stepping up**: a body on the ground that runs into something up to
//!   its step height tall (a slab, at the player's 0.6) climbs onto it,
//!   when that gets it further than not climbing.
//! - **Not off edges**: a sneaking body ([`guard_edges`]) doesn't walk off
//!   an edge it would drop more than its step height from.
//!
//! Positions are `f64`, and boxes are compared with a small tolerance
//! ([`EPSILON`]), so a body resting on the ground or against a wall stays
//! put and slides freely along it even millions of blocks from the origin,
//! where an `f32` position would be off by whole fractions of a block.
//!
//! Chunks that aren't loaded are solid, so nothing falls or walks into the
//! unloaded world; a body inside one doesn't move at all (see `player`).
//!
//! Bodies move in [`FixedUpdate`], at [`STEP_HZ`], so how they move doesn't
//! depend on the frame rate: a slow frame runs several steps, and
//! `Time<Virtual>` caps how many after a long stall. Whatever draws a body
//! between steps uses [`Body::interpolated`].

use std::{borrow::Cow, time::Duration};

use bevy::{ecs::system::SystemParam, math::DVec3, prelude::*};

use crate::{
    content::{BlockBox, BlockState, Content},
    world::{Chunk, ChunkCoord, ChunkMap},
};

/// Physics steps per second.
pub const STEP_HZ: u64 = 60;
/// The time one physics step covers.
pub const STEP: Duration = Duration::from_nanos(1_000_000_000 / STEP_HZ);
/// How far two boxes may overlap and still count as only touching, in
/// blocks. Far bigger than `f64`'s rounding error anywhere a player can
/// reach, far smaller than anything anyone would see.
pub const EPSILON: f64 = 1e-7;
/// How far a body may be inside a box and still be pushed back out of it
/// before it moves, rather than treated as stuck in it: rounding (an `f32`
/// position, say) leaves it far less than this in.
pub const MAX_PUSH: f64 = 1e-3;
/// The most a body moves along any axis in one go; longer moves are split.
pub const MAX_STEP: f64 = 0.5;
/// The furthest a body moves in one call, in blocks. Anything faster is
/// slowed to this, so a runaway velocity can't make a step take forever.
pub const MAX_MOVE: f64 = 256.0;

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Time::<Fixed>::from_duration(STEP))
            .add_systems(FixedFirst, remember_positions);
    }
}

/// Keeps where each body was before this step, for drawing it between
/// steps.
fn remember_positions(mut bodies: Query<&mut Body>) {
    for mut body in &mut bodies {
        body.previous = body.position;
    }
}

/// An axis-aligned box in world coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Reflect)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Aabb {
    pub fn new(min: DVec3, max: DVec3) -> Self {
        Self { min, max }
    }

    /// A block's box `b`, for the block at `block`.
    pub fn of_block(block: IVec3, b: &BlockBox) -> Self {
        let origin = block.as_dvec3();
        Self::new(origin + b.min.as_dvec3(), origin + b.max.as_dvec3())
    }

    /// The box `size` across, with the middle of its bottom at `feet`.
    pub fn standing(feet: DVec3, size: DVec3) -> Self {
        let half = DVec3::new(size.x / 2.0, 0.0, size.z / 2.0);
        Self::new(feet - half, feet + half + DVec3::Y * size.y)
    }

    pub fn offset(self, by: DVec3) -> Self {
        Self::new(self.min + by, self.max + by)
    }

    /// This box stretched to also cover itself moved by `motion`.
    pub fn stretch(self, motion: DVec3) -> Self {
        Self::new(
            self.min + motion.min(DVec3::ZERO),
            self.max + motion.max(DVec3::ZERO),
        )
    }

    /// Whether the boxes overlap by more than [`EPSILON`] on `axis`.
    fn overlaps_on(&self, other: &Aabb, axis: usize) -> bool {
        self.max[axis] > other.min[axis] + EPSILON && self.min[axis] < other.max[axis] - EPSILON
    }

    /// Whether the boxes overlap by more than [`EPSILON`] on every axis:
    /// touching isn't overlapping.
    pub fn intersects(&self, other: &Aabb) -> bool {
        (0..3).all(|axis| self.overlaps_on(other, axis))
    }

    /// How far this box can move by `motion` along `axis` before running
    /// into `other`: `motion` if `other` isn't in the way.
    fn clip(&self, other: &Aabb, axis: usize, motion: f64) -> f64 {
        let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
        if !self.overlaps_on(other, a) || !self.overlaps_on(other, b) {
            return motion;
        }
        if motion > 0.0 && other.min[axis] >= self.max[axis] - EPSILON {
            motion.min((other.min[axis] - self.max[axis]).max(0.0))
        } else if motion < 0.0 && other.max[axis] <= self.min[axis] + EPSILON {
            motion.max((other.max[axis] - self.min[axis]).min(0.0))
        } else {
            // Beside it, or already inside it: a body stuck in a block can
            // always move out. (One only a hair inside is pushed out before
            // it moves; see `push_out`.)
            motion
        }
    }

    /// The axis the boxes overlap least along: the way out of each other.
    fn shallowest_overlap(&self, other: &Aabb) -> usize {
        let depth =
            |axis: usize| (self.max[axis] - other.min[axis]).min(other.max[axis] - self.min[axis]);
        (0..3)
            .min_by(|&a, &b| depth(a).total_cmp(&depth(b)))
            .unwrap_or(1)
    }
}

/// A box that moves through the world, such as the player.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct Body {
    /// The middle of the bottom of its box.
    pub position: DVec3,
    /// [`Body::position`] before the last physics step, for drawing it
    /// smoothly between steps. Kept up to date in [`FixedFirst`].
    pub previous: DVec3,
    /// Blocks per second.
    pub velocity: DVec3,
    /// Width, height and depth.
    pub size: DVec3,
    /// The tallest step it climbs without jumping.
    pub step_height: f64,
    /// Whether it ended the last step resting on something.
    pub on_ground: bool,
}

impl Body {
    /// A body standing still at `position`.
    pub fn new(position: DVec3, size: DVec3, step_height: f64) -> Self {
        Self {
            position,
            previous: position,
            velocity: DVec3::ZERO,
            size,
            step_height,
            on_ground: false,
        }
    }

    pub fn aabb(&self) -> Aabb {
        Aabb::standing(self.position, self.size)
    }

    /// Puts it somewhere at once, standing still, with no movement to draw
    /// between the old place and the new.
    pub fn teleport(&mut self, position: DVec3) {
        self.position = position;
        self.previous = position;
        self.velocity = DVec3::ZERO;
        self.on_ground = false;
    }

    /// Moves it through the world by `motion` for this step, stopping and
    /// sliding as [`move_box`] does, and sets its velocity to `velocity`
    /// except on the axes something stopped it on.
    pub fn advance(&mut self, motion: DVec3, velocity: DVec3, shapes: &impl Shapes) -> Moved {
        let moved = move_box(
            self.aabb(),
            motion,
            self.step_height,
            self.on_ground,
            shapes,
        );
        self.position += moved.offset;
        self.velocity = DVec3::select(moved.blocked, DVec3::ZERO, velocity);
        self.on_ground = moved.landed;
        moved
    }

    /// Where to draw it, `fraction` of the way through the step after the
    /// last one.
    pub fn interpolated(&self, fraction: f64) -> DVec3 {
        self.previous.lerp(self.position, fraction)
    }
}

/// The block shapes in the world, by block position, for the collision
/// functions: a block's boxes in its own coordinates, or `None` where the
/// world isn't loaded (which is solid).
pub trait Shapes {
    fn shape(&self, block: IVec3) -> Option<Cow<'_, [BlockBox]>>;
}

/// [`Shapes`] read from the loaded chunks.
#[derive(SystemParam)]
pub struct WorldShapes<'w, 's> {
    map: Res<'w, ChunkMap>,
    chunks: Query<'w, 's, &'static Chunk>,
    content: Res<'w, Content>,
}

impl WorldShapes<'_, '_> {
    /// The block at `block`, or `None` where the world isn't loaded.
    fn state(&self, block: IVec3) -> Option<BlockState> {
        let (coord, local) = ChunkCoord::split(block);
        let chunk = self.chunks.get(self.map.get(coord)?).ok()?;
        Some(chunk.at(local))
    }
}

impl Shapes for WorldShapes<'_, '_> {
    /// A block's collision boxes, with those of its parts drawn by the
    /// blocks around it ([`Content::collision`]; unloaded ones count as
    /// air).
    fn shape(&self, block: IVec3) -> Option<Cow<'_, [BlockBox]>> {
        let state = self.state(block)?;
        Some(self.content.collision(state, |side| {
            self.state(block + side.normal()).unwrap_or(BlockState::AIR)
        }))
    }
}

/// The whole block, for the unloaded world.
const FULL: &[BlockBox] = &[BlockBox::FULL];

/// Calls `f` with each world box in the blocks `region` overlaps or
/// touches, until it returns true. Returns whether it did.
fn any_box_in(region: Aabb, shapes: &impl Shapes, mut f: impl FnMut(Aabb) -> bool) -> bool {
    let min = region.min.floor().as_ivec3();
    let max = region.max.floor().as_ivec3();
    for x in min.x..=max.x {
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                let block = IVec3::new(x, y, z);
                for b in shapes.shape(block).as_deref().unwrap_or(FULL) {
                    if f(Aabb::of_block(block, b)) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Whether any world box overlaps `aabb`.
pub fn collides(aabb: Aabb, shapes: &impl Shapes) -> bool {
    any_box_in(aabb, shapes, |b| b.intersects(&aabb))
}

/// What happened when a box moved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moved {
    /// How far it went.
    pub offset: DVec3,
    /// The axes something stopped it on.
    pub blocked: BVec3,
    /// Whether it came down onto something: moving down, it was stopped,
    /// or it stepped up onto something.
    pub landed: bool,
}

/// Moves `aabb` by `motion` through the world, stopping at whatever is in
/// the way on each axis and sliding along it. With a `step_height`, and
/// `grounded` (standing on something before moving) or landing on the way,
/// it climbs onto anything up to that tall it runs into. See the module
/// docs.
pub fn move_box(
    aabb: Aabb,
    motion: DVec3,
    step_height: f64,
    grounded: bool,
    shapes: &impl Shapes,
) -> Moved {
    let mut moved = Moved {
        offset: DVec3::ZERO,
        blocked: BVec3::FALSE,
        landed: false,
    };
    if !motion.is_finite() {
        return moved;
    }
    let motion = motion.clamp_length_max(MAX_MOVE);
    let steps = (motion.abs().max_element() / MAX_STEP).ceil().max(1.0);
    let part = motion / steps;
    let mut boxes = Vec::new();
    for _ in 0..steps as usize {
        // An axis that's been stopped stays stopped for the rest of the
        // move, as its velocity will be.
        let part = DVec3::select(moved.blocked, DVec3::ZERO, part);
        if part == DVec3::ZERO {
            break;
        }
        let at = aabb.offset(moved.offset);
        let step = step_once(
            at,
            part,
            step_height,
            grounded || moved.landed,
            shapes,
            &mut boxes,
        );
        moved.offset += step.offset;
        moved.blocked |= step.blocked;
        moved.landed |= step.landed;
    }
    moved
}

/// One step of [`move_box`], with `motion` short enough not to need
/// splitting.
fn step_once(
    aabb: Aabb,
    motion: DVec3,
    step_height: f64,
    grounded: bool,
    shapes: &impl Shapes,
    boxes: &mut Vec<Aabb>,
) -> Moved {
    // Everything any way of making this move could touch, stepping up
    // included.
    boxes.clear();
    let climb = DVec3::new(motion.x, step_height, motion.z);
    any_box_in(aabb.stretch(motion).stretch(climb), shapes, |b| {
        boxes.push(b);
        false
    });

    // Out of anything it's a hair inside, so it doesn't catch on the edges
    // of the blocks next to it.
    let push = push_out(aabb, boxes);
    let aabb = aabb.offset(push);
    let mut moved = climb_or_slide(aabb, motion, step_height, grounded, boxes);
    moved.offset += push;
    moved
}

/// How far to move `aabb` to get it out of each box it's less than
/// [`MAX_PUSH`] inside, the shortest way.
fn push_out(aabb: Aabb, boxes: &[Aabb]) -> DVec3 {
    let mut push = DVec3::ZERO;
    for b in boxes {
        let at = aabb.offset(push);
        if !at.intersects(b) {
            continue;
        }
        let axis = at.shallowest_overlap(b);
        let (up, down) = (b.max[axis] - at.min[axis], at.max[axis] - b.min[axis]);
        let out = if up < down { up } else { -down };
        if out.abs() <= MAX_PUSH {
            push[axis] += out;
        }
    }
    push
}

/// [`step_once`] once it's out of anything it was a hair inside: moves,
/// and climbs what it runs into if it can.
fn climb_or_slide(
    aabb: Aabb,
    motion: DVec3,
    step_height: f64,
    grounded: bool,
    boxes: &[Aabb],
) -> Moved {
    let (offset, blocked) = sweep(aabb, motion, boxes);
    let landed = motion.y < 0.0 && blocked.y;
    let flat = |v: DVec3| v.x * v.x + v.z * v.z;
    // Not while going up: a jump into a slab's edge stays a jump.
    let can_step = step_height > 0.0 && motion.y <= 0.0 && (grounded || landed);
    if can_step && (blocked.x || blocked.z) {
        // Step from where it stands: where it was, on the ground already,
        // or where it just landed. Go up first, then across, then back down
        // onto whatever is there (and on down as far as it was falling).
        let (base, fall) = if grounded {
            (DVec3::ZERO, motion.y)
        } else {
            (DVec3::Y * offset.y, 0.0)
        };
        let start = aabb.offset(base);
        let (up, _) = sweep(start, DVec3::Y * step_height, boxes);
        let raised = start.offset(up);
        let (across, stopped) = sweep(raised, DVec3::new(motion.x, 0.0, motion.z), boxes);
        let (down, rests) = sweep(raised.offset(across), DVec3::Y * (fall - up.y), boxes);
        let stepped = base + up + across + down;
        if flat(stepped) > flat(offset) + EPSILON {
            let landed = rests.y || !grounded;
            return Moved {
                offset: stepped,
                blocked: BVec3::new(stopped.x, landed && motion.y < 0.0, stopped.z),
                landed,
            };
        }
    }
    Moved {
        offset,
        blocked,
        landed,
    }
}

/// How far `aabb` gets moving by `motion` through `boxes`, and on which
/// axes a box stopped it: up or down first, then the horizontal axis with
/// more motion, then the other.
fn sweep(aabb: Aabb, motion: DVec3, boxes: &[Aabb]) -> (DVec3, BVec3) {
    let order = if motion.x.abs() < motion.z.abs() {
        [1, 2, 0]
    } else {
        [1, 0, 2]
    };
    let mut at = aabb;
    let mut offset = DVec3::ZERO;
    let mut blocked = BVec3::FALSE;
    for axis in order {
        let mut distance = motion[axis];
        if distance == 0.0 {
            continue;
        }
        for b in boxes {
            distance = at.clip(b, axis, distance);
        }
        if distance != motion[axis] {
            blocked.set(axis, true);
            // Close enough to touching is touching.
            if distance.abs() < EPSILON {
                distance = 0.0;
            }
        }
        offset[axis] = distance;
        at.min[axis] += distance;
        at.max[axis] += distance;
    }
    (offset, blocked)
}

/// Cuts a sneaking body's horizontal `motion` short so it doesn't walk off
/// an edge: wherever it would end up must still have something under it
/// within its step height, as in Minecraft. Only for a body on the ground.
pub fn guard_edges(aabb: Aabb, motion: DVec3, step_height: f64, shapes: &impl Shapes) -> DVec3 {
    /// How much to back off each time, as Minecraft does.
    const BACK_OFF: f64 = 0.05;
    let supported = |x: f64, z: f64| collides(aabb.offset(DVec3::new(x, -step_height, z)), shapes);
    let shrink = |v: f64| {
        if v.abs() < BACK_OFF {
            0.0
        } else {
            v - BACK_OFF * v.signum()
        }
    };
    let (mut x, mut z) = (motion.x, motion.z);
    while x != 0.0 && !supported(x, 0.0) {
        x = shrink(x);
    }
    while z != 0.0 && !supported(0.0, z) {
        z = shrink(z);
    }
    while x != 0.0 && z != 0.0 && !supported(x, z) {
        x = shrink(x);
        z = shrink(z);
    }
    DVec3::new(x, motion.y, z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::platform::collections::HashMap;

    /// Blocks in memory: listed ones have their shape, the rest are air,
    /// and nothing is unloaded unless listed as `None`.
    #[derive(Default)]
    struct TestShapes(HashMap<IVec3, Option<Vec<BlockBox>>>);

    impl Shapes for TestShapes {
        fn shape(&self, block: IVec3) -> Option<Cow<'_, [BlockBox]>> {
            match self.0.get(&block) {
                Some(Some(boxes)) => Some(Cow::Borrowed(boxes)),
                Some(None) => None,
                None => Some(Cow::Borrowed(&[])),
            }
        }
    }

    const SLAB: BlockBox = BlockBox {
        min: Vec3::ZERO,
        max: Vec3::new(1.0, 0.5, 1.0),
    };

    impl TestShapes {
        fn with(mut self, block: IVec3, shape: BlockBox) -> Self {
            self.0.insert(block, Some(vec![shape]));
            self
        }

        fn solid(self, block: IVec3) -> Self {
            self.with(block, BlockBox::FULL)
        }

        /// A floor of full blocks at `y`, from `-r` to `r` in x and z
        /// around `(cx, cz)`.
        fn floor(mut self, cx: i32, y: i32, cz: i32, r: i32) -> Self {
            for x in cx - r..=cx + r {
                for z in cz - r..=cz + r {
                    self = self.solid(IVec3::new(x, y, z));
                }
            }
            self
        }
    }

    /// The player's size and step height.
    const SIZE: DVec3 = DVec3::new(0.6, 1.8, 0.6);
    const STEP_UP: f64 = 0.6;

    fn player_at(feet: DVec3) -> Aabb {
        Aabb::standing(feet, SIZE)
    }

    #[test]
    fn falling_stops_on_the_ground_and_lands() {
        let world = TestShapes::default().floor(0, 0, 0, 2);
        let moved = move_box(
            player_at(DVec3::new(0.5, 3.0, 0.5)),
            DVec3::NEG_Y * 5.0,
            STEP_UP,
            false,
            &world,
        );
        assert_eq!(moved.offset, DVec3::NEG_Y * 2.0);
        assert!(moved.blocked.y && moved.landed);
    }

    /// Running into a wall at an angle keeps the motion along it.
    #[test]
    fn walls_stop_one_axis_and_the_other_slides() {
        let mut world = TestShapes::default();
        for z in -3..=3 {
            for y in 0..=2 {
                world = world.solid(IVec3::new(2, y, z));
            }
        }
        let start = player_at(DVec3::new(1.5, 0.0, 0.5));
        let moved = move_box(start, DVec3::new(1.0, 0.0, 0.8), 0.0, false, &world);
        // Up to the wall at x = 2: the body's side is at 1.8, so 0.2.
        assert!((moved.offset.x - 0.2).abs() < 1e-12, "{moved:?}");
        assert_eq!(moved.offset.z, 0.8);
        assert!(moved.blocked.x && !moved.blocked.z && !moved.landed);
    }

    /// Standing on the ground, moving sideways isn't stopped by the ground,
    /// even with the feet a hair inside it from rounding.
    #[test]
    fn the_ground_doesnt_catch_on_sliding() {
        let world = TestShapes::default().floor(0, -1, 0, 4);
        for feet_y in [0.0, -1e-9, 1e-9] {
            let start = player_at(DVec3::new(0.5, feet_y, 0.5));
            let moved = move_box(start, DVec3::new(2.0, 0.0, -1.5), 0.0, true, &world);
            assert_eq!(moved.offset, DVec3::new(2.0, 0.0, -1.5), "feet at {feet_y}");
        }
    }

    /// However fast it goes, a body doesn't pass through a block: one
    /// block thick, a thousand blocks a step.
    #[test]
    fn nothing_tunnels_at_any_speed() {
        let world = TestShapes::default().solid(IVec3::new(0, 0, 10));
        for speed in [1.0, 13.0, 100.0, 1000.0] {
            let moved = move_box(
                player_at(DVec3::new(0.5, 0.0, 0.5)),
                DVec3::Z * speed,
                0.0,
                false,
                &world,
            );
            // The block's near face is at z = 10; the body's front starts at 0.8.
            let expected = (speed.min(MAX_MOVE)).min(9.2);
            assert!(
                (moved.offset.z - expected).abs() < 1e-9,
                "{speed}: {moved:?}"
            );
        }
    }

    /// A fast diagonal move is split into steps, so it catches on a pillar
    /// in its path that doing all of one axis first would have gone round.
    #[test]
    fn fast_diagonals_follow_the_diagonal() {
        let world = TestShapes::default().solid(IVec3::new(5, 0, 5));
        let moved = move_box(
            player_at(DVec3::new(0.5, 0.0, 0.5)),
            DVec3::new(10.0, 0.0, 10.0),
            0.0,
            false,
            &world,
        );
        let end = DVec3::new(0.5, 0.0, 0.5) + moved.offset;
        assert!(moved.blocked.x || moved.blocked.z, "{moved:?}");
        assert!(end.x < 6.0 || end.z < 6.0, "went past the pillar: {end}");
    }

    /// Far from the origin, where `f32` can't even tell blocks apart,
    /// standing, sliding and stopping at walls work the same.
    #[test]
    fn far_from_the_origin_works_the_same() {
        for far in [0, 1 << 20, 30_000_000, -30_000_000, 2_000_000_000] {
            let o = IVec3::new(far, 0, -far);
            let world = TestShapes::default()
                .floor(o.x, -1, o.z, 3)
                .solid(o + IVec3::new(2, 0, 0));
            let feet = o.as_dvec3() + DVec3::new(0.5, 0.0, 0.5);
            let moved = move_box(
                player_at(feet),
                DVec3::new(3.0, -0.1, 0.25),
                0.0,
                true,
                &world,
            );
            assert!((moved.offset.x - 1.2).abs() < 1e-6, "at {far}: {moved:?}");
            assert_eq!(moved.offset.y, 0.0, "at {far}");
            assert!((moved.offset.z - 0.25).abs() < 1e-6, "at {far}");
            assert!(moved.blocked.x && moved.landed, "at {far}");
        }
    }

    #[test]
    fn bodies_step_up_onto_slabs_but_not_blocks() {
        let world = TestShapes::default()
            .floor(0, -1, 0, 4)
            .with(IVec3::new(1, 0, 0), SLAB)
            .solid(IVec3::new(1, 0, 2));
        let start = |z: f64| player_at(DVec3::new(0.5, 0.0, z));
        // Onto the slab: half a block up, and on along x.
        let moved = move_box(start(0.5), DVec3::X * 0.5, STEP_UP, true, &world);
        assert!((moved.offset.y - 0.5).abs() < 1e-9, "{moved:?}");
        assert!((moved.offset.x - 0.5).abs() < 1e-9, "{moved:?}");
        assert!(moved.landed && !moved.blocked.x);
        // A full block is too tall.
        let moved = move_box(start(2.5), DVec3::X * 0.5, STEP_UP, true, &world);
        assert_eq!(moved.offset.y, 0.0);
        assert!(moved.blocked.x);
        // In the air, it doesn't step.
        let moved = move_box(start(0.5), DVec3::X * 0.5, STEP_UP, false, &world);
        assert!(moved.blocked.x && moved.offset.y == 0.0, "{moved:?}");
    }

    /// Landing beside a block while moving into it climbs no higher than
    /// the step height above where it lands, not above where it was.
    #[test]
    fn landing_beside_a_block_doesnt_climb_it() {
        let world = TestShapes::default()
            .floor(0, -1, 0, 3)
            .solid(IVec3::new(1, 0, 0));
        for feet in [0.4, 0.45, 0.1] {
            let moved = move_box(
                player_at(DVec3::new(0.5, feet, 0.5)),
                DVec3::new(0.3, -0.45, 0.0),
                STEP_UP,
                false,
                &world,
            );
            // Down to the floor and stopped at the block. (From 0.45 it
            // reaches the floor without being stopped, so it hasn't landed
            // yet.)
            assert!(
                (moved.offset.y + feet).abs() < 1e-9,
                "from {feet}: {moved:?}"
            );
            assert!(moved.blocked.x, "from {feet}: {moved:?}");
        }
        // But landing beside a slab steps onto it.
        let world = TestShapes::default()
            .floor(0, -1, 0, 3)
            .with(IVec3::new(1, 0, 0), SLAB);
        let moved = move_box(
            player_at(DVec3::new(0.5, 0.4, 0.5)),
            DVec3::new(0.3, -0.45, 0.0),
            STEP_UP,
            false,
            &world,
        );
        assert!((moved.offset.y - 0.1).abs() < 1e-9, "{moved:?}");
        assert!(!moved.blocked.x && moved.landed, "{moved:?}");
    }

    /// Moving up into a ceiling stops there, and isn't landing.
    #[test]
    fn ceilings_stop_rising_bodies() {
        let world = TestShapes::default().solid(IVec3::new(0, 2, 0));
        let moved = move_box(
            player_at(DVec3::new(0.5, 0.0, 0.5)),
            DVec3::Y * 0.5,
            STEP_UP,
            true,
            &world,
        );
        assert!((moved.offset.y - 0.2).abs() < 1e-9, "{moved:?}");
        assert!(moved.blocked.y && !moved.landed);
    }

    /// Jumping into a slab's edge stays a jump: no stepping while going up.
    #[test]
    fn rising_bodies_dont_step_up() {
        let world = TestShapes::default()
            .floor(0, -1, 0, 3)
            .with(IVec3::new(1, 0, 0), SLAB);
        let moved = move_box(
            player_at(DVec3::new(0.5, 0.0, 0.5)),
            DVec3::new(0.3, 0.2, 0.0),
            STEP_UP,
            true,
            &world,
        );
        assert_eq!(moved.offset.y, 0.2);
        assert!(moved.blocked.x, "{moved:?}");
    }

    /// The horizontal axis with more motion goes first: past a block on
    /// the diagonal, that decides which way the body slides.
    #[test]
    fn the_larger_horizontal_axis_moves_first() {
        let world = TestShapes::default().solid(IVec3::new(1, 0, 1));
        let start = player_at(DVec3::new(0.5, 0.0, 0.5));
        // Mostly along z: all of z, then x stops at the block's side.
        let moved = move_box(start, DVec3::new(0.3, 0.0, 0.45), 0.0, false, &world);
        assert!((moved.offset.x - 0.2).abs() < 1e-9, "{moved:?}");
        assert_eq!(moved.offset.z, 0.45);
        // Mostly along x: the other way round.
        let moved = move_box(start, DVec3::new(0.45, 0.0, 0.3), 0.0, false, &world);
        assert_eq!(moved.offset.x, 0.45);
        assert!((moved.offset.z - 0.2).abs() < 1e-9, "{moved:?}");
    }

    /// The unloaded world is solid: nothing falls into it.
    #[test]
    fn unloaded_blocks_are_solid() {
        let mut world = TestShapes::default();
        world.0.insert(IVec3::new(0, -1, 0), None);
        let moved = move_box(
            player_at(DVec3::new(0.5, 0.5, 0.5)),
            DVec3::NEG_Y,
            0.0,
            false,
            &world,
        );
        assert_eq!(moved.offset.y, -0.5);
    }

    /// A body put a hair into the floor (here by rounding through `f32`,
    /// as the headless `camera` command does) lands on it rather than
    /// sinking through, and still moves along it and up out of it.
    #[test]
    fn a_body_slightly_in_the_floor_doesnt_sink() {
        let world = TestShapes::default().floor(0, -1, 0, 3);
        let feet = DVec3::new(0.5, -1.07e-6, 0.5);
        let moved = move_box(player_at(feet), DVec3::NEG_Y * 0.3, 0.0, false, &world);
        // Pushed back up onto it, and landed.
        assert!((moved.offset.y - 1.07e-6).abs() < 1e-12, "{moved:?}");
        assert!(moved.landed);
        // Sliding across the floor doesn't catch on the next block's edge.
        let moved = move_box(
            player_at(feet),
            DVec3::new(1.0, 0.0, 0.5),
            0.0,
            true,
            &world,
        );
        assert!(moved.blocked == BVec3::FALSE, "{moved:?}");
        assert!((moved.offset.x - 1.0).abs() < 1e-12);
        let moved = move_box(player_at(feet), DVec3::Y * 0.5, 0.0, true, &world);
        assert!((moved.offset.y - 0.5 - 1.07e-6).abs() < 1e-12);
    }

    /// Deep inside two blocks side by side, a body can move out either way
    /// along them.
    #[test]
    fn a_body_inside_two_blocks_can_move_out_either_way() {
        let world = TestShapes::default()
            .solid(IVec3::new(0, 0, 0))
            .solid(IVec3::new(1, 0, 0));
        let start = player_at(DVec3::new(0.9, 0.0, 0.5));
        for x in [2.0, -2.0] {
            let moved = move_box(start, DVec3::X * x, 0.0, false, &world);
            assert_eq!(moved.offset.x, x);
        }
    }

    /// Inside a block, a body can still move out of it.
    #[test]
    fn a_body_inside_a_block_can_move_out() {
        let world = TestShapes::default().solid(IVec3::ZERO);
        let moved = move_box(
            player_at(DVec3::new(0.5, 0.2, 0.5)),
            DVec3::X * 2.0,
            0.0,
            false,
            &world,
        );
        assert_eq!(moved.offset.x, 2.0);
    }

    /// Sneaking stops at an edge with the body still over it, and steps
    /// down no more than the step height.
    #[test]
    fn sneaking_stops_at_edges() {
        // A platform from x = -3 to 0 at y = -1 (top at 0).
        let mut world = TestShapes::default();
        for x in -3..=0 {
            for z in -2..=2 {
                world = world.solid(IVec3::new(x, -1, z));
            }
        }
        let aabb = player_at(DVec3::new(0.5, 0.0, 0.5));
        let motion = guard_edges(aabb, DVec3::new(1.0, -0.1, 0.0), STEP_UP, &world);
        // The body's back edge (at x - 0.3) stays over the platform, which
        // ends at 1.
        assert!(motion.x >= 0.0 && 0.5 + motion.x - 0.3 < 1.0, "{motion}");
        assert_eq!(motion.y, -0.1);
        // Along the edge, it's free.
        let motion = guard_edges(aabb, DVec3::new(0.0, 0.0, 1.0), STEP_UP, &world);
        assert_eq!(motion.z, 1.0);
        // At an outside corner, where each axis alone would stay over the
        // platform but both together wouldn't, it still stops.
        let corner = TestShapes::default()
            .solid(IVec3::new(0, -1, 0))
            .solid(IVec3::new(1, -1, 0))
            .solid(IVec3::new(0, -1, 1));
        let motion = guard_edges(aabb, DVec3::new(1.0, 0.0, 1.0), STEP_UP, &corner);
        let moved = aabb.offset(DVec3::new(motion.x, -STEP_UP, motion.z));
        assert!(collides(moved, &corner), "{motion}");
        assert!(motion.x > 0.0 && motion.z > 0.0, "{motion}");
        // A slab's drop is within the step height, so it's allowed.
        let world = world.with(IVec3::new(1, -1, 0), SLAB);
        let motion = guard_edges(aabb, DVec3::new(0.5, 0.0, 0.0), STEP_UP, &world);
        assert_eq!(motion.x, 0.5);
    }

    /// A tiny motion with nothing in the way isn't stopped, so a flying
    /// body drifting down a hair doesn't count as landing.
    #[test]
    fn tiny_motions_in_the_open_arent_blocked() {
        let world = TestShapes::default().floor(0, -5, 0, 2);
        let moved = move_box(
            player_at(DVec3::new(0.5, 0.0, 0.5)),
            DVec3::new(0.3, -1e-9, 0.0),
            STEP_UP,
            false,
            &world,
        );
        assert_eq!(moved.offset, DVec3::new(0.3, -1e-9, 0.0));
        assert!(!moved.landed && moved.blocked == BVec3::FALSE);
    }

    #[test]
    fn non_finite_motion_doesnt_move() {
        let world = TestShapes::default();
        let moved = move_box(
            player_at(DVec3::ZERO),
            DVec3::new(f64::NAN, 1.0, 0.0),
            0.0,
            false,
            &world,
        );
        assert_eq!(moved.offset, DVec3::ZERO);
    }
}
