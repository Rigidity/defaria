use bevy::{ecs::system::RunSystemOnce, platform::collections::HashMap};

use super::{testing::finish_meshing, *};
use defaria_core::player::tests::local_player_at;
use defaria_core::{
    content::{BlockState, Direction, base, block},
    world::{Voxels, WorldPlugin, spawn_chunk},
};
use defaria_server::worldgen::{
    Terrain, TerrainBlocks, TerrainSettings,
    testing::{PATCH_SIZE, patch_chunks, spawn_patch},
};

fn terrain() -> Terrain {
    Terrain::new(
        TerrainSettings::default(),
        TerrainBlocks::from_content(base()),
    )
}

/// The world and meshing plugins with the systems they need and the test patch of
/// terrain, meshed, and a record of which chunks changed in each frame.
fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(base().clone())
        .add_plugins((AssetPlugin::default(), WorldPlugin, MeshingPlugin))
        .init_asset::<Mesh>()
        .insert_resource(BlockMaterial::unloaded(Handle::default()))
        .init_resource::<ChangedChunks>()
        .add_systems(Update, record_changed.in_set(RemeshChunks));
    spawn_patch(app.world_mut());
    finish_meshing(&mut app);
    app
}

#[derive(Resource, Default)]
struct ChangedChunks(Vec<IVec3>);

fn record_changed(chunks: Query<(&ChunkCoord, Ref<Chunk>)>, mut changed: ResMut<ChangedChunks>) {
    changed.0 = chunks
        .iter()
        .filter(|(_, chunk)| chunk.is_changed())
        .map(|(coord, _)| coord.0)
        .collect();
    changed.0.sort_by_key(|c| c.to_array());
}

fn meshes(app: &mut App) -> HashMap<ChunkCoord, Option<AssetId<Mesh>>> {
    let mut chunks = app.world_mut().query::<(&ChunkCoord, Option<&Mesh3d>)>();
    chunks
        .iter(app.world())
        .map(|(coord, mesh)| (*coord, mesh.map(|mesh| mesh.id())))
        .collect()
}

/// Faces between two solid voxels in different chunks are culled, so the
/// whole world's faces equal the faces of the terrain as one solid, trees'
/// trunks included.
#[test]
fn no_faces_hidden_between_chunks() {
    let mut app = app();
    let ids = meshes(&mut app);
    let assets = app.world().resource::<Assets<Mesh>>();
    let total: usize = ids
        .values()
        .flatten()
        .map(|&id| assets.get(id).unwrap().count_vertices() / 4)
        .sum();

    // Count the exposed faces of the opaque blocks (the ground and tree
    // trunks) directly from the generated voxels, with nothing outside the
    // patch.
    let terrain = terrain();
    let content = base();
    let chunks: HashMap<_, _> = patch_chunks().map(|c| (c, terrain.chunk(c))).collect();
    let at = |pos: IVec3| {
        let (coord, local) = ChunkCoord::split(pos);
        chunks
            .get(&coord)
            .map_or(BlockState::AIR, |chunk| chunk.at(local))
    };
    let mut expected = 0;
    for x in 0..PATCH_SIZE.x {
        for y in 0..PATCH_SIZE.y {
            for z in 0..PATCH_SIZE.z {
                let pos = IVec3::new(x, y, z);
                let block = at(pos);
                let state = content.state(block);
                if block.is_air() || !state.is_opaque() || state.is_fluid() {
                    continue;
                }
                expected += Direction::ALL
                    .iter()
                    .filter(|&&side| !content.hides(block, at(pos + side.normal()), side))
                    .count();
            }
        }
    }
    assert_eq!(total, expected);
}

#[test]
fn unchanged_chunks_keep_their_meshes() {
    let mut app = app();
    let before = meshes(&mut app);
    app.update();
    assert!(app.world().resource::<ChangedChunks>().0.is_empty());
    assert_eq!(meshes(&mut app), before);
}

