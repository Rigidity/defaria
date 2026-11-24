//! Breaking and placing blocks.
//!
//! - While the cursor is locked, left click breaks the block under the
//!   crosshair and right click places the active block against the face
//!   you're looking at. The click that locks the cursor doesn't break
//!   anything.
//! - The number keys 1–7 pick the active block from [`HOTBAR`].
//! - A crosshair marks the middle of the screen, an outline shows the
//!   targeted block, and a hotbar along the bottom lists the blocks with the
//!   active one highlighted.
//!
//! Edited chunks, and neighbouring chunks whose faces the edit uncovers or
//! hides, are remeshed in the same frame.

use std::collections::HashMap;

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use crate::{
    fly_camera::{FlyCamera, FlyCameraSystems},
    terrain::ChunkCoord,
    voxel::{CHUNK_SIZE, Chunk, Voxel},
    voxel_material::{BlockMaterial, VoxelMaterial},
};

/// How far away, in blocks, you can break or place.
pub const REACH: f32 = 8.0;

/// The blocks on the number keys, in order: key 1 is the first.
pub const HOTBAR: [Voxel; 7] = [
    Voxel::Stone,
    Voxel::Dirt,
    Voxel::Grass,
    Voxel::Sand,
    Voxel::Gravel,
    Voxel::Log,
    Voxel::Planks,
];

const HOTBAR_KEYS: [KeyCode; HOTBAR.len()] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
];

pub struct BlockEditPlugin;

impl Plugin for BlockEditPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveBlock>()
            .add_systems(Startup, spawn_hud)
            .add_systems(
                Update,
                (
                    (select_block, edit_blocks, remesh_edited_chunks)
                        .chain()
                        .before(FlyCameraSystems),
                    (update_hotbar, outline_target),
                ),
            );
    }
}

/// The block that right click places: an index into [`HOTBAR`].
#[derive(Resource, Reflect, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[reflect(Resource)]
pub struct ActiveBlock(pub usize);

impl ActiveBlock {
    pub fn voxel(self) -> Voxel {
        HOTBAR[self.0]
    }
}

/// A block a ray ran into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub block: IVec3,
    /// The face the ray entered through, pointing back toward the ray's
    /// origin. Zero if the ray started inside the block.
    pub normal: IVec3,
}

impl Hit {
    /// Where a block placed against the hit face goes. `None` if the ray
    /// started inside the block, so there's no face to place against.
    pub fn place_at(self) -> Option<IVec3> {
        (self.normal != IVec3::ZERO).then_some(self.block + self.normal)
    }
}

/// The first block within `reach` along the ray from `origin` in
/// direction `dir` for which `solid` is true. Steps through every block the
/// ray touches, in order (Amanatides & Woo), so it can't skip through
/// corners.
pub fn raycast(origin: Vec3, dir: Vec3, reach: f32, solid: impl Fn(IVec3) -> bool) -> Option<Hit> {
    let dir = dir.normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }
    let mut block = origin.floor().as_ivec3();
    if solid(block) {
        return Some(Hit {
            block,
            normal: IVec3::ZERO,
        });
    }
    let step = IVec3::new(sign(dir.x), sign(dir.y), sign(dir.z));
    // Ray distance to cross one whole block along each axis, and to the
    // next block boundary along each axis.
    let delta = Vec3::from_array(dir.to_array().map(|d| (1.0 / d).abs()));
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
        if solid(block) {
            let mut normal = IVec3::ZERO;
            normal[axis] = -step[axis];
            return Some(Hit { block, normal });
        }
    }
}

fn sign(x: f32) -> i32 {
    if x > 0.0 {
        1
    } else if x < 0.0 {
        -1
    } else {
        0
    }
}

/// The chunk holding world block `pos`, and `pos` within it.
fn split(pos: IVec3) -> (IVec3, IVec3) {
    let size = CHUNK_SIZE as i32;
    (
        pos.div_euclid(IVec3::splat(size)),
        pos.rem_euclid(IVec3::splat(size)),
    )
}

fn local_get(chunk: &Chunk, local: IVec3) -> Voxel {
    chunk.get(local.x as usize, local.y as usize, local.z as usize)
}

fn select_block(keys: Res<ButtonInput<KeyCode>>, mut active: ResMut<ActiveBlock>) {
    if let Some(index) = HOTBAR_KEYS.iter().position(|&key| keys.just_pressed(key)) {
        active.0 = index;
    }
}

/// The ray from the camera through the crosshair.
fn camera_ray(transform: &Transform) -> (Vec3, Vec3) {
    (transform.translation, transform.forward().as_vec3())
}

