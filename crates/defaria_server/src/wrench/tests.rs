use std::path::Path;

use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::{
    block_entity::{BlockEntityPlugin, RegisterBlockEntityPart},
    connections::ConnectionsPlugin,
    network::{NetworkCorePlugin, Networks},
    testing::world_app,
};
use defaria_core::{
    content::{Around, BlockBox, BlockState, base, block},
    testing::temp_folder,
};

const E: Direction = Direction::East;
const W: Direction = Direction::West;

fn pipe() -> BlockState {
    base().default_state("defaria:pipe").unwrap()
}

/// A pipe joined on `sides`.
fn joined_pipe(sides: &[Direction]) -> BlockState {
    let values: Vec<(&str, &str)> = sides.iter().map(|side| (side.name(), "true")).collect();
    base().block_state("defaria:pipe", &values).unwrap()
}

fn ray(from: Vec3, to: Vec3) -> Ray3d {
    Ray3d::new(from, Dir3::new(to - from).unwrap())
}

/// The side a wrench aimed from `from` at `to` works on, in the conduit
/// of `state` at the origin, with the blocks `around` it.
fn side_beside(state: BlockState, around: Around, from: Vec3, to: Vec3) -> Option<Direction> {
    let content = base();
    let ray = ray(from, to);
    let target = content.target(state, &around);
    let hit = defaria_core::block_edit::raycast(ray, 8.0, |pos| {
        if pos == IVec3::ZERO {
            target.boxes.clone()
        } else {
            std::borrow::Cow::Borrowed(&[][..])
        }
    })?;
    assert_eq!(hit.block, IVec3::ZERO);
    wrench_side(&target, ray, hit)
}

/// [`side_beside`], with nothing around it.
fn side_of(state: BlockState, from: Vec3, to: Vec3) -> Option<Direction> {
    side_beside(state, Around::AIR, from, to)
}

/// On an arm, the wrench works on the side the arm reaches, whichever of
/// the arm's faces it's aimed at; on the core, the face aimed at.
#[test]
fn the_side_is_the_arm_aimed_at_or_the_face() {
    let state = joined_pipe(&[E, W]);
    // Down onto the east arm, near its end.
    let arm = Vec3::new(0.9, 0.6875, 0.5);
    assert_eq!(side_of(state, arm + Vec3::Y, arm), Some(E));
    // From the north, at the side of the west arm.
    let arm = Vec3::new(0.1, 0.5, 0.3125);
    assert_eq!(side_of(state, arm - Vec3::Z, arm), Some(W));
    // Down onto the core: its top.
    let core = Vec3::new(0.5, 0.6875, 0.5);
    assert_eq!(side_of(state, core + Vec3::Y, core), Some(Direction::Up));
    // From the north onto the core: its north face, not joined yet.
    let core = Vec3::new(0.5, 0.5, 0.3125);
    assert_eq!(side_of(state, core - Vec3::Z, core), Some(Direction::North));
}

/// A box whose part reaches no side (a whole block, a core) is worked on
/// by the face clicked; one whose part reaches one, by that side, from
/// any of its faces.
#[test]
fn a_box_works_on_the_side_its_part_reaches() {
    use std::borrow::Cow;
    let whole = Target {
        boxes: Cow::Owned(vec![BlockBox::FULL]),
        reaches: Cow::Owned(vec![None]),
    };
    let from_above = ray(Vec3::new(0.5, 2.0, 0.5), Vec3::new(0.5, 0.5, 0.5));
    let hit = |target: &Target| {
        defaria_core::block_edit::raycast(from_above, 8.0, |pos| {
            if pos == IVec3::ZERO {
                target.boxes.clone()
            } else {
                Cow::Borrowed(&[][..])
            }
        })
        .unwrap()
    };
    assert_eq!(
        wrench_side(&whole, from_above, hit(&whole)),
        Some(Direction::Up)
    );
    let arm = Target {
        boxes: Cow::Owned(vec![BlockBox::FULL]),
        reaches: Cow::Owned(vec![Some(E)]),
    };
    assert_eq!(wrench_side(&arm, from_above, hit(&arm)), Some(E));
}