/// An edit changes its chunk and every chunk across the edges and
/// corners it touches (their faces' corners can be shaded by it), and
/// only those are remeshed. Setting a voxel to what it already is
/// changes nothing.
#[test]
fn edits_on_a_border_remesh_the_chunks_across_it() {
    let mut app = app();
    let set = |app: &mut App, pos: IVec3, state: BlockState| {
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| {
                assert!(voxels.set(pos, state));
            })
            .unwrap();
        app.update();
        app.world().resource::<ChangedChunks>().0.clone()
    };
    let around = |chunk: IVec3, steps: IVec3| {
        let mut chunks = Vec::new();
        for x in [0, steps.x] {
            for y in [0, steps.y] {
                for z in [0, steps.z] {
                    chunks.push(chunk + IVec3::new(x, y, z));
                }
            }
        }
        chunks.sort_by_key(|c| c.to_array());
        chunks.dedup();
        chunks
    };
    let planks = block("planks");
    let chunk = IVec3::new(2, 1, 2);
    let origin = ChunkCoord(chunk).origin();
    let before = meshes(&mut app);
    let corner = origin + IVec3::new(0, 15, 15);
    let changed = set(&mut app, corner, planks);
    let expected = around(chunk, IVec3::new(-1, 1, 1));
    assert_eq!(changed.len(), 8);
    assert_eq!(changed, expected);
    // Every other chunk kept its mesh.
    let after = meshes(&mut app);
    for (coord, mesh) in &before {
        if !expected.contains(&coord.0) {
            assert_eq!(after[coord], *mesh, "{coord:?}");
        }
    }

    let changed = set(&mut app, origin + IVec3::new(15, 0, 0), planks);
    assert_eq!(changed, around(chunk, IVec3::new(1, -1, -1)));
    let changed = set(&mut app, origin + IVec3::new(0, 5, 7), planks);
    assert_eq!(changed, around(chunk, IVec3::new(-1, 0, 0)));
    let changed = set(&mut app, origin + IVec3::new(4, 5, 7), planks);
    assert_eq!(changed, [chunk]);
    assert!(set(&mut app, corner, planks).is_empty(), "already planks");
}

/// A chunk awaiting its neighbours keeps whatever mesh it has through
/// edits, and is meshed once the mark comes off.
#[test]
fn chunks_awaiting_neighbours_wait_to_be_meshed() {
    let mut app = app();
    let coord = ChunkCoord(IVec3::new(3, 0, 3));
    let entity = app.world().resource::<ChunkMap>().get(coord).unwrap();
    let before = meshes(&mut app)[&coord];
    app.world_mut()
        .entity_mut(entity)
        .insert(AwaitingNeighbours);
    let pos = coord.origin() + IVec3::new(8, 15, 8);
    app.world_mut()
        .run_system_once(move |mut voxels: Voxels| {
            assert!(voxels.set(pos, block("planks")));
        })
        .unwrap();
    finish_meshing(&mut app);
    assert_eq!(meshes(&mut app)[&coord], before, "meshed while awaiting");
    app.world_mut()
        .entity_mut(entity)
        .remove::<AwaitingNeighbours>();
    app.update();
    // Built in the background: still being built, or (if the task pool
    // got through it before the frame's end, as it can on a slow or
    // busy machine) already shown. Either way not left alone.
    let building = app.world().get::<MeshTask>(entity).is_some();
    let shown = meshes(&mut app)[&coord] != before;
    assert!(building || shown, "neither building nor rebuilt");
    finish_meshing(&mut app);
    assert_ne!(meshes(&mut app)[&coord], before);
}

#[test]
fn chunks_are_spawned_at_their_origin() {
    let mut app = app();

    let mut chunks = app
        .world_mut()
        .query::<(&ChunkCoord, &Transform, Option<&Mesh3d>)>();
    let mut meshed = 0;
    let mut seen: Vec<_> = chunks
        .iter(app.world())
        .map(|(coord, transform, mesh)| {
            meshed += mesh.is_some() as usize;
            assert_eq!(transform.translation, coord.origin().as_vec3());
            coord.0.to_array()
        })
        .collect();
    seen.sort();
    let mut expected: Vec<_> = patch_chunks().map(|c| c.0.to_array()).collect();
    expected.sort();
    assert_eq!(seen, expected);
    // Every bottom chunk has ground in it; empty upper ones get no mesh.
    assert!((64..=256).contains(&meshed), "{meshed} chunks have meshes");
}