fn edit_blocks(
    mouse: Res<ButtonInput<MouseButton>>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    camera: Single<&Transform, With<FlyCamera>>,
    active: Res<ActiveBlock>,
    mut chunks: Query<(Entity, &ChunkCoord, &mut Chunk)>,
) {
    // Runs before the fly camera's systems, so on the click that locks
    // the cursor it's still unlocked here and nothing is edited.
    if cursor.grab_mode == CursorGrabMode::None {
        return;
    }
    let (breaking, placing) = (
        mouse.just_pressed(MouseButton::Left),
        mouse.just_pressed(MouseButton::Right),
    );
    if !breaking && !placing {
        return;
    }
    let index: HashMap<IVec3, Entity> = chunks
        .iter()
        .map(|(entity, coord, _)| (coord.0, entity))
        .collect();
    // `None` outside the world.
    let voxel = |pos: IVec3| {
        let (coord, local) = split(pos);
        let entity = *index.get(&coord)?;
        Some(local_get(chunks.get(entity).ok()?.2, local))
    };
    let (origin, dir) = camera_ray(&camera);
    let Some(hit) = raycast(origin, dir, REACH, |pos| {
        voxel(pos).is_some_and(Voxel::is_solid)
    }) else {
        return;
    };
    let (pos, new) = if breaking {
        (hit.block, Voxel::Air)
    } else {
        let Some(pos) = hit.place_at() else {
            return;
        };
        // Not into the camera, outside the world, or over another block.
        if pos == origin.floor().as_ivec3() || voxel(pos) != Some(Voxel::Air) {
            return;
        }
        (pos, active.voxel())
    };

    let (coord, local) = split(pos);
    let mut chunk = chunks.get_mut(index[&coord]).unwrap().2;
    chunk.set(local.x as usize, local.y as usize, local.z as usize, new);
    // A block on a chunk's edge also shows or hides a face of the
    // neighbouring chunk's block; mark that chunk for remeshing too.
    let last = CHUNK_SIZE as i32 - 1;
    for axis in 0..3 {
        let mut offset = IVec3::ZERO;
        if local[axis] == 0 {
            offset[axis] = -1;
        } else if local[axis] == last {
            offset[axis] = 1;
        } else {
            continue;
        }
        if let Some(&neighbour) = index.get(&(coord + offset)) {
            chunks.get_mut(neighbour).unwrap().2.set_changed();
        }
    }
}

/// Rebuilds the mesh of every chunk changed since the last run, culling
/// faces against the neighbouring chunks as they are now. Skips chunks that
/// were just spawned: the terrain meshes those itself.
fn remesh_edited_chunks(
    mut commands: Commands,
    chunks: Query<(Entity, &ChunkCoord, Ref<Chunk>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    material: Res<BlockMaterial>,
) {
    let changed: Vec<(Entity, ChunkCoord)> = chunks
        .iter()
        .filter(|(_, _, chunk)| chunk.is_changed() && !chunk.is_added())
        .map(|(entity, coord, _)| (entity, *coord))
        .collect();
    if changed.is_empty() {
        return;
    }
    let all: HashMap<IVec3, &Chunk> = chunks
        .iter()
        .map(|(_, coord, chunk)| (coord.0, chunk.into_inner()))
        .collect();
    for (entity, coord) in changed {
        let origin = coord.origin();
        let mesh = all[&coord.0].mesh(|local| {
            let (coord, local) = split(origin + local);
            all.get(&coord)
                .map_or(Voxel::Air, |chunk| local_get(chunk, local))
        });
        // Like the terrain: no mesh at all rather than an empty one.
        if mesh.count_vertices() > 0 {
            commands.entity(entity).insert((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material.material.clone()),
            ));
        } else {
            commands
                .entity(entity)
                .remove::<(Mesh3d, MeshMaterial3d<VoxelMaterial>)>();
        }
    }
}

/// Outlines the block under the crosshair while the cursor is locked.
fn outline_target(
    mut gizmos: Gizmos,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    camera: Single<&Transform, With<FlyCamera>>,
    chunks: Query<(&ChunkCoord, &Chunk)>,
) {
    if cursor.grab_mode == CursorGrabMode::None {
        return;
    }
    let index: HashMap<IVec3, &Chunk> = chunks
        .iter()
        .map(|(coord, chunk)| (coord.0, chunk))
        .collect();
    let (origin, dir) = camera_ray(&camera);
    let hit = raycast(origin, dir, REACH, |pos| {
        let (coord, local) = split(pos);
        index
            .get(&coord)
            .is_some_and(|chunk| local_get(chunk, local).is_solid())
    });
    if let Some(hit) = hit {
        let center = hit.block.as_vec3() + Vec3::splat(0.5);
        gizmos.cube(
            Transform::from_translation(center).with_scale(Vec3::splat(1.004)),
            Color::srgba(0.05, 0.05, 0.05, 0.8),
        );
    }
}