/// The blocked faces round trip through their saved form, and a mask
/// with bits past the six faces isn't read.
#[test]
fn blocked_faces_save_as_their_mask() {
    use defaria_core::block_entity::BlockEntityPart;
    let mut faces = BlockedFaces::default();
    faces.toggle(E);
    faces.toggle(Direction::Up);
    assert!(faces.has(E) && faces.has(Direction::Up) && !faces.has(W));
    let content = base();
    let load = |value: &defaria_core::block_entity::Value, version: u32, state: BlockState| {
        BlockedFaces::load(value, version, state, content)
    };
    let saved = faces.save(content);
    assert_eq!(load(&saved, 1, pipe()), Some(faces));
    assert_eq!(
        load(&defaria_core::block_entity::Value::Int(64), 1, pipe()),
        None
    );
    assert_eq!(load(&saved, 2, pipe()), None);
    // Only a conduit has blocked faces.
    assert_eq!(load(&saved, 1, block("stone")), None);
    faces.toggle(E);
    faces.toggle(Direction::Up);
    assert!(faces.is_empty());
}

/// Conduits have a block entity, with every face open and so no blocked
/// faces kept; other blocks don't.
#[test]
fn conduits_start_with_every_face_open() {
    use defaria_core::block_entity::BlockEntityPart;
    let content = base();
    assert!(content.block_of(pipe()).has_block_entity());
    assert_eq!(BlockedFaces::new(pipe(), content), None);
    // Turning a face adds them; turning it back takes them off.
    let east = turned(None, E);
    assert!(east.is_some_and(|faces| faces.has(E)));
    assert_eq!(turned(east, E), None);
    assert!(!content.block_of(block("stone")).has_block_entity());
    assert_eq!(BlockedFaces::new(block("stone"), content), None);
}

/// The world with block entities, connections and networks and no
/// rendering, loading around `HOME`, saved to `folder` if there is one.
fn app(folder: Option<&Path>) -> App {
    world_app(base().clone())
        .plugins((BlockEntityPlugin, ConnectionsPlugin, NetworkCorePlugin))
        .with(|app| {
            app.register_block_entity_part::<BlockedFaces>();
        })
        .folder(folder)
        .build()
}
/// Where a line of pipes runs east, in the air: `LINE` to `LINE + 4X`.
const LINE: IVec3 = IVec3::new(2, 50, -5);

/// A line of five pipes, the middle one at `LINE + 2X`.
fn lay_line(app: &mut App) {
    app.world_mut()
        .run_system_once(|mut voxels: Voxels| {
            for x in 0..5 {
                assert!(voxels.set(LINE + IVec3::X * x, pipe()));
            }
        })
        .unwrap();
    app.update();
}

fn get(app: &mut App, pos: IVec3) -> BlockState {
    app.world_mut()
        .run_system_once(move |voxels: Voxels| voxels.get(pos).unwrap())
        .unwrap()
}

fn faces(app: &App, pos: IVec3) -> Option<BlockedFaces> {
    let entity = app.world().resource::<BlockEntities>().get(pos)?;
    app.world().get::<BlockedFaces>(entity).copied()
}

/// Turns the middle pipe's `side` as a wrench does, then lets a frame
/// run.
fn toggle(app: &mut App, pos: IVec3, side: Direction) {
    let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
    let now = app.world().get::<BlockedFaces>(entity).copied();
    let mut entity = app.world_mut().entity_mut(entity);
    match turned(now, side) {
        Some(faces) => entity.insert(faces),
        None => entity.remove::<BlockedFaces>(),
    };
    app.update();
}

/// Whether the block entity at `pos` saves blocked faces.
fn saves_faces(app: &App, pos: IVec3) -> bool {
    let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
    app.world()
        .get::<crate::block_entity::SavedParts>(entity)
        .unwrap()
        .0
        .contains_key("blocked_faces")
}

/// How many networks the line is in, from its west end to its east.
fn networks_along(app: &App) -> Vec<Option<u64>> {
    let networks = app.world().resource::<Networks>();
    (0..5)
        .map(|x| networks.at(LINE + IVec3::X * x).map(|id| id.0))
        .collect()
}

fn distinct(ids: &[Option<u64>]) -> usize {
    let mut ids: Vec<_> = ids.iter().flatten().collect();
    ids.dedup();
    ids.len()
}

