//! Breaking and placing blocks.
//!
//! - While the cursor is locked, left click breaks the block under the
//!   crosshair and right click places the active block against the face
//!   you're looking at. The click that locks the cursor doesn't break
//!   anything.
//! - The number keys 1–9 and 0 pick the active block from the [`HOTBAR`]
//!   items.
//! - A crosshair marks the middle of the screen, an outline shows the
//!   targeted block, and a hotbar along the bottom lists the blocks with the
//!   active one highlighted.
//!
//! Edits go through [`Voxels`], and the terrain remeshes the changed chunks
//! on the same frame.

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use crate::{
    content::{BlockState, Content},
    fly_camera::{FlyCamera, FlyCameraSystems},
    terrain::{RemeshChunks, Voxels},
};

/// How far away, in blocks, you can break or place.
pub const REACH: f32 = 8.0;

/// The items on the number keys, in order: key 1 is the first.
pub const HOTBAR: [&str; 10] = [
    "defaria:stone",
    "defaria:dirt",
    "defaria:grass",
    "defaria:sand",
    "defaria:gravel",
    "defaria:log",
    "defaria:planks",
    "defaria:lamp",
    "defaria:red_lamp",
    "defaria:blue_lamp",
];

const HOTBAR_KEYS: [KeyCode; HOTBAR.len()] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
    KeyCode::Digit0,
];

pub struct BlockEditPlugin;

impl Plugin for BlockEditPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Hotbar>()
            .init_resource::<ActiveBlock>()
            .add_systems(Startup, spawn_hud)
            .add_systems(
                Update,
                (
                    // Before the fly camera, so the click that locks the
                    // cursor finds it still unlocked and edits nothing.
                    (select_block, edit_blocks)
                        .chain()
                        .before(FlyCameraSystems)
                        .before(RemeshChunks),
                    update_hotbar,
                    // After the camera moves, so the outline matches the
                    // frame being drawn.
                    outline_target.after(FlyCameraSystems),
                ),
            );
    }
}

/// The hotbar's items, as the content defines them.
#[derive(Resource, Debug, Clone)]
pub struct Hotbar(pub Vec<HotbarItem>);

#[derive(Debug, Clone)]
pub struct HotbarItem {
    pub name: String,
    /// The block it places, if any.
    pub block: Option<BlockState>,
}

impl FromWorld for Hotbar {
    /// Looks up the [`HOTBAR`] items. One that didn't load is reported and
    /// leaves its slot empty.
    fn from_world(world: &mut World) -> Self {
        let content = world.resource::<Content>();
        Self(
            HOTBAR
                .iter()
                .map(|&id| match content.item(id) {
                    Some(item) => HotbarItem {
                        name: item.name.clone(),
                        block: item.block,
                    },
                    None => {
                        error!("the hotbar needs item `{id}`, which didn't load");
                        HotbarItem {
                            name: "?".into(),
                            block: None,
                        }
                    }
                })
                .collect(),
        )
    }
}

/// The hotbar slot whose item right click places.
#[derive(Resource, Reflect, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[reflect(Resource)]
pub struct ActiveBlock(pub usize);

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

/// The first block within `reach` along `ray` for which `solid` is true.
/// Steps through every block the ray touches, in order (Amanatides & Woo),
/// so it can't skip through corners.
pub fn raycast(ray: Ray3d, reach: f32, solid: impl Fn(IVec3) -> bool) -> Option<Hit> {
    let (origin, dir) = (ray.origin, *ray.direction);
    let mut block = origin.floor().as_ivec3();
    if solid(block) {
        return Some(Hit {
            block,
            normal: IVec3::ZERO,
        });
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
        if solid(block) {
            let mut normal = IVec3::ZERO;
            normal[axis] = -step[axis];
            return Some(Hit { block, normal });
        }
    }
}

fn select_block(keys: Res<ButtonInput<KeyCode>>, mut active: ResMut<ActiveBlock>) {
    if let Some(index) = HOTBAR_KEYS.iter().position(|&key| keys.just_pressed(key)) {
        active.0 = index;
    }
}

/// The ray from the camera through the crosshair.
fn camera_ray(transform: &Transform) -> Ray3d {
    Ray3d::new(transform.translation, transform.forward())
}

