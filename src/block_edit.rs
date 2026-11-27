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
//! - A crosshair marks the middle of the screen (hidden while a screen is
//!   open), and an outline shows the targeted block.
//! - Nothing is edited while a screen (the inventory) is open.
//! - Right click on a block that does something when used (a machine
//!   opens its screen, a script's `on_used` runs) uses it instead of
//!   placing against it, and an item that does something when used (the
//!   wrench, a script's `on_use`) is used instead of placing. Features
//!   handle that in [`BlockUseSystems`].
//!
//! Edits go through [`Voxels`], and the terrain remeshes the changed chunks
//! on the same frame.

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use crate::{
    content::{BlockBox, BlockState, Content, Direction, ItemState},
    fly_camera::FlyCameraSystems,
    game_mode::{GameMode, harvest, use_up},
    hotbar::{ActiveSlot, Hotbar, HotbarInput},
    inventory::Inventory,
    meshing::RemeshChunks,
    physics::{Aabb, Body},
    player::{Eyes, LocalPlayer},
    ui::{UiScreen, playing},
    world::{FLUID_STEPS, VoxelFluid, Voxels, filled, fluid_step, fluid_surface},
};

/// How far away, in blocks, you can break or place.
pub const REACH: f32 = 8.0;

pub struct BlockEditPlugin;

impl Plugin for BlockEditPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_crosshair)
            // After `Update`, so a screen opened or closed there shows on
            // the same frame.
            .add_systems(
                PostUpdate,
                show_crosshair
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            )
            .configure_sets(
                Update,
                // Without a sync point after it, so a screen it opens
                // applies at the end of the frame, as one a key opens does.
                BlockUseSystems
                    .run_if(playing)
                    .after(HotbarInput)
                    .before_ignore_deferred(BlockEditSystems)
                    .before_ignore_deferred(FlyCameraSystems),
            )
            .add_systems(
                Update,
                (
                    // Before the fly camera, so the click that locks the cursor
                    // finds it still unlocked and edits nothing. After the
                    // hotbar, so a slot picked this frame is the one placed.
                    edit_blocks
                        .in_set(BlockEditSystems)
                        .run_if(playing)
                        .after(HotbarInput)
                        .before(FlyCameraSystems)
                        .before(RemeshChunks),
                    // After the camera moves, so the outline matches the
                    // frame being drawn.
                    outline_target
                        .in_set(BlockOutlineSystems)
                        .after(FlyCameraSystems),
                ),
            );
    }
}

/// Where blocks are placed.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockEditSystems;

/// Where right clicks on blocks that do something when used are handled,
/// while playing, before blocks are placed (and before the fly camera, so
/// the click that locks the cursor finds it unlocked). A system here that
/// uses a click takes it with
/// `ButtonInput::<MouseButton>::clear_just_pressed(MouseButton::Right)`,
/// so nothing is placed against the block too. Use
/// [`crosshair_target`] for the block.
///
/// They run in order of precedence, each taking the click from those
/// after it: the wrench (an item's action, in hand) first, then a
/// block's script's `on_used`, then a machine's screen.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockUseSystems;

/// Where the block under the crosshair is outlined. Systems that change
/// blocks run before it, so the outline never surrounds a block removed
/// on the frame being drawn.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockOutlineSystems;