/// A chunk with see-through blocks draws them on two child entities,
/// cut-out and translucent, each with its own material; most chunks
/// have none. The translucent faces are sorted farthest first from the
/// eyes, and again when they move far enough.
#[test]
fn see_through_faces_draw_on_their_own_parts_sorted() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(base().clone())
        .add_plugins((AssetPlugin::default(), WorldPlugin, MeshingPlugin))
        .init_asset::<Mesh>()
        .init_asset::<VoxelMaterial>();
    let reserve = || {
        app.world()
            .resource::<Assets<VoxelMaterial>>()
            .reserve_handle()
    };
    let materials = BlockMaterial {
        cutout: reserve(),
        translucent: reserve(),
        ..BlockMaterial::unloaded(reserve())
    };
    app.insert_resource(materials.clone());
    let mut chunk = Chunk::default();
    chunk.set(2, 2, 2, block("stone"));
    chunk.set(4, 2, 2, block("leaves"));
    chunk.set(6, 2, 2, block("glass"));
    chunk.set(6, 2, 12, block("glass"));
    // Away from the origin, so sorting from the world-space eye would
    // put the panes in the other order.
    let coord = ChunkCoord(IVec3::new(2, 0, -2));
    let origin = coord.origin();
    let player = app
        .world_mut()
        .spawn(local_player_at(Transform::from_translation(
            origin.as_vec3() + Vec3::new(6.5, 2.5, 30.0),
        )))
        .id();
    // A pond walled in with stone: its faces are all in one plane.
    let mut pond = Chunk::default();
    for (x, y, z) in Chunk::positions() {
        let edge = x == 0 || x == 15 || z == 0 || z == 15;
        match y {
            1 => pond.set(x, y, z, block("stone")),
            2 if edge => pond.set(x, y, z, block("stone")),
            2 => pond.set(x, y, z, block("water")),
            _ => {}
        }
    }
    let (plain, see_through, water) = app
        .world_mut()
        .run_system_once(move |mut commands: Commands, mut map: ResMut<ChunkMap>| {
            let mut plain = Chunk::default();
            plain.set(1, 1, 1, block("stone"));
            let mut spawn = |at: IVec3, chunk: Chunk| {
                spawn_chunk(&mut commands, &mut map, ChunkCoord(at), chunk, ())
            };
            (
                spawn(IVec3::ZERO, plain),
                spawn(coord.0, chunk.clone()),
                spawn(IVec3::new(2, 0, 1), pond.clone()),
            )
        })
        .unwrap();
    finish_meshing(&mut app);
    app.update();

    assert!(app.world().get::<ChunkParts>(plain).is_none());
    let parts = *app.world().get::<ChunkParts>(see_through).unwrap();
    let world = app.world();
    let material = |entity| {
        world
            .get::<MeshMaterial3d<VoxelMaterial>>(entity)
            .unwrap()
            .0
            .clone()
    };
    let faces = |entity| {
        let mesh = world.get::<Mesh3d>(entity).unwrap();
        world
            .resource::<Assets<Mesh>>()
            .get(mesh)
            .unwrap()
            .count_vertices()
            / 4
    };
    assert_eq!(faces(see_through), 6);
    assert_eq!(faces(parts.cutout), 6);
    // Two panes of glass.
    assert_eq!(faces(parts.translucent), 12);
    assert_eq!(material(see_through), materials.material);
    assert_eq!(material(parts.cutout), materials.cutout);
    assert_eq!(material(parts.translucent), materials.translucent);
    for part in [parts.cutout, parts.translucent] {
        assert_eq!(world.get::<ChildOf>(part).unwrap().parent(), see_through);
    }
    // Only the mesh that's sorted keeps a copy in the main world once it's
    // drawn: sorting rewrites its indices there.
    let usage = |entity| {
        let mesh = world.get::<Mesh3d>(entity).unwrap();
        world
            .resource::<Assets<Mesh>>()
            .get(mesh)
            .unwrap()
            .asset_usage
    };
    assert_eq!(usage(see_through), RenderAssetUsages::RENDER_WORLD);
    assert_eq!(usage(parts.cutout), RenderAssetUsages::RENDER_WORLD);
    assert_eq!(usage(parts.translucent), RenderAssetUsages::all());
    let pond = world.get::<ChunkParts>(water).unwrap().translucent;
    assert_eq!(usage(pond), RenderAssetUsages::RENDER_WORLD);

    // Seen from the south (+Z), the north pane's faces come first.
    let first_face_z = |app: &App| {
        let mesh = app.world().get::<Mesh3d>(parts.translucent).unwrap();
        let mesh = app.world().resource::<Assets<Mesh>>().get(mesh).unwrap();
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("no indices");
        };
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("no positions");
        };
        positions[indices[0] as usize][2]
    };
    assert!(first_face_z(&app) < 4.0);
    defaria_core::player::tests::put_eyes(
        app.world_mut(),
        player,
        Transform::from_translation(origin.as_vec3() + Vec3::new(6.5, 2.5, -20.0)),
    );
    app.update();
    assert!(first_face_z(&app) > 10.0);

    let set = |app: &mut App, pos: IVec3, state: BlockState| {
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| {
                voxels.set(pos, state);
            })
            .unwrap();
        finish_meshing(app);
        app.update();
    };
    // An edit rebuilds the mesh in the order it's built in, south pane
    // (z = 2) first; it's sorted again without the eyes moving.
    set(&mut app, origin + IVec3::new(6, 2, 7), block("glass"));
    assert!(first_face_z(&app) > 10.0, "not sorted after an edit");

    // A move within the same block doesn't sort it again.
    let sorted = *app.world().get::<SortedFrom>(parts.translucent).unwrap();
    let mesh = app.world().get::<Mesh3d>(parts.translucent).unwrap().id();
    let mut events = app
        .world()
        .resource::<Messages<AssetEvent<Mesh>>>()
        .get_cursor_current();
    defaria_core::player::tests::put_eyes(
        app.world_mut(),
        player,
        Transform::from_translation(origin.as_vec3() + Vec3::new(6.5, 2.5, -19.7)),
    );
    app.update();
    app.update();
    assert_eq!(
        *app.world().get::<SortedFrom>(parts.translucent).unwrap(),
        sorted
    );
    let messages = app.world().resource::<Messages<AssetEvent<Mesh>>>();
    assert!(
        !events.read(messages).any(|e| e.is_modified(mesh)),
        "re-sorted after a small move"
    );

    // Without glass, the translucent part draws nothing.
    // The pond's surface never needed sorting, however the eyes moved.
    let pond = app.world().get::<ChunkParts>(water).unwrap().translucent;
    assert!(app.world().get::<Mesh3d>(pond).is_some());
    assert_eq!(app.world().get::<SortedFrom>(pond), Some(&SortedFrom(None)));

    for z in [2, 7, 12] {
        set(&mut app, origin + IVec3::new(6, 2, z), BlockState::AIR);
    }
    assert!(app.world().get::<Mesh3d>(parts.translucent).is_none());
    assert!(app.world().get::<Mesh3d>(parts.cutout).is_some());
}