/// Blocking a face parts the conduits on both sides of it, from both
/// sides, and splits their network; opening it again joins them back.
#[test]
fn blocking_a_face_splits_the_network_and_opening_it_joins_it() {
    let mut app = app(None);
    lay_line(&mut app);
    let middle = LINE + IVec3::X * 2;
    assert_eq!(distinct(&networks_along(&app)), 1);
    assert_eq!(get(&mut app, middle), joined_pipe(&[E, W]));

    toggle(&mut app, middle, E);
    assert_eq!(get(&mut app, middle), joined_pipe(&[W]));
    // The pipe beyond lets go too, though its own face is open.
    assert_eq!(get(&mut app, middle + IVec3::X), joined_pipe(&[E]));
    let ids = networks_along(&app);
    assert_eq!(distinct(&ids), 2, "{ids:?}");
    assert_eq!(ids[0], ids[2]);
    assert_eq!(ids[3], ids[4]);

    // Blocking the face beyond, toward the blocked one, changes nothing
    // more; opening only one of the two keeps them apart.
    toggle(&mut app, middle + IVec3::X, W);
    toggle(&mut app, middle, E);
    assert_eq!(distinct(&networks_along(&app)), 2);
    toggle(&mut app, middle + IVec3::X, W);
    assert_eq!(get(&mut app, middle), joined_pipe(&[E, W]));
    assert_eq!(distinct(&networks_along(&app)), 1);
    // Every face open again: nothing kept or saved for them.
    assert_eq!(faces(&app, middle), None);
    assert!(!saves_faces(&app, middle) && !saves_faces(&app, LINE));
}

/// What a wrench aimed along `ray` turns, in `app`'s world.
fn target_of(app: &mut App, ray: Ray3d) -> Option<(IVec3, Direction)> {
    app.world_mut()
        .run_system_once(
            move |content: Res<Content>, voxels: Voxels, blocked: Blocked| {
                let hit = defaria_core::block_edit::raycast(ray, 8.0, |pos| {
                    target_at(&content, &voxels, pos).boxes
                })?;
                wrench_target(&content, &voxels, |pos| blocked.at(pos), ray, hit)
            },
        )
        .unwrap()
}

/// A face blocked between two conduits opens from either: aimed at the
/// other conduit's open face toward it, the wrench turns the blocked one
/// (provisional, for the user: docs/decisions.md).
#[test]
fn a_face_blocked_between_two_conduits_opens_from_either() {
    let mut app = app(None);
    lay_line(&mut app);
    let (middle, beyond) = (LINE + IVec3::X * 2, LINE + IVec3::X * 3);
    // At the west face of the core beyond, from the middle pipe's voxel
    // east of its core; and at the middle core's east face from beyond's.
    let at_beyond = ray(
        middle.as_vec3() + Vec3::new(0.95, 0.5, 0.5),
        beyond.as_vec3() + Vec3::new(0.5, 0.5, 0.5),
    );
    let at_middle = ray(
        beyond.as_vec3() + Vec3::new(0.05, 0.5, 0.5),
        middle.as_vec3() + Vec3::new(0.5, 0.5, 0.5),
    );
    toggle(&mut app, middle, E);
    assert_eq!(target_of(&mut app, at_beyond), Some((middle, E)));
    assert_eq!(target_of(&mut app, at_middle), Some((middle, E)));
    // Opened, and blocked from the other side instead.
    toggle(&mut app, middle, E);
    toggle(&mut app, beyond, W);
    assert_eq!(target_of(&mut app, at_beyond), Some((beyond, W)));
    assert_eq!(target_of(&mut app, at_middle), Some((beyond, W)));
}

/// Aimed at a machine's face where a conduit is joined to it, the wrench
/// blocks the conduit's face, and then opens it again; on a face no
/// conduit is joined to, it does nothing.
#[test]
fn a_joint_with_a_machine_turns_from_the_machine() {
    let mut app = app(None);
    let tank = LINE;
    let above = tank + IVec3::Y;
    app.world_mut()
        .run_system_once(move |mut voxels: Voxels| {
            assert!(voxels.set(tank, block("tank")));
            assert!(voxels.set(above, pipe()));
        })
        .unwrap();
    app.update();
    assert_eq!(get(&mut app, above), joined_pipe(&[Direction::Down]));
    // Down onto the tank's top, beside the pipe.
    let corner = tank.as_vec3() + Vec3::new(0.1, 1.0, 0.1);
    let at_tank = ray(corner + Vec3::Y * 2.0, corner);
    assert_eq!(target_of(&mut app, at_tank), Some((above, Direction::Down)));
    toggle(&mut app, above, Direction::Down);
    assert_eq!(get(&mut app, above), pipe());
    assert_eq!(target_of(&mut app, at_tank), Some((above, Direction::Down)));
    // At the tank's side, where nothing is joined: nothing.
    let side = tank.as_vec3() + Vec3::new(1.0, 0.5, 0.5);
    assert_eq!(target_of(&mut app, ray(side + Vec3::X * 2.0, side)), None);
}

