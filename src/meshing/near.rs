//! The translucent faces near the eyes, merged into one mesh and sorted as
//! a whole.
//!
//! Bevy draws each blended mesh whole, ordered by its centre, so sorting
//! each chunk's translucent faces ([`sort_translucent_faces`]) leaves one
//! chunk's drawn entirely before or after its neighbour's. Where they
//! overlap on screen (water in glass across a chunk border, seen at an
//! angle) that's a hard-edged band of the wrong blend. So the chunks
//! within [`NearRadius`] of the eyes' chunk draw their translucent faces in
//! one mesh instead, on the [`NearFaces`] entity, sorted face by face
//! across them all, and their own translucent parts are hidden ([`Merged`]).
//!
//! The eyes are always inside that cube of chunks, so along any line of
//! sight its faces are in front of every other chunk's: its mesh's material
//! draws after every other blended mesh
//! ([`VoxelMaterial::draws_last`](crate::voxel_material::VoxelMaterial::draws_last)).
//! Farther out, chunks still sort one by one, where their overlaps are far
//! smaller on screen.
//!
//! [`sort_translucent_faces`]: super::sort_translucent_faces

use bevy::{
    mesh::{Indices, PrimitiveTopology, VertexAttributeValues},
    prelude::*,
};

use super::{ChunkParts, SortedFrom, build::TranslucentFaces};
use crate::{
    player::{Eyes, LocalPlayer},
    voxel_material::BlockMaterial,
    world::{CHUNK_SIZE, ChunkCoord, ChunkMap},
};

/// How many chunks each way around the eyes' chunk have their translucent
/// faces merged: a cube of `2 * radius + 1` chunks a side. `None` merges
/// none, and every chunk sorts its own.
#[derive(Resource, Reflect, Debug, Clone, Copy, PartialEq)]
#[reflect(Resource)]
pub struct NearRadius(pub Option<i32>);

impl Default for NearRadius {
    fn default() -> Self {
        Self(Some(RADIUS))
    }
}

/// The usual [`NearRadius`].
pub const RADIUS: i32 = 2;

/// How far, in blocks, the eyes can go outside the middle chunk of the
/// merged cube before it's moved to theirs, so going back and forth across
/// a chunk border doesn't merge again each time.
const LEEWAY: i32 = 4;

/// A chunk's translucent part whose faces the [`NearFaces`] mesh draws.
/// It's hidden, and not sorted on its own.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct Merged;

/// The entity that draws the translucent faces near the eyes. Its
/// `Transform` is at the middle chunk's origin, and its mesh is in that
/// chunk's coordinates.
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq)]
#[reflect(Component)]
#[require(Transform, Visibility, TranslucentFaces, SortedFrom)]
pub struct NearFaces {
    /// The chunk in the middle of the merged cube, once there are eyes.
    middle: Option<ChunkCoord>,
    /// Each merged chunk, its translucent part and the mesh that had when
    /// the merged mesh was built, in the order they were merged in.
    members: Vec<(ChunkCoord, Entity, AssetId<Mesh>)>,
}

pub(super) fn spawn_near_faces(mut commands: Commands) {
    commands.spawn((Name::new("Near translucent faces"), NearFaces::default()));
}

/// The middle of the merged cube, for eyes at `eye`: the chunk it was in
/// (`middle`) while the eyes stay within [`LEEWAY`] of it, or else the
/// eyes' own chunk.
fn middle_for(eye: Vec3, middle: Option<ChunkCoord>) -> ChunkCoord {
    let block = eye.floor().as_ivec3();
    let near = |coord: ChunkCoord| {
        let low = coord.origin() - LEEWAY;
        let high = coord.origin() + CHUNK_SIZE as i32 + LEEWAY;
        block.cmpge(low).all() && block.cmplt(high).all()
    };
    match middle {
        Some(middle) if near(middle) => middle,
        _ => ChunkCoord::split(block).0,
    }
}