/// Nearby, a translucent mesh is sorted again each block the eyes
/// move. Far away, each chunk every 16 blocks, but not all at the same
/// place, so crossing a line doesn't sort every far chunk on one frame.
#[test]
fn far_chunks_sort_at_staggered_places() {
    let near = ChunkCoord(IVec3::ZERO);
    for x in 0..20 {
        let eye = Vec3::new(x as f32 + 0.5, 8.0, 8.0);
        assert_eq!(sort_position(eye, near), eye.floor().as_ivec3());
    }
    // Flying 32 blocks along X, 100 blocks from each of 64 chunks: each
    // sorts twice, at its own places.
    let mut places = HashSet::new();
    for i in 0..64 {
        let coord = ChunkCoord(IVec3::new(i % 8, i / 8, 3));
        let eye = |x: i32| Vec3::new(x as f32 + 0.5, 8.0, 108.0);
        let changes: Vec<i32> = (1..=32)
            .filter(|&x| sort_position(eye(x), coord) != sort_position(eye(x - 1), coord))
            .collect();
        assert_eq!(changes.len(), 2, "{coord:?}: {changes:?}");
        assert_eq!(changes[1] - changes[0], 16);
        places.insert(changes[0]);
    }
    assert!(places.len() >= 8, "only {} different places", places.len());
}