/// A wire with every face open keeps nothing in its block entity, so its
/// chunk saves none for it; one with a face blocked saves its faces.
#[test]
fn open_wires_save_no_block_entity() {
    let mut app = app(None);
    let wire = base().default_state("defaria:wire").unwrap();
    let (open, blocked) = (LINE, LINE + IVec3::X * 3);
    app.world_mut()
        .run_system_once(move |mut voxels: Voxels| {
            assert!(voxels.set(open, wire) && voxels.set(blocked, wire));
        })
        .unwrap();
    app.update();
    toggle(&mut app, blocked, E);
    let saved = app
        .world_mut()
        .run_system_once(
            move |saver: crate::block_entity::ChunkSaver,
                  map: Res<defaria_core::world::ChunkMap>| {
                let (coord, _) = defaria_core::world::ChunkCoord::split(open);
                let mut chunk = saver.chunk(map.get(coord).unwrap()).unwrap();
                let mut at: Vec<IVec3> = chunk
                    .take_block_entities()
                    .keys()
                    .map(|&index| {
                        coord.origin() + defaria_core::world::Chunk::position_of(index as usize)
                    })
                    .collect();
                at.sort_by_key(|pos| pos.to_array());
                at
            },
        )
        .unwrap();
    assert_eq!(saved, [blocked]);
}

/// A pipe placed against a blocked face doesn't join it.
#[test]
fn a_pipe_placed_against_a_blocked_face_doesnt_join() {
    let mut app = app(None);
    app.world_mut()
        .run_system_once(|mut voxels: Voxels| assert!(voxels.set(LINE, pipe())))
        .unwrap();
    app.update();
    app.update();
    toggle(&mut app, LINE, E);
    app.world_mut()
        .run_system_once(|mut voxels: Voxels| assert!(voxels.set(LINE + IVec3::X, pipe())))
        .unwrap();
    app.update();
    assert_eq!(get(&mut app, LINE), pipe());
    assert_eq!(get(&mut app, LINE + IVec3::X), pipe());
}

/// Blocked faces are saved with the conduit and still block after a
/// restart, and the networks rebuilt on loading follow them.
#[test]
fn blocked_faces_survive_a_restart() {
    let folder = temp_folder("blocked-faces");
    let middle = LINE + IVec3::X * 2;
    {
        let mut app = app(Some(&folder));
        lay_line(&mut app);
        toggle(&mut app, middle, E);
        app.world_mut().write_message(AppExit::Success);
        app.update();
    }
    let mut app = app(Some(&folder));
    app.update();
    let mut blocked = BlockedFaces::default();
    blocked.toggle(E);
    assert_eq!(faces(&app, middle), Some(blocked));
    assert_eq!(faces(&app, LINE), None);
    assert_eq!(get(&mut app, middle), joined_pipe(&[W]));
    assert_eq!(get(&mut app, middle + IVec3::X), joined_pipe(&[E]));
    assert_eq!(distinct(&networks_along(&app)), 2);

    // And opening it after the restart joins them.
    toggle(&mut app, middle, E);
    assert_eq!(distinct(&networks_along(&app)), 1);
}

/// A face blocked toward air, on a conduit whose chunk was already
/// saved, is saved too: the block entity was there before, so blocking
/// it is a change to its chunk though nothing joins or parts.
#[test]
fn a_face_blocked_toward_air_after_a_restart_is_saved() {
    let folder = temp_folder("blocked-toward-air");
    {
        let mut app = app(Some(&folder));
        app.world_mut()
            .run_system_once(|mut voxels: Voxels| assert!(voxels.set(LINE, pipe())))
            .unwrap();
        app.update();
        app.world_mut().write_message(AppExit::Success);
        app.update();
    }
    {
        let mut app = app(Some(&folder));
        app.update();
        toggle(&mut app, LINE, E);
        app.world_mut().write_message(AppExit::Success);
        app.update();
    }
    let mut app = app(Some(&folder));
    app.update();
    let mut blocked = BlockedFaces::default();
    blocked.toggle(E);
    assert_eq!(faces(&app, LINE), Some(blocked));
}