/// Merges the translucent faces of the chunks around the eyes into the
/// [`NearFaces`] mesh again whenever which chunks those are, or any of
/// their translucent meshes, changes, and sorts it from the eyes. Hides
/// the chunks' own translucent parts while they're merged, and shows a part
/// again (to be sorted on its own) once it's left out.
pub(super) fn merge_near_faces(
    mut commands: Commands,
    radius: Res<NearRadius>,
    eyes: Query<Eyes, With<LocalPlayer>>,
    map: Res<ChunkMap>,
    chunk_parts: Query<&ChunkParts>,
    translucent: Query<(&Mesh3d, &TranslucentFaces), Without<NearFaces>>,
    merged: Query<Entity, With<Merged>>,
    mut near: Query<
        (
            Entity,
            &mut NearFaces,
            &mut Transform,
            &mut TranslucentFaces,
            &mut SortedFrom,
        ),
        Without<LocalPlayer>,
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    material: Res<BlockMaterial>,
) {
    let Ok((entity, mut near, mut transform, mut faces, mut sorted)) = near.single_mut() else {
        return;
    };
    let eye = eyes.single().ok().map(|eyes| eyes.transform().translation);
    let middle = eye.map(|eye| middle_for(eye, near.middle));
    let mut members = Vec::new();
    if let (Some(middle), Some(radius)) = (middle, radius.0) {
        let span = -radius..=radius;
        for x in span.clone() {
            for y in span.clone() {
                for z in span.clone() {
                    let coord = ChunkCoord(middle.0 + IVec3::new(x, y, z));
                    let part = map
                        .get(coord)
                        .and_then(|chunk| chunk_parts.get(chunk).ok())
                        .map(|parts| parts.translucent);
                    if let Some(part) = part
                        && let Ok((mesh, _)) = translucent.get(part)
                    {
                        members.push((coord, part, mesh.id()));
                    }
                }
            }
        }
    }
    if middle == near.middle && members == near.members {
        return;
    }

    for part in &merged {
        if !members.iter().any(|(_, member, _)| *member == part) {
            commands
                .entity(part)
                .remove::<Merged>()
                .insert((Visibility::Inherited, SortedFrom::default()));
        }
    }
    for (_, part, _) in &members {
        commands.entity(*part).insert((Merged, Visibility::Hidden));
    }

    near.middle = middle;
    near.members = members;
    let (Some(middle), Some(eye)) = (middle, eye) else {
        commands.entity(entity).remove::<Mesh3d>();
        return;
    };
    let parts: Vec<_> = near
        .members
        .iter()
        .filter_map(|(coord, part, mesh)| {
            let (_, part_faces) = translucent.get(*part).ok()?;
            let offset = ((coord.0 - middle.0) * CHUNK_SIZE as i32).as_vec3();
            Some((meshes.get(*mesh)?, part_faces, offset))
        })
        .collect();
    let Some((mesh, merged_faces)) = merged_mesh(&parts) else {
        commands.entity(entity).remove::<Mesh3d>();
        *faces = TranslucentFaces::default();
        return;
    };
    let origin = middle.origin().as_vec3();
    let local_eye = eye - origin;
    let mut mesh = mesh;
    // Sorted even when it doesn't need it: it has no indices yet.
    mesh.insert_indices(Indices::U32(merged_faces.indices_from(local_eye)));
    *transform = Transform::from_translation(origin);
    *faces = merged_faces;
    *sorted = SortedFrom(Some(local_eye.floor().as_ivec3()));
    commands.entity(entity).insert((
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(material.near_translucent.clone()),
    ));
}

/// Sorts the [`NearFaces`] mesh again each block the eyes move, unless its
/// faces all lie in one plane. (Merging it sorts it too.)
pub(super) fn sort_near_faces(
    eyes: Query<Eyes, With<LocalPlayer>>,
    mut near: Query<(&Transform, &TranslucentFaces, &Mesh3d, &mut SortedFrom), With<NearFaces>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let (Ok(eyes), Ok((transform, faces, mesh, mut sorted))) = (eyes.single(), near.single_mut())
    else {
        return;
    };
    if !faces.needs_sorting() {
        return;
    }
    let eye = eyes.transform().translation - transform.translation;
    let at = eye.floor().as_ivec3();
    if sorted.0 == Some(at) {
        return;
    }
    sorted.0 = Some(at);
    if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
        mesh.insert_indices(Indices::U32(faces.indices_from(eye)));
    }
}

/// One mesh of several chunks' translucent meshes, each moved by its
/// offset, and their faces, numbered as in it. `None` if there are none.
/// The mesh has no indices yet: sort its faces for them.
pub(super) fn merged_mesh(
    parts: &[(&Mesh, &TranslucentFaces, Vec3)],
) -> Option<(Mesh, TranslucentFaces)> {
    let (first, ..) = parts.first()?;
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, first.asset_usage);
    let mut firsts = Vec::with_capacity(parts.len());
    let mut count = 0;
    for (part, ..) in parts {
        firsts.push(count);
        count += part.count_vertices() as u32;
    }
    for (attribute, _) in first.attributes() {
        let mut merged: Option<VertexAttributeValues> = None;
        for (part, _, offset) in parts {
            let Some(values) = part.attribute(attribute.id) else {
                continue;
            };
            let offset = (attribute.id == Mesh::ATTRIBUTE_POSITION.id).then_some(*offset);
            match &mut merged {
                Some(merged) => append(merged, values, offset),
                None => {
                    let mut values = values.clone();
                    if let (Some(offset), VertexAttributeValues::Float32x3(positions)) =
                        (offset, &mut values)
                    {
                        move_positions(positions, offset);
                    }
                    merged = Some(values);
                }
            }
        }
        if let Some(merged) = merged {
            mesh.insert_attribute(*attribute, merged);
        }
    }
    let faces = TranslucentFaces::merged(
        parts
            .iter()
            .zip(firsts)
            .map(|((_, faces, offset), first)| (*faces, *offset, first)),
    );
    Some((mesh, faces))
}