/// Drawing on top of core's test apps: meshes, and the material they use.
fn with_meshing(app: &mut App) {
    app.add_plugins(MeshingPlugin)
        .init_asset::<Mesh>()
        .insert_resource(BlockMaterial::unloaded(Handle::default()));
}

/// The chunk's mesh.
fn mesh_of(app: &App, coord: IVec3) -> Option<AssetId<Mesh>> {
    let entity = app.world().resource::<ChunkMap>().get(ChunkCoord(coord))?;
    app.world().get::<Mesh3d>(entity).map(|mesh| mesh.id())
}

fn set_voxel(app: &mut App, pos: IVec3, state: BlockState) {
    app.world_mut()
        .run_system_once(move |mut voxels: Voxels| {
            assert!(voxels.set(pos, state));
        })
        .unwrap();
    app.update();
    finish_meshing(app);
}

/// A lamp's light reaching into a chunk its edit didn't touch remeshes
/// that chunk too, from the light changing alone; chunks out of reach
/// keep their meshes.
#[test]
fn light_changes_remesh_the_chunks_they_reach() {
    let mut app = defaria_server::testing::patch_app(base().clone());
    with_meshing(&mut app);
    finish_meshing(&mut app);
    // High in the air, where the terrain doesn't reach: a stone in the
    // middle of one chunk, and a lamp 3 voxels into the next.
    let (lit, lamp_chunk) = (IVec3::new(1, 3, 2), IVec3::new(2, 3, 2));
    let origin = |coord: IVec3| coord * CHUNK_SIZE as i32;
    set_voxel(&mut app, origin(lit) + IVec3::splat(8), block("stone"));
    let before = mesh_of(&app, lit).expect("the stone has a mesh");
    let far = IVec3::new(7, 0, 7);
    let far_before = mesh_of(&app, far);
    assert!(far_before.is_some());

    set_voxel(
        &mut app,
        origin(lamp_chunk) + IVec3::new(3, 8, 8),
        block("red_lamp"),
    );
    assert_ne!(
        mesh_of(&app, lit),
        Some(before),
        "lit chunk wasn't remeshed"
    );
    assert_eq!(mesh_of(&app, far), far_before, "far chunk was remeshed");
}

/// Streamed in, the chunks with every chunk around them loaded are drawn,
/// and only those: the edge of what's loaded waits for its neighbours.
#[test]
fn streamed_chunks_are_drawn_once_their_neighbours_load() {
    let app = defaria_server::testing::world_app(base().clone())
        .with(with_meshing)
        .build();
    let mut chunks = app
        .world()
        .try_query::<(&ChunkCoord, Has<Meshed>, Has<AwaitingNeighbours>)>()
        .unwrap();
    let (mut drawn, mut waiting) = (0, 0);
    for (coord, meshed, awaiting) in chunks.iter(app.world()) {
        assert_eq!(meshed, !awaiting, "{coord:?}");
        drawn += meshed as usize;
        waiting += awaiting as usize;
    }
    assert!(drawn > 0 && waiting > 0, "{drawn} drawn, {waiting} waiting");
}