/// A block a ray ran into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub block: IVec3,
    /// The face the ray entered through, pointing back toward the ray's
    /// origin. Zero if the ray started inside the block.
    pub normal: IVec3,
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
            .filter_map(|b| ray_box(local, dir, b))
            .filter(|&(t, _)| t <= reach)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, normal)| Hit { block, normal })
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
pub(crate) fn ray_box(origin: Vec3, dir: Vec3, b: &BlockBox) -> Option<(f32, IVec3)> {
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

fn edit_blocks(
    mouse: Res<ButtonInput<MouseButton>>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    eyes: Single<Eyes, With<LocalPlayer>>,
    player: Option<
        Single<
            (
                &Hotbar,
                &ActiveSlot,
                Option<&mut Inventory>,
                Option<&GameMode>,
            ),
            With<LocalPlayer>,
        >,
    >,
    content: Res<Content>,
    bodies: Query<&Body>,
    mut voxels: Voxels,
) {
    let camera = eyes.transform();
    if cursor.grab_mode == CursorGrabMode::None {
        return;
    }
    if !mouse.just_pressed(MouseButton::Right) {
        return;
    }
    let Some(hit) = crosshair_target(&camera, &content, &voxels) else {
        return;
    };
    let Some(player) = player else {
        return;
    };
    let (hotbar, active, inventory, mode) = player.into_inner();
    let Some(item) = hotbar.item(*active) else {
        return;
    };
    let Some(block) = hotbar.block(*active, &content) else {
        return;
    };
    let Some((pos, face)) = placement(&content, &voxels, hit, block) else {
        return;
    };
    // Properties such as a log's axis follow the face clicked or the view.
    let block = content.placed(block, face, *camera.forward());
    // Not into the camera, outside the world, over another block (but
    // over one placing replaces: air, which may hold fluid, or a plant),
    // or where it would overlap someone.
    let overlaps = content.state(block).collision.iter().any(|b| {
        let b = Aabb::of_block(pos, b);
        bodies.iter().any(|body| body.aabb().intersects(&b))
    });
    let Some(replaced) = voxels.get(pos) else {
        return;
    };
    let replaceable = replaces(&content, replaced, block);
    // Nor where it can't stand (a flower off the grass).
    let supported = !content.needs_support(block)
        || voxels
            .get(pos - IVec3::Y)
            .is_some_and(|below| content.supports(block, below));
    if pos == camera.translation.floor().as_ivec3() || !replaceable || overlaps || !supported {
        return;
    }
    // In survival, only what you have, and placing uses it up. A fluid
    // block is creative only: placing one would make fluid from nothing,
    // so in survival fluid comes only from containers (a bucket).
    let mode = mode.copied().unwrap_or_default();
    if !mode.is_creative() && content.state(block).is_fluid() {
        return;
    }
    let mut inventory = inventory.map(Mut::into_inner);
    if !use_up(mode, inventory.as_deref_mut(), item) {
        return;
    }
    // A plant built over goes into the inventory in survival, if it fits
    // (otherwise it's lost, as when it loses its support).
    if let Some(inventory) = inventory {
        harvest(mode, inventory, &content, replaced);
    }
    voxels.set(pos, block);
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

/// Outlines the block under the crosshair while the cursor is locked: or,
/// with a fluid container in hand, the fluid or block it aims at.
fn outline_target(
    mut gizmos: Gizmos,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    eyes: Single<Eyes, With<LocalPlayer>>,
    player: Option<Single<(&Hotbar, &ActiveSlot), With<LocalPlayer>>>,
    content: Res<Content>,
    voxels: Voxels,
) {
    let camera = eyes.transform();
    if cursor.grab_mode == CursorGrabMode::None {
        return;
    }
    let at_fluid = player.is_some_and(|player| {
        let (hotbar, active) = player.into_inner();
        aims_at_fluid(&content, hotbar.item(*active))
    });
    let hit = if at_fluid {
        fluid_target(&camera, &content, &voxels)
    } else {
        crosshair_target(&camera, &content, &voxels)
    };
    let Some(hit) = hit else {
        return;
    };
    let (state, partial) = voxels
        .get_with_fluid(hit.block)
        .unwrap_or((BlockState::AIR, None));
    let shape = if at_fluid {
        fluid_shape(&content, state, partial)
    } else {
        target_shape(&content, state)
    };
    for b in shape {
        // A little bigger than the box, so it isn't hidden in its faces.
        let center = hit.block.as_vec3() + (b.min + b.max) / 2.0;
        let size = b.max - b.min + Vec3::splat(0.004);
        gizmos.cube(
            Transform::from_translation(center).with_scale(size),
            Color::srgba(0.05, 0.05, 0.05, 0.8),
        );
    }
}

/// The crosshair's root node.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
struct Crosshair;

/// Shows the crosshair only while playing.
fn show_crosshair(
    screens: Query<(), With<UiScreen>>,
    mut crosshairs: Query<&mut Visibility, With<Crosshair>>,
) {
    let wanted = if playing(screens) {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut visibility in &mut crosshairs {
        visibility.set_if_neq(wanted);
    }
}

fn spawn_crosshair(mut commands: Commands) {
    // Two thin bars crossing at the middle of the screen.
    let bar = |width: f32, height: f32| {
        (
            Node {
                position_type: PositionType::Absolute,
                width: px(width),
                height: px(height),
                ..default()
            },
            BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.8)),
        )
    };
    commands.spawn((
        Name::new("Crosshair"),
        Crosshair,
        Node {
            width: percent(100),
            height: percent(100),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        Pickable::IGNORE,
        children![bar(16.0, 2.0), bar(2.0, 16.0)],
    ));
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::{
        content::{FULL_FLUID, base, block, grass, test_mod},
        headless::{Script, settle, test_game},
        hotbar::select_with_keys,
        inventory::player::new_player,
        meshing::{apply_finished_meshes, remesh_changed_chunks, testing::finish_meshing},
        mining::{Mining, MiningPlugin},
        player::Player,
        player::SIZE,
        script::{
            BlockUsed, ItemUsed, ScriptIndex,
            using::{use_scripted_block, use_scripted_item},
        },
        testing::{chunk_app, insert_chunk},
        voxel_material::BlockMaterial,
        world::{Chunk, ChunkCoord},
        worldgen::{Terrain, TerrainBlocks},
    };
    use bevy::math::DVec3;
    use std::time::Duration;

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

    /// Two chunks side by side along X, a camera locked in front of them,
    /// and the edit, mining and remesh systems; no rendering.
    fn app() -> (App, Entity, Entity) {
        let content = base().clone();
        let (_, hotbar) = new_player(&content);
        let (mut app, left, right) = app_with(content, hotbar);
        let mut chunk = app.world_mut().get_mut::<Chunk>(left).unwrap();
        // A wall at x = 14..=15 of the left chunk, on the chunk border.
        for y in 0..4 {
            chunk.set(15, y, 5, block("stone"));
        }
        chunk.set(14, 1, 5, block("soil"));
        let mut chunk = app.world_mut().get_mut::<Chunk>(right).unwrap();
        chunk.set(0, 1, 5, block("stone"));
        // Meshes the new chunks and lets their change ticks pass.
        finish_meshing(&mut app);
        (app, left, right)
    }

    /// The player's hotbar and active slot.
    fn player(app: &mut App) -> (Hotbar, ActiveSlot) {
        let mut players = app
            .world_mut()
            .query_filtered::<(&Hotbar, &ActiveSlot), With<LocalPlayer>>();
        let (hotbar, active) = players.single(app.world()).unwrap();
        (hotbar.clone(), *active)
    }

    fn set_hotbar(app: &mut App, hotbar: Hotbar) {
        let mut players = app
            .world_mut()
            .query_filtered::<&mut Hotbar, With<LocalPlayer>>();
        *players.single_mut(app.world_mut()).unwrap() = hotbar;
    }

    /// [`app`] with `content`, the player's hotbar `hotbar`, and both
    /// chunks empty.
    fn app_with(content: Content, hotbar: Hotbar) -> (App, Entity, Entity) {
        let mut app = chunk_app(content, Duration::from_millis(50));
        app.add_plugins((AssetPlugin::default(), MiningPlugin))
            .init_asset::<Mesh>()
            .insert_resource(BlockMaterial::unloaded(Handle::default()))
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_message::<BlockUsed>()
            .add_message::<ItemUsed>()
            .add_systems(
                Update,
                (
                    (select_with_keys, edit_blocks).chain().before(RemeshChunks),
                    (remesh_changed_chunks, apply_finished_meshes)
                        .chain()
                        .in_set(RemeshChunks),
                ),
            );
        app.world_mut().spawn((
            Window::default(),
            CursorOptions {
                grab_mode: CursorGrabMode::Locked,
                ..default()
            },
            PrimaryWindow,
        ));
        let mut chunks = [Entity::PLACEHOLDER; 2];
        for (i, coord) in [IVec3::ZERO, IVec3::X].into_iter().enumerate() {
            chunks[i] = insert_chunk(&mut app, coord, Chunk::default());
        }
        // Looking along +X at block (14, 1, 5) from three blocks away.
        app.world_mut()
            .spawn((
                crate::player::tests::local_player_at(
                    Transform::from_xyz(11.5, 1.5, 5.5).looking_to(Vec3::X, Vec3::Y),
                ),
                Inventory::new(),
                hotbar,
                ActiveSlot::default(),
            ))
            // Just eyes: a body would be in the way of blocks placed close by.
            .remove::<Body>();
        (app, chunks[0], chunks[1])
    }

    fn voxel(app: &App, chunk: Entity, x: usize, y: usize, z: usize) -> BlockState {
        app.world().get::<Chunk>(chunk).unwrap().get(x, y, z)
    }

    /// How much fluid the voxel holds.
    fn fluid(app: &App, chunk: Entity, x: i32, y: i32, z: i32) -> u16 {
        let chunk = app.world().get::<Chunk>(chunk).unwrap();
        chunk.fluid_amount_at(IVec3::new(x, y, z))
    }

    fn mesh(app: &App, chunk: Entity) -> Option<AssetId<Mesh>> {
        app.world().get::<Mesh3d>(chunk).map(|mesh| mesh.id())
    }

    fn vertices(app: &App, chunk: Entity) -> usize {
        let id = mesh(app, chunk).expect("chunk has a mesh");
        let meshes = app.world().resource::<Assets<Mesh>>();
        meshes.get(id).unwrap().count_vertices()
    }

    fn look_at(app: &mut App, from: Vec3, dir: Vec3) {
        crate::player::tests::put_local_eyes(
            app,
            Transform::from_translation(from).looking_to(dir, Vec3::Y),
        );
    }

    fn click(app: &mut App, button: MouseButton) {
        let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        mouse.release_all();
        mouse.clear();
        mouse.press(button);
        app.update();
        let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        mouse.release(button);
        mouse.clear();
    }

    /// Holds left click until the targeted block breaks.
    fn mine_target(app: &mut App) {
        let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        mouse.release_all();
        mouse.clear();
        mouse.press(MouseButton::Left);
        let mut target = None;
        for _ in 0..200 {
            app.update();
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
            let mut miners = app.world_mut().query::<&Mining>();
            let mining = miners.single(app.world()).unwrap();
            target = mining.target.map(|(pos, _)| pos).or(target);
            if target.is_some_and(|pos| world_voxel(app, pos).is_air()) {
                break;
            }
        }
        assert!(target.is_some(), "nothing to mine");
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        app.update();
    }

    fn press_key(app: &mut App, key: KeyCode) {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(key);
        app.update();
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(key);
        keys.clear();
    }

    #[test]
    fn mining_breaks_the_targeted_block_and_remeshes() {
        let (mut app, left, _) = app();
        let before = mesh(&app, left);
        mine_target(&mut app);
        assert_eq!(voxel(&app, left, 14, 1, 5), BlockState::AIR);
        assert_eq!(voxel(&app, left, 15, 1, 5), block("stone"));
        assert_ne!(mesh(&app, left), before);
        // Again: the next block along is now the target.
        mine_target(&mut app);
        assert_eq!(voxel(&app, left, 15, 1, 5), BlockState::AIR);
    }

    #[test]
    fn right_click_places_the_active_block_against_the_face() {
        let (mut app, left, _) = app();
        press_key(&mut app, KeyCode::Digit6);
        assert_eq!(player(&mut app).1, ActiveSlot(5));
        // Against the west face, so the log runs along X.
        let log = base().block("defaria:log").unwrap();
        let log = log.state(&[("axis", "x")]).unwrap();
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), log);
        // Placing again stacks toward the camera, but never into it.
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 12, 1, 5), log);
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 11, 1, 5), BlockState::AIR);
    }

    /// The crosshair passes through fluids to the block behind, and a
    /// block placed against it into the fluid in the way, leaving no room
    /// for any, deletes it; mining the block behind leaves the fluid alone.
    #[test]
    fn fluids_are_passed_through_and_replaced() {
        let (mut app, left, _) = app();
        let mut chunk = app.world_mut().get_mut::<Chunk>(left).unwrap();
        for x in 12..14 {
            chunk.fill_fluid_at(IVec3::new(x, 1, 5), block("water"));
        }
        chunk.fill_fluid_at(IVec3::new(13, 2, 5), block("lava"));
        app.update();
        assert_eq!(target_shape(base(), block("water")), []);
        assert_eq!(target_shape(base(), block("lava")), []);
        // The dirt behind the water is the target: placing goes into the
        // water against it.
        press_key(&mut app, KeyCode::Digit1);
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), block("stone"));
        assert_eq!(fluid(&app, left, 13, 1, 5), 0, "stone leaves no room");
        assert_eq!(fluid(&app, left, 12, 1, 5), FULL_FLUID);
        mine_target(&mut app);
        assert_eq!(voxel(&app, left, 13, 1, 5), BlockState::AIR);
        assert_eq!(fluid(&app, left, 12, 1, 5), FULL_FLUID);
    }

    /// A slab placed into water keeps the half of the water it leaves room
    /// for, and the crosshair picks the slab, not the water; breaking it
    /// leaves that water behind.
    #[test]
    fn a_slab_placed_in_water_keeps_its_share() {
        let (mut app, left, _) = app();
        let mut chunk = app.world_mut().get_mut::<Chunk>(left).unwrap();
        chunk.fill_fluid_at(IVec3::new(13, 1, 5), block("water"));
        app.update();
        let mut hotbar = player(&mut app).0;
        hotbar.assign(0, base().default_item("defaria:stone_slab"));
        set_hotbar(&mut app, hotbar);
        press_key(&mut app, KeyCode::Digit1);
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), block("stone_slab"));
        assert_eq!(fluid(&app, left, 13, 1, 5), FULL_FLUID / 2);
        mine_target(&mut app);
        assert_eq!(voxel(&app, left, 13, 1, 5), BlockState::AIR);
        assert_eq!(fluid(&app, left, 13, 1, 5), FULL_FLUID / 2, "left behind");
    }

    /// A log placed against each face of a block runs along that face's
    /// axis, whichever way you came at it.
    #[test]
    fn logs_placed_against_each_face_run_along_it() {
        let (mut app, left, _) = app();
        let target = IVec3::new(7, 7, 10);
        app.world_mut()
            .get_mut::<Chunk>(left)
            .unwrap()
            .set(7, 7, 10, block("stone"));
        press_key(&mut app, KeyCode::Digit6);
        let log = base().block("defaria:log").unwrap();
        for face in Direction::ALL {
            let normal = face.normal();
            let axis = face.axis_name();
            // Three blocks out from the face, looking back at it, a little
            // off center so the view isn't straight along the axis.
            let from = (target + normal * 3).as_vec3() + Vec3::splat(0.5);
            let dir = (target.as_vec3() + Vec3::new(0.6, 0.55, 0.45) - from).normalize();
            let up = if normal.y == 0 { Vec3::Y } else { Vec3::Z };
            crate::player::tests::put_local_eyes(
                &mut app,
                Transform::from_translation(from).looking_to(dir, up),
            );
            click(&mut app, MouseButton::Right);
            let pos = (target + normal).as_uvec3();
            let placed = voxel(&app, left, pos.x as usize, pos.y as usize, pos.z as usize);
            assert_eq!(placed, log.state(&[("axis", axis)]).unwrap(), "{face:?}");
        }
    }

    /// Placing through a click tells a face from its opposite, and reads
    /// the view the way the camera looks: a half placed on top of a block
    /// is its bottom half and one placed underneath its top, and a block
    /// facing the way you look faces west when you look mostly west.
    #[test]
    fn placing_reads_the_clicked_face_and_the_view() {
        let content = test_mod(&[
            ("models/m.json5", "{ faces: [] }"),
            ("blocks/base.json5", r#"{ model: "m" }"#),
            (
                "blocks/half.json5",
                r#"{ model: "m", properties: { half: {
                    values: ["bottom", "top"],
                    place: { from: "face", values: { up: "bottom", down: "top" } },
                } } }"#,
            ),
            (
                "blocks/facing.json5",
                r#"{ model: "m", properties: { facing: {
                    values: ["north", "south", "east", "west"], place: "look",
                } } }"#,
            ),
        ]);
        assert!(content.errors.is_empty(), "{:?}", content.errors);
        let half = content.block("test:half").unwrap();
        let facing = content.block("test:facing").unwrap();
        let expect = [
            // From above, looking down onto its top.
            (
                "test:half",
                Vec3::new(8.5, 10.5, 8.5),
                Vec3::new(8.6, 9.0, 8.4),
                IVec3::new(8, 9, 8),
                half.state(&[("half", "bottom")]).unwrap(),
            ),
            // From below, looking up at its underside.
            (
                "test:half",
                Vec3::new(8.5, 6.5, 8.5),
                Vec3::new(8.4, 8.0, 8.6),
                IVec3::new(8, 7, 8),
                half.state(&[("half", "top")]).unwrap(),
            ),
            // Onto its top from the east, looking mostly west and down.
            (
                "test:facing",
                Vec3::new(10.5, 9.6, 8.5),
                Vec3::new(8.5, 9.0, 8.4),
                IVec3::new(8, 9, 8),
                facing.state(&[("facing", "west")]).unwrap(),
            ),
        ];
        for (item, from, at, pos, state) in expect {
            let mut hotbar = Hotbar::default();
            hotbar.assign(0, content.default_item(item));
            let (mut app, entity, _) = app_with(content.clone(), hotbar);
            app.world_mut().get_mut::<Chunk>(entity).unwrap().set(
                8,
                8,
                8,
                content.default_state("test:base").unwrap(),
            );
            look_at(&mut app, from, (at - from).normalize());
            click(&mut app, MouseButton::Right);
            let p = pos.as_uvec3();
            let placed = voxel(&app, entity, p.x as usize, p.y as usize, p.z as usize);
            assert_eq!(placed, state, "{item} from {from}");
        }
    }

    /// A block isn't placed where it would overlap a body, such as the
    /// player standing there, but a slab fits under one floating above it.
    #[test]
    fn blocks_arent_placed_into_bodies() {
        let (mut app, left, _) = app();
        let body = app
            .world_mut()
            .spawn(Body::new(DVec3::new(13.5, 1.0, 5.5), SIZE, 0.6))
            .id();
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), BlockState::AIR);
        // Raised half a block, a slab fits under it, but a block doesn't.
        app.world_mut().get_mut::<Body>(body).unwrap().position.y = 1.5;
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), BlockState::AIR);
        press_key(&mut app, KeyCode::Digit5);
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), block("stone_slab"));
    }

    /// A plant is only placed on a block that supports it: a dead bush
    /// isn't placed on dirt, and in survival isn't used up; a daisy is.
    #[test]
    fn plants_are_placed_only_where_they_stand() {
        let (mut app, left, _) = app();
        let content = base();
        let [bush, daisy] =
            ["defaria:dead_bush", "defaria:daisy"].map(|id| content.default_item(id).unwrap());
        let mut players = app
            .world_mut()
            .query_filtered::<(Entity, &mut Inventory), With<LocalPlayer>>();
        let (owner, mut inventory) = players.single_mut(app.world_mut()).unwrap();
        inventory.add(content, crate::inventory::ItemStack::new(bush, 1));
        inventory.add(content, crate::inventory::ItemStack::new(daisy, 1));
        app.world_mut().entity_mut(owner).insert(GameMode::Survival);
        // Over the dirt at (14, 1, 5), looking down.
        look_at(&mut app, Vec3::new(14.5, 3.5, 5.5), Vec3::NEG_Y);
        let mut hotbar = player(&mut app).0;
        hotbar.assign(0, Some(bush));
        hotbar.assign(1, Some(daisy));
        set_hotbar(&mut app, hotbar);
        press_key(&mut app, KeyCode::Digit1);
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 14, 2, 5), BlockState::AIR);
        let inventory = app.world().get::<Inventory>(owner).unwrap();
        assert_eq!(inventory.count(bush), 1);
        press_key(&mut app, KeyCode::Digit2);
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 14, 2, 5), block("daisy"));
        let inventory = app.world().get::<Inventory>(owner).unwrap();
        assert_eq!(inventory.count(daisy), 0);
    }

    /// The player in survival, with `items` in the inventory and on the
    /// hotbar from its first slot, the first in hand. The player's entity.
    fn survival_with(app: &mut App, items: &[&str]) -> Entity {
        let content = base();
        let mut hotbar = player(app).0;
        let mut players = app
            .world_mut()
            .query_filtered::<(Entity, &mut Inventory), With<LocalPlayer>>();
        let (owner, mut inventory) = players.single_mut(app.world_mut()).unwrap();
        for (slot, id) in items.iter().enumerate() {
            let item = content.default_item(id).unwrap();
            inventory.add(content, crate::inventory::ItemStack::new(item, 1));
            hotbar.assign(slot, Some(item));
        }
        app.world_mut().entity_mut(owner).insert(GameMode::Survival);
        set_hotbar(app, hotbar);
        press_key(app, KeyCode::Digit1);
        owner
    }

    fn count(app: &App, owner: Entity, id: &str) -> u32 {
        let item = base().default_item(id).unwrap();
        app.world().get::<Inventory>(owner).unwrap().count(item)
    }

    /// A tuft in front of the soil at (14, 1, 5), on a soil floor.
    fn meadow(app: &mut App, left: Entity) {
        let mut chunk = app.world_mut().get_mut::<Chunk>(left).unwrap();
        for x in 12..15 {
            chunk.set(x, 0, 5, block("soil"));
        }
        chunk.set(13, 1, 5, block("tuft"));
    }

    /// A block placed against a plant goes into the plant's voxel instead
    /// of in front of it, and in survival the plant's item goes into the
    /// inventory.
    #[test]
    fn a_block_placed_against_a_plant_replaces_it() {
        let (mut app, left, _) = app();
        meadow(&mut app, left);
        let owner = survival_with(&mut app, &["defaria:stone"]);
        // Straight at the tuft's middle.
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), block("stone"));
        assert_eq!(voxel(&app, left, 12, 1, 5), BlockState::AIR);
        assert_eq!(count(&app, owner, "defaria:stone"), 0);
        assert_eq!(count(&app, owner, "defaria:tuft"), 1);
    }

    /// Aimed over a plant's small target, the crosshair picks the block
    /// behind it, and a block placed against that goes where the plant
    /// stands.
    #[test]
    fn a_block_placed_beside_another_replaces_the_plant_there() {
        let (mut app, left, _) = app();
        meadow(&mut app, left);
        look_at(&mut app, Vec3::new(11.5, 1.95, 5.5), Vec3::X);
        let mut voxels =
            bevy::ecs::system::SystemState::<(Res<Content>, Voxels)>::new(app.world_mut());
        let eyes = Transform::from_xyz(11.5, 1.95, 5.5).looking_to(Vec3::X, Vec3::Y);
        let (content, voxels) = voxels.get_mut(app.world_mut()).unwrap();
        let hit = crosshair_target(&eyes, &content, &voxels).unwrap();
        assert_eq!(hit.block, IVec3::new(14, 1, 5), "past the tuft");
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), block("stone"));
        // In creative, building over it gives nothing.
        let mut inventories = app
            .world_mut()
            .query_filtered::<&Inventory, With<LocalPlayer>>();
        let inventory = inventories.single(app.world()).unwrap();
        assert!(inventory.is_empty(), "{inventory:?}");
    }

    /// A block that isn't replaceable stays: one placed against the soil
    /// behind a slab isn't put over the slab, and uses nothing up.
    #[test]
    fn blocks_that_arent_replaceable_arent_placed_over() {
        let (mut app, left, _) = app();
        meadow(&mut app, left);
        let mut chunk = app.world_mut().get_mut::<Chunk>(left).unwrap();
        chunk.set(13, 1, 5, block("stone_slab"));
        let owner = survival_with(&mut app, &["defaria:stone"]);
        // Over the slab, at the soil's face behind it.
        look_at(&mut app, Vec3::new(11.5, 1.95, 5.5), Vec3::X);
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), block("stone_slab"));
        assert_eq!(count(&app, owner, "defaria:stone"), 1, "nothing used up");
    }

    /// A plant placed against the same plant goes beside it, as against
    /// any other block, so a row can be planted; a different plant
    /// replaces it.
    #[test]
    fn the_same_plant_goes_beside_a_plant_and_another_replaces_it() {
        let (mut app, left, _) = app();
        meadow(&mut app, left);
        let owner = survival_with(&mut app, &["defaria:tuft", "defaria:daisy"]);
        // At the tuft's west side, from (11.5, 1.5, 5.5).
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), block("tuft"));
        assert_eq!(voxel(&app, left, 12, 1, 5), block("tuft"));
        assert_eq!(count(&app, owner, "defaria:tuft"), 0, "none harvested");
        // The daisy goes into the nearer tuft's voxel.
        press_key(&mut app, KeyCode::Digit2);
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 12, 1, 5), block("daisy"));
        assert_eq!(voxel(&app, left, 11, 1, 5), BlockState::AIR);
        assert_eq!(count(&app, owner, "defaria:daisy"), 0);
        assert_eq!(count(&app, owner, "defaria:tuft"), 1);
    }

    /// A block placed into a plant takes its placed properties from the
    /// plant's face that was hit: a log against a tuft's side lies along
    /// X; one placed down onto a tuft stands up.
    #[test]
    fn a_block_placed_into_a_plant_faces_the_side_hit() {
        let (mut app, left, _) = app();
        meadow(&mut app, left);
        let log = base().block("defaria:log").unwrap();
        press_key(&mut app, KeyCode::Digit6);
        click(&mut app, MouseButton::Right);
        assert_eq!(
            voxel(&app, left, 13, 1, 5),
            log.state(&[("axis", "x")]).unwrap()
        );
        app.world_mut()
            .get_mut::<Chunk>(left)
            .unwrap()
            .set(12, 1, 5, block("tuft"));
        look_at(&mut app, Vec3::new(12.5, 3.5, 5.5), Vec3::NEG_Y);
        click(&mut app, MouseButton::Right);
        assert_eq!(
            voxel(&app, left, 12, 1, 5),
            log.state(&[("axis", "y")]).unwrap()
        );
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

    /// An item that places no block (a tool, or an empty slot) does
    /// nothing on right click.
    #[test]
    fn right_click_with_an_item_that_places_nothing_does_nothing() {
        let (mut app, left, _) = app();
        let pickaxe = base().default_item("defaria:pickaxe");
        let mut hotbar = player(&mut app).0;
        hotbar.assign(0, pickaxe);
        hotbar.assign(1, None);
        set_hotbar(&mut app, hotbar);
        click(&mut app, MouseButton::Right);
        press_key(&mut app, KeyCode::Digit2);
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), BlockState::AIR);
        assert_eq!(voxel(&app, left, 14, 1, 5), block("soil"));
    }

    /// [`app`] with the base game's script index.
    fn scripted_app() -> (App, Entity, Entity) {
        let (mut app, left, right) = app();
        let ids: Vec<String> = base().mods.iter().map(|m| m.id.clone()).collect();
        let runtime = crate::script::ScriptRuntime::new(
            crate::script::ScriptSources::read(std::path::Path::new("assets/mods"), &ids),
            crate::script::ScriptSettings::untimed(),
        );
        app.insert_resource(ScriptIndex::new(base(), &runtime))
            .add_systems(
                Update,
                (use_scripted_block, use_scripted_item)
                    .chain()
                    .after(select_with_keys)
                    .before(edit_blocks),
            );
        (app, left, right)
    }

    fn drain<M: Message>(app: &mut App) -> Vec<M> {
        app.world_mut()
            .resource_mut::<Messages<M>>()
            .drain()
            .collect()
    }

    /// Right click on a block whose script takes it uses it rather than
    /// placing against it, even with an empty hand; sneaking with a block
    /// in hand places.
    #[test]
    fn right_click_uses_a_scripted_block_instead_of_placing() {
        let (mut app, left, _) = scripted_app();
        let lamp = block("switch_lamp");
        app.world_mut()
            .get_mut::<Chunk>(left)
            .unwrap()
            .set(14, 1, 5, lamp);
        press_key(&mut app, KeyCode::Digit6);
        drain::<BlockUsed>(&mut app);
        click(&mut app, MouseButton::Right);
        let used = drain::<BlockUsed>(&mut app);
        assert_eq!(used.len(), 1);
        assert_eq!(used[0].pos, IVec3::new(14, 1, 5));
        let view = used[0].player.unwrap();
        assert_eq!(
            view.target,
            Some((IVec3::new(14, 1, 5), Some(Direction::West)))
        );
        assert_eq!(voxel(&app, left, 13, 1, 5), BlockState::AIR);

        let mut hotbar = player(&mut app).0;
        let active = player(&mut app).1;
        hotbar.assign(active.0, None);
        set_hotbar(&mut app, hotbar);
        click(&mut app, MouseButton::Right);
        assert_eq!(drain::<BlockUsed>(&mut app).len(), 1);

        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<LocalPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut().entity_mut(entity).insert(Player {
            sneaking: true,
            ..default()
        });
        press_key(&mut app, KeyCode::Digit1);
        click(&mut app, MouseButton::Right);
        assert!(drain::<BlockUsed>(&mut app).is_empty());
        assert!(!voxel(&app, left, 13, 1, 5).is_air());
    }

    /// Right click with an item whose script has `on_use` uses the item.
    #[test]
    fn right_click_uses_a_scripted_item() {
        let (mut app, left, _) = scripted_app();
        let moss = base().default_item("defaria:moss");
        let mut hotbar = player(&mut app).0;
        hotbar.assign(0, moss);
        set_hotbar(&mut app, hotbar);
        press_key(&mut app, KeyCode::Digit1);
        click(&mut app, MouseButton::Right);
        let used = drain::<ItemUsed>(&mut app);
        assert_eq!(used.len(), 1);
        assert_eq!(Some(used[0].item), moss);
        assert_eq!(voxel(&app, left, 13, 1, 5), BlockState::AIR);
    }

    /// The left chunk's block at x = 0 faces -X, where there's no chunk.
    #[test]
    fn right_click_never_places_outside_the_world() {
        let (mut app, left, _) = app();
        app.world_mut()
            .get_mut::<Chunk>(left)
            .unwrap()
            .set(0, 1, 8, block("stone"));
        look_at(&mut app, Vec3::new(-2.5, 1.5, 8.5), Vec3::X);
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 0, 1, 8), block("stone"));
        assert_eq!(voxel(&app, left, 1, 1, 8), BlockState::AIR);
    }

    #[test]
    fn breaking_on_a_chunk_border_remeshes_the_neighbour() {
        let (mut app, left, right) = app();
        // The right chunk's lone block has its -X face against the left
        // chunk's wall, so it starts with 5 faces.
        assert_eq!(vertices(&app, right), 5 * 4);
        mine_target(&mut app);
        // Breaking (15, 1, 5) uncovers that face.
        mine_target(&mut app);
        assert_eq!(voxel(&app, left, 15, 1, 5), BlockState::AIR);
        assert_eq!(vertices(&app, right), 6 * 4);
    }

    /// The edit is at the right chunk's x = 0, so the chunk to its -X
    /// remeshes, and with nothing left in it the right chunk has no mesh.
    #[test]
    fn breaking_the_last_block_of_a_chunk_removes_its_mesh() {
        let (mut app, left, right) = app();
        let before = vertices(&app, left);
        look_at(&mut app, Vec3::new(19.5, 1.5, 5.5), Vec3::NEG_X);
        mine_target(&mut app);
        assert_eq!(voxel(&app, right, 0, 1, 5), BlockState::AIR);
        assert_eq!(mesh(&app, right), None);
        // The wall's block at (15, 1, 5) shows its +X face now.
        assert_eq!(vertices(&app, left), before + 4);
    }

    #[test]
    fn clicks_do_nothing_while_the_cursor_is_free() {
        let (mut app, left, _) = app();
        app.world_mut()
            .query::<&mut CursorOptions>()
            .single_mut(app.world_mut())
            .unwrap()
            .grab_mode = CursorGrabMode::None;
        click(&mut app, MouseButton::Left);
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 14, 1, 5), block("soil"));
        assert_eq!(voxel(&app, left, 13, 1, 5), BlockState::AIR);
    }

    /// The crosshair hides while a screen is open.
    #[test]
    fn the_crosshair_hides_while_a_screen_is_open() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_systems(Startup, spawn_crosshair)
            .add_systems(Update, show_crosshair);
        let shown = |app: &mut App| {
            let mut crosshairs = app
                .world_mut()
                .query_filtered::<&Visibility, With<Crosshair>>();
            *crosshairs.single(app.world()).unwrap() == Visibility::Inherited
        };
        app.update();
        assert!(shown(&mut app));
        let screen = app.world_mut().spawn(UiScreen).id();
        app.update();
        assert!(!shown(&mut app));
        app.world_mut().despawn(screen);
        app.update();
        assert!(shown(&mut app));
    }

    fn world_voxel(app: &mut App, pos: IVec3) -> BlockState {
        let (coord, local) = ChunkCoord::split(pos);
        let mut chunks = app.world_mut().query::<(&ChunkCoord, &Chunk)>();
        chunks
            .iter(app.world())
            .find(|(c, _)| **c == coord)
            .map_or(BlockState::AIR, |(_, chunk)| chunk.at(local))
    }

    /// Frames, at the headless runner's 60 per second, that holding left
    /// click on grass takes to break it.
    fn grass_frames() -> u32 {
        let seconds = crate::mining::break_time(base().state(grass()).hardness);
        (seconds * 60.0).ceil() as u32
    }

    /// Runs `script` (after a `camera` line three blocks above the grass
    /// at (40, 40), looking straight down) through the real game and input
    /// pipeline, and returns the game, the grass's position and what was
    /// under it before.
    fn run_over_grass(script: &str) -> (App, IVec3, BlockState) {
        let (x, z) = (40, 40);
        let height = Terrain::new(default(), TerrainBlocks::from_content(base())).height(x, z);
        let script = Script::parse(&format!(
            "camera {} {} {} 0 -90\n{script}",
            x as f32 + 0.5,
            height as f32 + 3.5,
            z as f32 + 0.5,
        ))
        .unwrap();
        let mut app = test_game(script);
        settle(&mut app);

        let top = IVec3::new(x, height - 1, z);
        let below = top - IVec3::Y;
        assert_eq!(world_voxel(&mut app, top), grass());
        let below_before = world_voxel(&mut app, below);
        assert!(!below_before.is_air());

        let mut frames = 1;
        while app.should_exit().is_none() {
            app.update();
            frames += 1;
            // The script starts once the fog has opened up: a few seconds.
            assert!(frames < 600, "script never finished");
        }
        (app, top, below_before)
    }

    /// Through the real game: the click that locks the cursor mines
    /// nothing, even held for longer than the grass takes to break.
    #[test]
    fn holding_the_locking_click_mines_nothing() {
        let hold = grass_frames() * 2;
        let (mut app, top, _) = run_over_grass(&format!(
            "wait 2\n\
             hold MouseLeft {hold}  # lock the cursor, and keep holding\n\
             wait 2\n"
        ));
        assert_eq!(world_voxel(&mut app, top), grass());
        let mut miners = app.world_mut().query::<&Mining>();
        assert_eq!(miners.single(app.world()).unwrap().target, None);
    }

    /// Through the real game: the click that locks the cursor edits
    /// nothing, holding the next one breaks the block under the crosshair,
    /// and a right click puts the active block (stone) back.
    #[test]
    fn locking_click_edits_nothing_then_clicks_break_and_place() {
        // Long enough to break the grass, but not the block under it too.
        let hold = grass_frames() + 2;
        let (mut app, top, below_before) = run_over_grass(&format!(
            "wait 2\n\
             hold MouseLeft 1  # lock the cursor\n\
             wait 1\n\
             hold MouseLeft {hold} # break\n\
             wait 1\n\
             hold MouseRight 1  # place\n\
             wait 2\n"
        ));
        // Grass broken once, not twice, and stone placed in its place.
        assert_eq!(world_voxel(&mut app, top), block("stone"));
        assert_eq!(world_voxel(&mut app, top - IVec3::Y), below_before);
    }
}