fn move_positions(positions: &mut [[f32; 3]], by: Vec3) {
    for position in positions {
        *position = (Vec3::from_array(*position) + by).to_array();
    }
}

/// Appends `more` to `values`, its positions moved by `offset` if it's
/// given. Every chunk mesh has the same attributes in the same formats.
fn append(values: &mut VertexAttributeValues, more: &VertexAttributeValues, offset: Option<Vec3>) {
    use VertexAttributeValues as V;
    match (values, more) {
        (V::Float32x3(values), V::Float32x3(more)) => {
            let start = values.len();
            values.extend_from_slice(more);
            if let Some(offset) = offset {
                move_positions(&mut values[start..], offset);
            }
        }
        (V::Float32x2(values), V::Float32x2(more)) => values.extend_from_slice(more),
        (V::Uint32x4(values), V::Uint32x4(more)) => values.extend_from_slice(more),
        (V::Uint32x2(values), V::Uint32x2(more)) => values.extend_from_slice(more),
        (V::Snorm16x2(values), V::Snorm16x2(more)) => values.extend_from_slice(more),
        _ => panic!("chunk meshes' vertex formats differ"),
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::{
        content::{base, block},
        meshing::{MeshingPlugin, testing::finish_meshing},
        player::tests::{local_player_at, put_eyes},
        voxel_material::VoxelMaterial,
        world::{Chunk, WorldPlugin, spawn_chunk},
    };

    /// The middle chunk stays put while the eyes stay within the leeway
    /// of it, and moves to theirs once they're past it.
    #[test]
    fn the_middle_moves_only_past_the_leeway() {
        let start = ChunkCoord(IVec3::ZERO);
        let at = |x: f32| Vec3::new(x, 8.0, 8.0);
        assert_eq!(middle_for(at(8.0), None), start);
        assert_eq!(middle_for(at(19.5), Some(start)), start);
        assert_eq!(middle_for(at(-3.5), Some(start)), start);
        assert_eq!(
            middle_for(at(20.5), Some(start)),
            ChunkCoord(IVec3::new(1, 0, 0))
        );
        assert_eq!(
            middle_for(at(-4.5), Some(start)),
            ChunkCoord(IVec3::new(-1, 0, 0))
        );
        // Back across the border, it stays in the new chunk.
        let next = ChunkCoord(IVec3::new(1, 0, 0));
        assert_eq!(middle_for(at(14.0), Some(next)), next);
    }

    /// Meshing and merging, with a pane of glass in each of two chunks one
    /// behind the other, and the local player's eyes.
    fn app() -> (App, Entity, [Entity; 2]) {
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
            translucent: reserve(),
            near_translucent: reserve(),
            ..BlockMaterial::unloaded(reserve())
        };
        app.insert_resource(materials);
        let player = app
            .world_mut()
            .spawn(local_player_at(Transform::from_xyz(8.5, 2.5, 30.0)))
            .id();
        let chunks = app
            .world_mut()
            .run_system_once(|mut commands: Commands, mut map: ResMut<ChunkMap>| {
                let mut spawn = |z: i32, pane: usize| {
                    let mut chunk = Chunk::default();
                    chunk.set(8, 2, pane, block("glass"));
                    spawn_chunk(
                        &mut commands,
                        &mut map,
                        ChunkCoord(IVec3::new(0, 0, z)),
                        chunk,
                        (),
                    )
                };
                [spawn(0, 2), spawn(1, 4)]
            })
            .unwrap();
        finish_meshing(&mut app);
        app.update();
        (app, player, chunks)
    }

    fn part(app: &App, chunk: Entity) -> Entity {
        app.world().get::<ChunkParts>(chunk).unwrap().translucent
    }

    fn near(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<NearFaces>>()
            .single(app.world())
            .unwrap()
    }

    /// The near mesh's faces in drawing order, as each one's middle in
    /// the world.
    fn near_faces(app: &mut App) -> Vec<Vec3> {
        let near = near(app);
        let world = app.world();
        let Some(mesh) = world.get::<Mesh3d>(near) else {
            return Vec::new();
        };
        let origin = world.get::<Transform>(near).unwrap().translation;
        let mesh = world.resource::<Assets<Mesh>>().get(mesh).unwrap();
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("no indices");
        };
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("no positions");
        };
        indices
            .chunks(6)
            .map(|face| {
                let sum: Vec3 = face
                    .iter()
                    .map(|&i| Vec3::from_array(positions[i as usize]))
                    .sum();
                origin + sum / 6.0
            })
            .collect()
    }

    fn hidden(app: &App, part: Entity) -> bool {
        app.world().get::<Visibility>(part) == Some(&Visibility::Hidden)
            && app.world().get::<Merged>(part).is_some()
    }

    /// Near the eyes, both chunks' panes are drawn in one mesh, where they
    /// are in the world, sorted across both: the far chunk's pane first,
    /// though its chunk is the near one's neighbour. Their own parts are
    /// hidden.
    #[test]
    fn near_chunks_draw_their_faces_in_one_sorted_mesh() {
        let (mut app, _, [far, close]) = app();
        let faces = near_faces(&mut app);
        assert_eq!(faces.len(), 12);
        assert!(faces[..6].iter().all(|f| f.z < 4.0), "{faces:?}");
        assert!(faces[6..].iter().all(|f| f.z > 19.0), "{faces:?}");
        // The pane's back face comes first within it, too.
        assert!(faces[0].z < faces[5].z, "{faces:?}");
        assert!(hidden(&app, part(&app, far)));
        assert!(hidden(&app, part(&app, close)));
        let near = near(&mut app);
        let material = app
            .world()
            .resource::<BlockMaterial>()
            .near_translucent
            .clone();
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<VoxelMaterial>>(near)
                .unwrap()
                .0,
            material
        );
    }

    /// Seen from the other side, the merged faces sort the other way
    /// round; moving within a block doesn't sort them again.
    #[test]
    fn the_near_mesh_sorts_as_the_eyes_move() {
        let (mut app, player, _) = app();
        put_eyes(
            app.world_mut(),
            player,
            Transform::from_xyz(8.5, 2.5, -10.2),
        );
        app.update();
        let faces = near_faces(&mut app);
        assert_eq!(faces.len(), 12);
        assert!(faces[..6].iter().all(|f| f.z > 19.0), "{faces:?}");

        let near = near(&mut app);
        let mesh = app.world().get::<Mesh3d>(near).unwrap().id();
        let mut events = app
            .world()
            .resource::<Messages<AssetEvent<Mesh>>>()
            .get_cursor_current();
        put_eyes(
            app.world_mut(),
            player,
            Transform::from_xyz(8.5, 2.5, -10.6),
        );
        app.update();
        let messages = app.world().resource::<Messages<AssetEvent<Mesh>>>();
        assert!(
            !events.read(messages).any(|e| e.is_modified(mesh)),
            "sorted again within a block"
        );
    }

    /// A chunk left out of the cube when the eyes move away draws its own
    /// faces again, sorted on its own.
    #[test]
    fn chunks_left_behind_draw_their_own_faces() {
        let (mut app, player, [far, close]) = app();
        // Three chunks north of the near pane's chunk.
        put_eyes(
            app.world_mut(),
            player,
            Transform::from_xyz(8.5, 2.5, -24.0),
        );
        app.update();
        app.update();
        let faces = near_faces(&mut app);
        assert_eq!(faces.len(), 6);
        assert!(hidden(&app, part(&app, far)));
        let close = part(&app, close);
        assert!(app.world().get::<Merged>(close).is_none());
        assert_eq!(
            app.world().get::<Visibility>(close),
            Some(&Visibility::Inherited)
        );
        assert!(app.world().get::<SortedFrom>(close).unwrap().0.is_some());
    }

    /// An edit in a merged chunk merges it again, with the new face.
    #[test]
    fn edits_merge_again() {
        let (mut app, _, _) = app();
        app.world_mut()
            .run_system_once(|mut voxels: crate::world::Voxels| {
                voxels.set(IVec3::new(4, 2, 10), block("glass"));
            })
            .unwrap();
        finish_meshing(&mut app);
        app.update();
        assert_eq!(near_faces(&mut app).len(), 18);
    }

    /// With no radius, nothing merges and every chunk draws its own.
    #[test]
    fn no_radius_merges_nothing() {
        let (mut app, _, [far, close]) = app();
        app.insert_resource(NearRadius(None));
        app.update();
        assert!(near_faces(&mut app).is_empty());
        for chunk in [far, close] {
            let part = part(&app, chunk);
            assert!(app.world().get::<Merged>(part).is_none());
            assert_eq!(
                app.world().get::<Visibility>(part),
                Some(&Visibility::Inherited)
            );
        }
    }
}