/// One slot of the hotbar, showing [`HOTBAR`]`[index]`.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
struct HotbarSlot(usize);

const SLOT_COLOR: Color = Color::srgba(0.0, 0.0, 0.0, 0.45);
const ACTIVE_SLOT_COLOR: Color = Color::srgba(0.95, 0.85, 0.35, 0.85);
const SLOT_TEXT: Color = Color::srgb(0.9, 0.9, 0.9);
const ACTIVE_SLOT_TEXT: Color = Color::srgb(0.1, 0.1, 0.1);

fn spawn_hud(mut commands: Commands) {
    // Crosshair: two thin bars crossing at the middle of the screen.
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

    commands
        .spawn((
            Name::new("Hotbar"),
            Node {
                position_type: PositionType::Absolute,
                bottom: px(16),
                width: percent(100),
                justify_content: JustifyContent::Center,
                column_gap: px(6),
                ..default()
            },
        ))
        .with_children(|hotbar| {
            for (index, voxel) in HOTBAR.iter().enumerate() {
                hotbar.spawn((
                    HotbarSlot(index),
                    Node {
                        padding: UiRect::axes(px(10), px(6)),
                        ..default()
                    },
                    BackgroundColor(SLOT_COLOR),
                    children![(
                        Text::new(format!("{} {voxel:?}", index + 1)),
                        TextFont::from_font_size(16.0),
                        TextColor(SLOT_TEXT),
                    )],
                ));
            }
        });
}