fn edit_blocks(
    mouse: Res<ButtonInput<MouseButton>>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    camera: Single<&Transform, With<FlyCamera>>,
    active: Res<ActiveBlock>,
    hotbar: Res<Hotbar>,
    mut voxels: Voxels,
) {
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
    let ray = camera_ray(&camera);
    let Some(hit) = raycast(ray, REACH, |pos| {
        voxels.get(pos).is_some_and(|state| !state.is_air())
    }) else {
        return;
    };
    if breaking {
        voxels.set(hit.block, BlockState::AIR);
        return;
    }
    let Some(block) = hotbar.0.get(active.0).and_then(|item| item.block) else {
        return;
    };
    let Some(pos) = hit.place_at() else {
        return;
    };
    // Not into the camera, outside the world, or over another block.
    if pos == ray.origin.floor().as_ivec3() || voxels.get(pos) != Some(BlockState::AIR) {
        return;
    }
    voxels.set(pos, block);
}

/// Outlines the block under the crosshair while the cursor is locked.
fn outline_target(
    mut gizmos: Gizmos,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    camera: Single<&Transform, With<FlyCamera>>,
    voxels: Voxels,
) {
    if cursor.grab_mode == CursorGrabMode::None {
        return;
    }
    let hit = raycast(camera_ray(&camera), REACH, |pos| {
        voxels.get(pos).is_some_and(|state| !state.is_air())
    });
    if let Some(hit) = hit {
        let center = hit.block.as_vec3() + Vec3::splat(0.5);
        gizmos.cube(
            Transform::from_translation(center).with_scale(Vec3::splat(1.004)),
            Color::srgba(0.05, 0.05, 0.05, 0.8),
        );
    }
}

/// One slot of the hotbar, showing [`Hotbar`]`[index]`.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
struct HotbarSlot(usize);

const SLOT_COLOR: Color = Color::srgba(0.0, 0.0, 0.0, 0.45);
const ACTIVE_SLOT_COLOR: Color = Color::srgba(0.95, 0.85, 0.35, 0.85);
const SLOT_TEXT: Color = Color::srgb(0.9, 0.9, 0.9);
const ACTIVE_SLOT_TEXT: Color = Color::srgb(0.1, 0.1, 0.1);

