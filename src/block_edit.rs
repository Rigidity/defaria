//! The crosshair, its target, and placing blocks. Breaking them is in
//! `mining`.
//!
//! - While the cursor is locked, holding left click breaks the block under
//!   the crosshair (see `mining`) and right click places the active block
//!   against the face you're looking at, into air or a fluid. The crosshair
//!   passes through fluids. A property the block sets on
//!   placement (a log's axis) comes from that face or your view. The click
//!   that locks the cursor doesn't edit anything. The active block is the
//!   active hotbar slot's (see `hotbar`); an item that places no block
//!   places nothing.
//! - A crosshair marks the middle of the screen (hidden while a screen is
//!   open), and an outline shows the targeted block.
//! - Nothing is edited while a screen (the inventory) is open.
//! - Right click on a block that does something when used (a machine
//!   opens its screen) uses it instead of placing against it. Features
//!   handle that in [`BlockUseSystems`].
//!
//! Edits go through [`Voxels`], and the terrain remeshes the changed chunks
//! on the same frame.

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use crate::{
    content::{BlockBox, BlockState, Content, Direction},
    fly_camera::{FlyCamera, FlyCameraSystems},
    game_mode::{GameMode, use_up},
    hotbar::{ActiveSlot, Hotbar, HotbarInput},
    inventory::Inventory,
    physics::{Aabb, Body},
    player::LocalPlayer,
    terrain::{RemeshChunks, Voxels},
    ui::{UiScreen, playing},
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
/// machine's screen.
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

/// What the crosshair picks in a block: its collision boxes, or the whole
/// block for one you walk through. Nothing for air or a fluid, which the
/// crosshair passes through.
pub fn target_shape(content: &Content, state: BlockState) -> &[BlockBox] {
    if state.is_air() || content.state(state).is_fluid() {
        return &[];
    }
    let boxes = &content.state(state).collision;
    if boxes.is_empty() {
        std::slice::from_ref(&BlockBox::FULL)
    } else {
        boxes
    }
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
    camera: Single<&Transform, With<FlyCamera>>,
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
    let Some((pos, face)) = hit.place_at() else {
        return;
    };
    // Properties such as a log's axis follow the face clicked or the view.
    let block = content.placed(block, face, *camera.forward());
    // Not into the camera, outside the world, over another block (but
    // over air or a fluid), or where it would overlap someone.
    let overlaps = content.state(block).collision.iter().any(|b| {
        let b = Aabb::of_block(pos, b);
        bodies.iter().any(|body| body.aabb().intersects(&b))
    });
    let replaceable = voxels
        .get(pos)
        .is_some_and(|there| there.is_air() || content.state(there).is_fluid());
    if pos == camera.translation.floor().as_ivec3() || !replaceable || overlaps {
        return;
    }
    // In survival, only what you have, and placing uses it up.
    let mode = mode.copied().unwrap_or_default();
    if !use_up(mode, inventory.map(Mut::into_inner), item) {
        return;
    }
    voxels.set(pos, block);
}

/// Outlines the block under the crosshair while the cursor is locked.
fn outline_target(
    mut gizmos: Gizmos,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    camera: Single<&Transform, With<FlyCamera>>,
    content: Res<Content>,
    voxels: Voxels,
) {
    if cursor.grab_mode == CursorGrabMode::None {
        return;
    }
    let Some(hit) = crosshair_target(&camera, &content, &voxels) else {
        return;
    };
    let state = voxels.get(hit.block).unwrap_or(BlockState::AIR);
    for b in target_shape(&content, state) {
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
        content::{base, block, grass, test_mod},
        headless::{Script, settle, test_game},
        hotbar::select_with_keys,
        inventory::player::new_player,
        mining::{Mining, MiningPlugin},
        player::SIZE,
        terrain::{
            ChunkCoord, ChunkMap, Terrain, TerrainBlocks, VoxelChanged, apply_finished_meshes,
            remesh_changed_chunks, testing::finish_meshing,
        },
        voxel::Chunk,
        voxel_material::BlockMaterial,
    };
    use bevy::{math::DVec3, time::TimeUpdateStrategy};
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
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), MiningPlugin))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                50,
            )))
            .init_asset::<Mesh>()
            .insert_resource(BlockMaterial::unloaded(Handle::default()))
            .insert_resource(content)
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ChunkMap>()
            .add_message::<VoxelChanged>()
            .add_message::<crate::terrain::PropertiesUpdated>()
            .add_message::<crate::terrain::FluidChanged>()
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
            let entity = app
                .world_mut()
                .spawn((ChunkCoord(coord), Chunk::default()))
                .id();
            app.world_mut()
                .resource_mut::<ChunkMap>()
                .insert(ChunkCoord(coord), entity);
            chunks[i] = entity;
        }
        // Looking along +X at block (14, 1, 5) from three blocks away.
        app.world_mut().spawn((
            FlyCamera::default(),
            Transform::from_xyz(11.5, 1.5, 5.5).looking_to(Vec3::X, Vec3::Y),
            LocalPlayer,
            Inventory::new(),
            hotbar,
            ActiveSlot::default(),
        ));
        (app, chunks[0], chunks[1])
    }

    fn voxel(app: &App, chunk: Entity, x: usize, y: usize, z: usize) -> BlockState {
        app.world().get::<Chunk>(chunk).unwrap().get(x, y, z)
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
        let mut camera = app
            .world_mut()
            .query_filtered::<&mut Transform, With<FlyCamera>>();
        *camera.single_mut(app.world_mut()).unwrap() =
            Transform::from_translation(from).looking_to(dir, Vec3::Y);
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

    /// The crosshair passes through fluids to the block behind, and placing
    /// against it replaces the fluid in the way; mining the block behind
    /// leaves the fluid alone.
    #[test]
    fn fluids_are_passed_through_and_replaced() {
        let (mut app, left, _) = app();
        let mut chunk = app.world_mut().get_mut::<Chunk>(left).unwrap();
        for x in 12..14 {
            chunk.set(x, 1, 5, block("water"));
        }
        chunk.set(13, 2, 5, block("lava"));
        app.update();
        assert_eq!(target_shape(base(), block("water")), []);
        assert_eq!(target_shape(base(), block("lava")), []);
        // The dirt behind the water is the target: placing goes into the
        // water against it.
        press_key(&mut app, KeyCode::Digit1);
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), block("stone"));
        assert_eq!(voxel(&app, left, 12, 1, 5), block("water"));
        mine_target(&mut app);
        assert_eq!(voxel(&app, left, 13, 1, 5), BlockState::AIR);
        assert_eq!(voxel(&app, left, 12, 1, 5), block("water"));
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
            *app.world_mut()
                .query_filtered::<&mut Transform, With<FlyCamera>>()
                .single_mut(app.world_mut())
                .unwrap() = Transform::from_translation(from).looking_to(dir, up);
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