fn update_hotbar(
    active: Res<ActiveBlock>,
    mut slots: Query<(&HotbarSlot, &mut BackgroundColor, &Children)>,
    mut texts: Query<&mut TextColor>,
) {
    if !active.is_changed() {
        return;
    }
    for (slot, mut background, children) in &mut slots {
        let is_active = slot.0 == active.0;
        background.0 = if is_active {
            ACTIVE_SLOT_COLOR
        } else {
            SLOT_COLOR
        };
        for &child in children {
            if let Ok(mut color) = texts.get_mut(child) {
                color.0 = if is_active {
                    ACTIVE_SLOT_TEXT
                } else {
                    SLOT_TEXT
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_at(blocks: &[IVec3]) -> impl Fn(IVec3) -> bool + '_ {
        move |pos| blocks.contains(&pos)
    }

    #[test]
    fn ray_hits_the_first_block_and_the_face_it_entered() {
        let blocks = [IVec3::new(3, 0, 0), IVec3::new(5, 0, 0)];
        let hit = raycast(Vec3::new(0.5, 0.5, 0.5), Vec3::X, 8.0, solid_at(&blocks));
        assert_eq!(
            hit,
            Some(Hit {
                block: IVec3::new(3, 0, 0),
                normal: IVec3::NEG_X,
            })
        );
        assert_eq!(hit.unwrap().place_at(), Some(IVec3::new(2, 0, 0)));
    }

    #[test]
    fn ray_looking_down_hits_the_top_face() {
        let blocks = [IVec3::new(2, 1, 2)];
        let hit = raycast(
            Vec3::new(2.5, 5.2, 2.5),
            Vec3::NEG_Y,
            8.0,
            solid_at(&blocks),
        )
        .unwrap();
        assert_eq!(hit.normal, IVec3::Y);
        assert_eq!(hit.place_at(), Some(IVec3::new(2, 2, 2)));
    }

    #[test]
    fn ray_stops_at_its_reach() {
        let blocks = [IVec3::new(10, 0, 0)];
        let origin = Vec3::new(0.5, 0.5, 0.5);
        assert_eq!(raycast(origin, Vec3::X, 8.0, solid_at(&blocks)), None);
        assert!(raycast(origin, Vec3::X, 10.0, solid_at(&blocks)).is_some());
    }

    #[test]
    fn ray_works_at_negative_coordinates() {
        let blocks = [IVec3::new(-3, -1, 0)];
        let hit = raycast(
            Vec3::new(-0.5, -0.5, 0.5),
            Vec3::NEG_X,
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
            Vec3::new(0.2, 0.5, 0.7),
            Vec3::new(1.0, 0.0, -1.0),
            3.0,
            |pos| {
                visited.borrow_mut().push(pos);
                false
            },
        );
        let visited = visited.into_inner();
        for pair in visited.windows(2) {
            let step = (pair[1] - pair[0]).abs();
            assert_eq!(step.x + step.y + step.z, 1, "{pair:?}");
        }
        assert!(visited.len() >= 4);
    }

    #[test]
    fn ray_starting_inside_a_block_hits_it_with_no_face() {
        let blocks = [IVec3::ZERO];
        let hit = raycast(Vec3::splat(0.5), Vec3::X, 8.0, solid_at(&blocks)).unwrap();
        assert_eq!(hit.block, IVec3::ZERO);
        assert_eq!(hit.place_at(), None);
    }

    #[test]
    fn split_finds_the_chunk_and_local_position() {
        assert_eq!(
            split(IVec3::new(17, 0, 15)),
            (IVec3::new(1, 0, 0), IVec3::new(1, 0, 15))
        );
        assert_eq!(
            split(IVec3::new(-1, 0, 0)),
            (IVec3::new(-1, 0, 0), IVec3::new(15, 0, 0))
        );
    }

    /// Two chunks side by side along X, a camera locked in front of them,
    /// and the edit systems; no rendering.
    fn app() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .insert_resource(BlockMaterial {
                material: Handle::default(),
                layers: default(),
            })
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ActiveBlock>()
            .add_systems(
                Update,
                (select_block, edit_blocks, remesh_edited_chunks).chain(),
            );
        app.world_mut().spawn((
            Window::default(),
            CursorOptions {
                grab_mode: CursorGrabMode::Locked,
                ..default()
            },
            PrimaryWindow,
        ));
        let mut left = Chunk::default();
        // A wall at x = 14..=15 of the left chunk, on the chunk border.
        for y in 0..4 {
            left.set(15, y, 5, Voxel::Stone);
        }
        left.set(14, 1, 5, Voxel::Dirt);
        let left = app.world_mut().spawn((ChunkCoord(IVec3::ZERO), left)).id();
        let mut right = Chunk::default();
        right.set(0, 1, 5, Voxel::Stone);
        let right = app.world_mut().spawn((ChunkCoord(IVec3::X), right)).id();
        // Looking along +X at block (14, 1, 5) from three blocks away.
        app.world_mut().spawn((
            FlyCamera::default(),
            Transform::from_xyz(11.5, 1.5, 5.5).looking_to(Vec3::X, Vec3::Y),
        ));
        // Let the spawn-time change ticks pass.
        app.update();
        (app, left, right)
    }

    fn voxel(app: &App, chunk: Entity, x: usize, y: usize, z: usize) -> Voxel {
        app.world().get::<Chunk>(chunk).unwrap().get(x, y, z)
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

    fn press_key(app: &mut App, key: KeyCode) {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(key);
        app.update();
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(key);
        keys.clear();
    }

    #[test]
    fn left_click_breaks_the_targeted_block_and_remeshes() {
        let (mut app, left, _) = app();
        assert!(app.world().get::<Mesh3d>(left).is_none());
        click(&mut app, MouseButton::Left);
        assert_eq!(voxel(&app, left, 14, 1, 5), Voxel::Air);
        assert_eq!(voxel(&app, left, 15, 1, 5), Voxel::Stone);
        assert!(app.world().get::<Mesh3d>(left).is_some());
        // Again: the next block along is now the target.
        click(&mut app, MouseButton::Left);
        assert_eq!(voxel(&app, left, 15, 1, 5), Voxel::Air);
    }

    #[test]
    fn right_click_places_the_active_block_against_the_face() {
        let (mut app, left, _) = app();
        press_key(&mut app, KeyCode::Digit6);
        assert_eq!(*app.world().resource::<ActiveBlock>(), ActiveBlock(5));
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), Voxel::Log);
        // Placing again stacks toward the camera, but never into it.
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 12, 1, 5), Voxel::Log);
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 11, 1, 5), Voxel::Air);
    }

    #[test]
    fn breaking_on_a_chunk_border_remeshes_the_neighbour() {
        let (mut app, left, right) = app();
        click(&mut app, MouseButton::Left);
        assert!(app.world().get::<Mesh3d>(right).is_none());
        // Breaking (15, 1, 5) uncovers the -X face of the right chunk's
        // block at (0, 1, 5).
        click(&mut app, MouseButton::Left);
        assert_eq!(voxel(&app, left, 15, 1, 5), Voxel::Air);
        let mesh = app
            .world()
            .get::<Mesh3d>(right)
            .expect("neighbour remeshed");
        let meshes = app.world().resource::<Assets<Mesh>>();
        assert_eq!(meshes.get(&mesh.0).unwrap().count_vertices(), 6 * 4);
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
        assert_eq!(voxel(&app, left, 14, 1, 5), Voxel::Dirt);
        assert_eq!(voxel(&app, left, 13, 1, 5), Voxel::Air);
    }

    #[test]
    fn number_keys_pick_every_hotbar_slot() {
        let (mut app, _, _) = app();
        for (index, key) in HOTBAR_KEYS.into_iter().enumerate() {
            press_key(&mut app, key);
            assert_eq!(app.world().resource::<ActiveBlock>().0, index);
        }
    }
}