fn spawn_hud(mut commands: Commands, items: Res<Hotbar>) {
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
            for (index, item) in items.0.iter().enumerate() {
                hotbar.spawn((
                    HotbarSlot(index),
                    Node {
                        padding: UiRect::axes(px(10), px(6)),
                        ..default()
                    },
                    BackgroundColor(SLOT_COLOR),
                    children![(
                        // Key 0 is the tenth slot.
                        Text::new(format!("{} {}", (index + 1) % 10, item.name)),
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
    use bevy::log::LogPlugin;

    use super::*;
    use crate::{
        content::{base, block, test_mod},
        headless::{HeadlessPlugin, Script, default_plugins},
        terrain::{
            ChunkCoord, ChunkMap, Terrain, TerrainBlocks, VoxelChanged, remesh_changed_chunks,
        },
        voxel::Chunk,
        voxel_material::BlockMaterial,
    };

    fn solid_at(blocks: &[IVec3]) -> impl Fn(IVec3) -> bool + '_ {
        move |pos| blocks.contains(&pos)
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
        assert_eq!(hit.unwrap().place_at(), Some(IVec3::new(2, 0, 0)));
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
        assert_eq!(hit.place_at(), Some(IVec3::new(2, 2, 2)));
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
        let hit = raycast(ray(Vec3::splat(0.5), Vec3::X), 8.0, solid_at(&blocks)).unwrap();
        assert_eq!(hit.block, IVec3::ZERO);
        assert_eq!(hit.place_at(), None);
    }

    /// Two chunks side by side along X, a camera locked in front of them,
    /// and the edit and remesh systems; no rendering.
    fn app() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .insert_resource(BlockMaterial {
                material: Handle::default(),
                layers: default(),
            })
            .insert_resource(base().clone())
            .init_resource::<Hotbar>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ActiveBlock>()
            .init_resource::<ChunkMap>()
            .add_message::<VoxelChanged>()
            .add_systems(
                Update,
                (
                    (select_block, edit_blocks).chain().before(RemeshChunks),
                    remesh_changed_chunks.in_set(RemeshChunks),
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
        let mut left = Chunk::default();
        // A wall at x = 14..=15 of the left chunk, on the chunk border.
        for y in 0..4 {
            left.set(15, y, 5, block("stone"));
        }
        left.set(14, 1, 5, block("dirt"));
        let mut right = Chunk::default();
        right.set(0, 1, 5, block("stone"));
        let mut chunks = [Entity::PLACEHOLDER; 2];
        for (i, (coord, chunk)) in [(IVec3::ZERO, left), (IVec3::X, right)]
            .into_iter()
            .enumerate()
        {
            let entity = app.world_mut().spawn((ChunkCoord(coord), chunk)).id();
            app.world_mut()
                .resource_mut::<ChunkMap>()
                .insert(ChunkCoord(coord), entity);
            chunks[i] = entity;
        }
        // Looking along +X at block (14, 1, 5) from three blocks away.
        app.world_mut().spawn((
            FlyCamera::default(),
            Transform::from_xyz(11.5, 1.5, 5.5).looking_to(Vec3::X, Vec3::Y),
        ));
        // Meshes the new chunks and lets their change ticks pass.
        app.update();
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
        let before = mesh(&app, left);
        click(&mut app, MouseButton::Left);
        assert_eq!(voxel(&app, left, 14, 1, 5), BlockState::AIR);
        assert_eq!(voxel(&app, left, 15, 1, 5), block("stone"));
        assert_ne!(mesh(&app, left), before);
        // Again: the next block along is now the target.
        click(&mut app, MouseButton::Left);
        assert_eq!(voxel(&app, left, 15, 1, 5), BlockState::AIR);
    }

    #[test]
    fn holding_the_button_breaks_only_one_block() {
        let (mut app, left, _) = app();
        let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        mouse.press(MouseButton::Left);
        for _ in 0..3 {
            app.update();
            // The next frame of holding: still pressed, no longer new.
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
        }
        assert_eq!(voxel(&app, left, 14, 1, 5), BlockState::AIR);
        assert_eq!(voxel(&app, left, 15, 1, 5), block("stone"));
    }

    #[test]
    fn right_click_places_the_active_block_against_the_face() {
        let (mut app, left, _) = app();
        press_key(&mut app, KeyCode::Digit6);
        assert_eq!(*app.world().resource::<ActiveBlock>(), ActiveBlock(5));
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), block("log"));
        // Placing again stacks toward the camera, but never into it.
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 12, 1, 5), block("log"));
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 11, 1, 5), BlockState::AIR);
    }

    /// An item that places no block (a tool, or a slot whose item didn't
    /// load) does nothing on right click.
    #[test]
    fn right_click_with_an_item_that_places_nothing_does_nothing() {
        let (mut app, left, _) = app();
        app.insert_resource(Hotbar(vec![HotbarItem {
            name: "Pickaxe".into(),
            block: None,
        }]));
        click(&mut app, MouseButton::Right);
        assert_eq!(voxel(&app, left, 13, 1, 5), BlockState::AIR);
        assert_eq!(voxel(&app, left, 14, 1, 5), block("dirt"));
    }

    /// The hotbar takes its names and blocks from the items, and a slot
    /// whose item didn't load is left empty rather than shifting the keys.
    #[test]
    fn hotbar_comes_from_the_items() {
        let mut world = World::new();
        world.insert_resource(base().clone());
        let hotbar = Hotbar::from_world(&mut world);
        let names: Vec<&str> = hotbar.0.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Stone",
                "Dirt",
                "Grass",
                "Sand",
                "Gravel",
                "Log",
                "Planks",
                "Lamp",
                "Red Lamp",
                "Blue Lamp"
            ]
        );
        assert_eq!(hotbar.0[5].block, Some(block("log")));

        world.insert_resource(test_mod(&[]));
        let hotbar = Hotbar::from_world(&mut world);
        assert_eq!(hotbar.0.len(), HOTBAR.len());
        for item in hotbar.0 {
            assert_eq!((item.name.as_str(), item.block), ("?", None));
        }
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
        click(&mut app, MouseButton::Left);
        // Breaking (15, 1, 5) uncovers that face.
        click(&mut app, MouseButton::Left);
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
        click(&mut app, MouseButton::Left);
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
        assert_eq!(voxel(&app, left, 14, 1, 5), block("dirt"));
        assert_eq!(voxel(&app, left, 13, 1, 5), BlockState::AIR);
    }

    #[test]
    fn number_keys_pick_every_hotbar_slot() {
        let (mut app, _, _) = app();
        for (index, key) in HOTBAR_KEYS.into_iter().enumerate() {
            press_key(&mut app, key);
            assert_eq!(app.world().resource::<ActiveBlock>().0, index);
        }
    }

    #[test]
    fn hotbar_highlights_the_active_slot() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(base().clone())
            .init_resource::<Hotbar>()
            .init_resource::<ActiveBlock>()
            .add_systems(Startup, spawn_hud)
            .add_systems(Update, update_hotbar);
        for active in [0, 2, 6] {
            app.world_mut().resource_mut::<ActiveBlock>().0 = active;
            app.update();
            let mut slots = app
                .world_mut()
                .query::<(&HotbarSlot, &BackgroundColor, &Children)>();
            let slots: Vec<_> = slots
                .iter(app.world())
                .map(|(slot, background, children)| {
                    let text = app.world().get::<TextColor>(children[0]).unwrap().0;
                    (slot.0, background.0, text)
                })
                .collect();
            assert_eq!(slots.len(), HOTBAR.len());
            for (slot, background, text) in slots {
                let expected = if slot == active {
                    (ACTIVE_SLOT_COLOR, ACTIVE_SLOT_TEXT)
                } else {
                    (SLOT_COLOR, SLOT_TEXT)
                };
                assert_eq!((background, text), expected, "slot {slot}, active {active}");
            }
        }
    }

    fn world_voxel(app: &mut App, pos: IVec3) -> BlockState {
        let (coord, local) = ChunkCoord::split(pos);
        let mut chunks = app.world_mut().query::<(&ChunkCoord, &Chunk)>();
        chunks
            .iter(app.world())
            .find(|(c, _)| **c == coord)
            .map_or(BlockState::AIR, |(_, chunk)| chunk.at(local))
    }

    /// Through the real game and input pipeline: the click that locks the
    /// cursor edits nothing, the next one breaks the block under the
    /// crosshair, and a right click puts the active block (stone) back.
    #[test]
    fn locking_click_edits_nothing_then_clicks_break_and_place() {
        let script = Script::parse(
            "wait 2\n\
             hold MouseLeft 1  # lock the cursor\n\
             wait 1\n\
             hold MouseLeft 1  # break\n\
             wait 1\n\
             hold MouseRight 1  # place\n\
             wait 2\n",
        )
        .unwrap();
        let mut app = App::new();
        app.add_plugins((
            default_plugins(false).disable::<LogPlugin>(),
            crate::GamePlugin,
            HeadlessPlugin {
                script,
                rendering: false,
            },
        ));
        app.finish();
        app.cleanup();
        app.update();

        let (x, z) = (40, 40);
        let height = Terrain::new(default(), TerrainBlocks::from_content(base())).height(x, z);
        let top = IVec3::new(x, height - 1, z);
        let below = top - IVec3::Y;
        assert_eq!(world_voxel(&mut app, top), block("grass"));
        let below_before = world_voxel(&mut app, below);
        assert!(!below_before.is_air());
        // Three blocks above the top, looking straight down.
        let mut camera = app
            .world_mut()
            .query_filtered::<&mut Transform, With<FlyCamera>>();
        *camera.single_mut(app.world_mut()).unwrap() =
            Transform::from_xyz(x as f32 + 0.5, height as f32 + 3.5, z as f32 + 0.5)
                .looking_to(Vec3::NEG_Y, Vec3::Z);

        let mut frames = 1;
        while app.should_exit().is_none() {
            app.update();
            frames += 1;
            assert!(frames < 100, "script never finished");
        }
        // Grass broken once, not twice, and stone placed in its place.
        assert_eq!(world_voxel(&mut app, top), block("stone"));
        assert_eq!(world_voxel(&mut app, below), below_before);
    }
}
