//! Breaking and placing blocks with the mouse, end to end: input, the
//! edit and the remesh.

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use crate::{
    crosshair::{Crosshair, show_crosshair, spawn_crosshair},
    input::{mining::MiningInputPlugin, using::UseInputPlugin},
    ui::UiScreen,
};
use crate::{
    hotbar_ui::select_with_keys,
    meshing::{
        ModelFaces, apply_finished_meshes, model_faces::update_model_faces, remesh_changed_chunks,
        testing::finish_meshing,
    },
    voxel_material::BlockMaterial,
};
use bevy::math::DVec3;
use defaria_core::block_edit::*;
use defaria_core::{
    content::{BlockState, Content, Direction},
    game_mode::GameMode,
    hotbar::{ActiveSlot, Hotbar},
    inventory::Inventory,
    physics::Body,
    player::LocalPlayer,
    use_action::UseStep,
    world::{RemeshChunks, Voxels},
};
use defaria_core::{
    content::{FULL_FLUID, base, block, test_mod},
    inventory::player::new_player,
    mining::Mining,
    player::SIZE,
    testing::{chunk_app, insert_chunk},
    use_action::{UseActionPlugin, UseSystems},
    world::{Chunk, ChunkCoord},
};
use defaria_server::block_edit::BlockEditPlugin;
use defaria_server::{
    mining::MiningPlugin,
    script::{
        BlockUsed, ItemUsed, ScriptIndex,
        using::{use_scripted_block, use_scripted_item},
    },
};
use std::time::Duration;

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
    app.add_plugins((
        AssetPlugin::default(),
        BlockEditPlugin,
        MiningPlugin,
        MiningInputPlugin,
        UseActionPlugin,
        UseInputPlugin,
    ))
    .init_asset::<Mesh>()
    .init_resource::<ModelFaces>()
    .insert_resource(BlockMaterial::unloaded(Handle::default()))
    .init_resource::<ButtonInput<KeyCode>>()
    .init_resource::<ButtonInput<MouseButton>>()
    .add_message::<BlockUsed>()
    .add_message::<ItemUsed>()
    .add_systems(
        Update,
        (
            select_with_keys.before(UseSystems).before(RemeshChunks),
            (
                update_model_faces,
                remesh_changed_chunks,
                apply_finished_meshes,
            )
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
            defaria_core::player::tests::local_player_at(
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
    defaria_core::player::tests::put_local_eyes(
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
    assert!(target_shape(base(), block("water"), &defaria_core::content::Around::AIR).is_empty());
    assert!(target_shape(base(), block("lava"), &defaria_core::content::Around::AIR).is_empty());
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
        defaria_core::player::tests::put_local_eyes(
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
    // Raised half a block, a slab fits under it, but a block doesn't:
    // placed on the floor, from above.
    app.world_mut()
        .get_mut::<Chunk>(left)
        .unwrap()
        .set(13, 0, 5, block("stone"));
    app.world_mut().get_mut::<Body>(body).unwrap().position.y = 1.5;
    let from = Vec3::new(13.5, 3.4, 5.5);
    look_at(&mut app, from, Vec3::new(0.01, -1.0, 0.02).normalize());
    click(&mut app, MouseButton::Right);
    assert_eq!(voxel(&app, left, 13, 1, 5), BlockState::AIR);
    press_key(&mut app, KeyCode::Digit5);
    click(&mut app, MouseButton::Right);
    assert_eq!(voxel(&app, left, 13, 1, 5), block("stone_slab"));
    // Nor does its top half: that would go into the body too.
    click(&mut app, MouseButton::Right);
    assert_eq!(voxel(&app, left, 13, 1, 5), block("stone_slab"));
}

use defaria_core::content::slab;

/// Puts the item `id` in the first hotbar slot and in hand.
fn hold(app: &mut App, id: &str) {
    let mut hotbar = player(app).0;
    hotbar.assign(0, base().default_item(id));
    set_hotbar(app, hotbar);
    press_key(app, KeyCode::Digit1);
}

/// Holds left click until the voxel at `pos` changes.
fn mine_until_changed(app: &mut App, pos: IVec3) {
    let before = world_voxel(app, pos);
    let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
    mouse.release_all();
    mouse.clear();
    mouse.press(MouseButton::Left);
    for _ in 0..200 {
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        if world_voxel(app, pos) != before {
            break;
        }
    }
    assert_ne!(world_voxel(app, pos), before, "{pos} didn't change");
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    app.update();
}

/// A slab takes the half of its voxel nearest the click: on top of a
/// block it's a bottom half, under one a top half, and on a block's
/// side the half clicked. A vertical slab stands against the block
/// whose side is clicked, wherever on it, and on a floor or a ceiling
/// takes the half clicked.
#[test]
fn slabs_take_the_half_nearest_the_click() {
    let (mut app, left, _) = app();
    let target = IVec3::new(7, 7, 10);
    app.world_mut()
        .get_mut::<Chunk>(left)
        .unwrap()
        .set(7, 7, 10, block("stone"));
    // Places `item` with a click at `aim` on the target's `face` (in
    // the target's coordinates), from three blocks out, and clears it.
    let place = |app: &mut App, item: &str, face: Direction, aim: Vec3| {
        hold(app, item);
        let normal = face.normal();
        let from = (target + normal * 3).as_vec3() + Vec3::new(0.45, 0.5, 0.55);
        let dir = (target.as_vec3() + aim - from).normalize();
        let up = if normal.y == 0 { Vec3::Y } else { Vec3::Z };
        defaria_core::player::tests::put_local_eyes(
            app,
            Transform::from_translation(from).looking_to(dir, up),
        );
        click(app, MouseButton::Right);
        let pos = (target + normal).as_uvec3();
        let (x, y, z) = (pos.x as usize, pos.y as usize, pos.z as usize);
        let placed = voxel(app, left, x, y, z);
        app.world_mut()
            .get_mut::<Chunk>(left)
            .unwrap()
            .set(x, y, z, BlockState::AIR);
        placed
    };
    for (face, aim, half, vertical) in [
        (Direction::Up, Vec3::new(0.5, 1.0, 0.2), "bottom", "north"),
        (Direction::Up, Vec3::new(0.8, 1.0, 0.6), "bottom", "east"),
        (Direction::Down, Vec3::new(0.5, 0.0, 0.85), "top", "south"),
        (Direction::Down, Vec3::new(0.1, 0.0, 0.5), "top", "west"),
        (Direction::North, Vec3::new(0.2, 0.8, 0.0), "top", "south"),
        (
            Direction::North,
            Vec3::new(0.9, 0.2, 0.0),
            "bottom",
            "south",
        ),
        (
            Direction::South,
            Vec3::new(0.5, 0.3, 1.0),
            "bottom",
            "north",
        ),
        (Direction::East, Vec3::new(1.0, 0.7, 0.1), "top", "west"),
        (Direction::West, Vec3::new(0.0, 0.1, 0.5), "bottom", "east"),
    ] {
        let placed = place(&mut app, "defaria:stone_slab", face, aim);
        assert_eq!(placed, slab("stone_slab", half), "{face:?} at {aim}");
        let placed = place(&mut app, "defaria:stone_vertical_slab", face, aim);
        assert_eq!(
            placed,
            slab("stone_vertical_slab", vertical),
            "{face:?} at {aim}"
        );
    }
}

/// Clicking the open side of a slab with another slab in hand fills
/// its other half, with the slab in hand, making one voxel of two
/// halves of different blocks. Mining it breaks the half under the
/// crosshair, leaving the other.
#[test]
fn a_slab_placed_on_a_slabs_open_side_fills_its_other_half() {
    let (mut app, left, _) = app();
    let content = base();
    let pos = IVec3::new(13, 1, 5);
    // From the west, against the dirt's west face: a vertical slab
    // standing against it.
    hold(&mut app, "defaria:stone_vertical_slab");
    click(&mut app, MouseButton::Right);
    let stone = slab("stone_vertical_slab", "east");
    assert_eq!(voxel(&app, left, 13, 1, 5), stone);
    // A slab doesn't fit its open side: it goes in front, as against
    // any block.
    hold(&mut app, "defaria:planks_slab");
    click(&mut app, MouseButton::Right);
    assert_eq!(voxel(&app, left, 13, 1, 5), stone);
    assert_eq!(voxel(&app, left, 12, 1, 5), block("planks_slab"));
    app.world_mut()
        .get_mut::<Chunk>(left)
        .unwrap()
        .set(12, 1, 5, BlockState::AIR);
    // A vertical planks slab does.
    hold(&mut app, "defaria:planks_vertical_slab");
    click(&mut app, MouseButton::Right);
    let both = content
        .combine(slab("planks_vertical_slab", "west"), stone)
        .unwrap();
    assert_eq!(voxel(&app, left, 13, 1, 5), both);
    assert_eq!(voxel(&app, left, 12, 1, 5), BlockState::AIR);
    // The planks half is the one in front.
    mine_until_changed(&mut app, pos);
    assert_eq!(voxel(&app, left, 13, 1, 5), stone);
    mine_until_changed(&mut app, pos);
    assert_eq!(voxel(&app, left, 13, 1, 5), BlockState::AIR);

    // A bottom slab on the floor, filled from above.
    app.world_mut()
        .get_mut::<Chunk>(left)
        .unwrap()
        .set(13, 0, 5, block("stone"));
    let from = Vec3::new(13.5, 3.5, 5.5);
    look_at(&mut app, from, Vec3::new(0.01, -1.0, 0.02).normalize());
    hold(&mut app, "defaria:stone_slab");
    click(&mut app, MouseButton::Right);
    assert_eq!(voxel(&app, left, 13, 1, 5), slab("stone_slab", "bottom"));
    hold(&mut app, "defaria:thatch_slab");
    click(&mut app, MouseButton::Right);
    let both = content
        .combine(slab("stone_slab", "bottom"), slab("thatch_slab", "top"))
        .unwrap();
    assert_eq!(voxel(&app, left, 13, 1, 5), both);
    // Full now, so the next goes on top, as on any block.
    click(&mut app, MouseButton::Right);
    assert_eq!(voxel(&app, left, 13, 1, 5), both);
    assert_eq!(voxel(&app, left, 13, 2, 5), slab("thatch_slab", "bottom"));
}

/// Clicking a side of a half that it covers places beside it, as
/// against any block: a slab clicked on its side gets a slab next to
/// it, and a vertical slab clicked on its top gets one on top.
#[test]
fn clicking_a_halfs_covered_side_places_beside_it() {
    let (mut app, left, _) = app();
    let bottom = slab("stone_slab", "bottom");
    app.world_mut()
        .get_mut::<Chunk>(left)
        .unwrap()
        .set(13, 1, 5, bottom);
    look_at(&mut app, Vec3::new(11.5, 1.25, 5.5), Vec3::X);
    hold(&mut app, "defaria:stone_slab");
    click(&mut app, MouseButton::Right);
    assert_eq!(voxel(&app, left, 13, 1, 5), bottom);
    assert_eq!(voxel(&app, left, 12, 1, 5), bottom);

    let east = slab("stone_vertical_slab", "east");
    app.world_mut()
        .get_mut::<Chunk>(left)
        .unwrap()
        .set(13, 1, 5, east);
    look_at(
        &mut app,
        Vec3::new(13.75, 3.5, 5.5),
        Vec3::new(0.01, -1.0, 0.02).normalize(),
    );
    hold(&mut app, "defaria:stone_vertical_slab");
    click(&mut app, MouseButton::Right);
    assert_eq!(voxel(&app, left, 13, 1, 5), east);
    // On its top, a floor: in the half clicked, the east.
    assert_eq!(
        voxel(&app, left, 13, 2, 5),
        slab("stone_vertical_slab", "east")
    );
}

/// A vertical slab placed into a plant from above stands at the side of
/// its voxel you look toward, as there's no side of it you click.
#[test]
fn a_vertical_slab_on_a_plant_faces_the_view() {
    let (mut app, left, _) = app();
    app.world_mut()
        .get_mut::<Chunk>(left)
        .unwrap()
        .set(13, 2, 5, block("tuft"));
    hold(&mut app, "defaria:stone_vertical_slab");
    for (from, half) in [
        (Vec3::new(13.5, 4.5, 6.6), "north"),
        (Vec3::new(13.5, 4.5, 4.4), "south"),
        (Vec3::new(14.6, 4.5, 5.5), "west"),
    ] {
        let dir = (Vec3::new(13.5, 2.4, 5.5) - from).normalize();
        look_at(&mut app, from, dir);
        click(&mut app, MouseButton::Right);
        assert_eq!(
            voxel(&app, left, 13, 2, 5),
            slab("stone_vertical_slab", half),
            "from {from}"
        );
        app.world_mut()
            .get_mut::<Chunk>(left)
            .unwrap()
            .set(13, 2, 5, block("tuft"));
    }
}

/// Aiming over a slab's open half at a plant beyond it builds over the
/// plant, as without the slab, rather than filling the slab.
#[test]
fn a_plant_past_a_half_is_built_over() {
    let (mut app, left, _) = app();
    let bottom = slab("stone_slab", "bottom");
    let mut chunk = app.world_mut().get_mut::<Chunk>(left).unwrap();
    chunk.set(13, 2, 5, bottom);
    chunk.set(14, 2, 5, block("tuft"));
    look_at(&mut app, Vec3::new(11.5, 2.75, 5.5), Vec3::X);
    hold(&mut app, "defaria:stone_slab");
    click(&mut app, MouseButton::Right);
    assert_eq!(voxel(&app, left, 13, 2, 5), bottom);
    assert_eq!(voxel(&app, left, 14, 2, 5), bottom);
}

/// Placing against a face with a half just in front of it fills that
/// half's other side: aiming under a top slab at the wall behind it
/// puts a bottom half under it.
#[test]
fn placing_into_a_voxel_with_a_half_fills_the_rest() {
    let (mut app, left, _) = app();
    app.world_mut()
        .get_mut::<Chunk>(left)
        .unwrap()
        .set(13, 1, 5, slab("stone_slab", "top"));
    let from = Vec3::new(11.5, 1.25, 5.5);
    look_at(&mut app, from, Vec3::X);
    hold(&mut app, "defaria:planks_slab");
    click(&mut app, MouseButton::Right);
    let both = base()
        .combine(slab("planks_slab", "bottom"), slab("stone_slab", "top"))
        .unwrap();
    assert_eq!(voxel(&app, left, 13, 1, 5), both);
    // A full block in hand fills nothing.
    app.world_mut()
        .get_mut::<Chunk>(left)
        .unwrap()
        .set(13, 1, 5, slab("stone_slab", "top"));
    press_key(&mut app, KeyCode::Digit2);
    click(&mut app, MouseButton::Right);
    assert_eq!(voxel(&app, left, 13, 1, 5), slab("stone_slab", "top"));
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
    inventory.add(content, defaria_core::inventory::ItemStack::new(bush, 1));
    inventory.add(content, defaria_core::inventory::ItemStack::new(daisy, 1));
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
        inventory.add(content, defaria_core::inventory::ItemStack::new(item, 1));
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

/// In survival, filling a half uses up the slab placed and gives
/// nothing back; with no slab, nothing is filled.
#[test]
fn filling_a_half_in_survival_uses_up_the_slab_and_gives_nothing() {
    let (mut app, left, _) = app();
    let bottom = slab("stone_slab", "bottom");
    let set = |app: &mut App, state| {
        app.world_mut()
            .get_mut::<Chunk>(left)
            .unwrap()
            .set(13, 1, 5, state);
    };
    set(&mut app, bottom);
    let owner = survival_with(&mut app, &["defaria:thatch_slab"]);
    look_at(
        &mut app,
        Vec3::new(13.5, 3.5, 5.5),
        Vec3::new(0.01, -1.0, 0.02).normalize(),
    );
    click(&mut app, MouseButton::Right);
    let both = base().combine(bottom, slab("thatch_slab", "top")).unwrap();
    assert_eq!(voxel(&app, left, 13, 1, 5), both);
    assert_eq!(count(&app, owner, "defaria:thatch_slab"), 0);
    assert_eq!(count(&app, owner, "defaria:stone_slab"), 0);
    // None left: nothing happens.
    set(&mut app, bottom);
    click(&mut app, MouseButton::Right);
    assert_eq!(voxel(&app, left, 13, 1, 5), bottom);
}

/// Filling the open half of a slab in water leaves no room for the
/// water, which is pushed out of the voxel.
#[test]
fn filling_a_slab_in_water_pushes_the_water_out() {
    let (mut app, left, _) = app();
    let bottom = slab("stone_slab", "bottom");
    let mut chunk = app.world_mut().get_mut::<Chunk>(left).unwrap();
    chunk.set(13, 1, 5, bottom);
    chunk.set_fluid_at(
        IVec3::new(13, 1, 5),
        defaria_core::world::VoxelFluid::of(block("water"), FULL_FLUID / 2),
        FULL_FLUID / 2,
    );
    assert_eq!(fluid(&app, left, 13, 1, 5), FULL_FLUID / 2);
    look_at(
        &mut app,
        Vec3::new(13.5, 3.5, 5.5),
        Vec3::new(0.01, -1.0, 0.02).normalize(),
    );
    hold(&mut app, "defaria:planks_slab");
    click(&mut app, MouseButton::Right);
    let both = base().combine(bottom, slab("planks_slab", "top")).unwrap();
    assert_eq!(voxel(&app, left, 13, 1, 5), both);
    assert_eq!(fluid(&app, left, 13, 1, 5), 0);
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
    let mut voxels = bevy::ecs::system::SystemState::<(Res<Content>, Voxels)>::new(app.world_mut());
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
    use defaria_core::script::{ScriptRuntime, ScriptSettings, ScriptSources, Side};
    let runtime = ScriptRuntime::new(
        Side::Server,
        ScriptSources::read(&defaria_core::content::mods_dir(), &ids),
        ScriptSettings::untimed(),
    );
    app.insert_resource(ScriptIndex::new(base(), &runtime))
        .add_systems(
            Update,
            (
                use_scripted_block.in_set(UseStep::Block),
                use_scripted_item.in_set(UseStep::Item),
            ),
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

    press_key(&mut app, KeyCode::Digit1);
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::ShiftLeft);
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
