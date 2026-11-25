//! Cracks drawn over the block being mined, worse as it nears breaking.
//!
//! One overlay entity follows [`Mining`]'s target. Its mesh is the target
//! state's own faces (so it fits a slab as well as a full block), pushed out
//! a hair so it isn't hidden in them, minus faces a neighbour covers, as the
//! chunk mesh leaves them out. Its material is one of [`STAGES`] crack
//! images, picked by the mining progress.
//!
//! The crack images are layer images (`images/cracks/stage_<n>.png` in the
//! base mod, drawn by `scripts/crack-stages.py`): transparent where there's
//! no crack, and multiplied into the block under them where there is, so a
//! gray level is how much of the block's color is left. They load as linear
//! values, so level 1 keeps 1/7 of the color and level 5 keeps 5/7.

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageLoaderSettings, ImageSampler},
    light::{NotShadowCaster, NotShadowReceiver},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

use crate::{
    content::{BlockState, Content, Direction},
    mining::{Mining, MiningSystems},
    terrain::Voxels,
};

/// Crack images, from a hairline crack to one about to break.
pub const STAGES: usize = 10;

/// How far the overlay sits out from the faces it covers, in blocks.
const LIFT: f32 = 0.002;

/// The crack image for `stage`, as an asset path.
pub fn image_path(stage: usize) -> String {
    format!("mods/defaria/images/cracks/stage_{stage}.png")
}

/// The crack stage for mining `progress` (0 to 1): an even share of the
/// time each, the last until the block breaks.
pub fn stage(progress: f32) -> usize {
    ((progress.max(0.0) * STAGES as f32) as usize).min(STAGES - 1)
}

pub struct CracksPlugin;

impl Plugin for CracksPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Cracks>()
            .add_systems(Startup, spawn_cracks)
            .add_systems(Update, show_cracks.after(MiningSystems));
    }
}

/// The crack overlay. `drawn` is the state and the hidden sides its mesh
/// was built for.
#[derive(Component, Reflect, Debug, Default)]
#[reflect(Component)]
pub struct Cracks {
    pub stage: usize,
    #[reflect(ignore)]
    drawn: Option<(BlockState, u8)>,
    #[reflect(ignore)]
    stages: Vec<Handle<StandardMaterial>>,
}

fn spawn_cracks(
    mut commands: Commands,
    server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let stages: Vec<_> = (0..STAGES)
        .map(|stage| {
            let image = server
                .load_builder()
                .with_settings(|settings: &mut ImageLoaderSettings| {
                    // The gray is a multiplier, not a color.
                    settings.is_srgb = false;
                    settings.sampler = ImageSampler::nearest();
                })
                .load(image_path(stage));
            materials.add(StandardMaterial {
                base_color_texture: Some(image),
                alpha_mode: AlphaMode::Multiply,
                unlit: true,
                // The block under it is fogged already.
                fog_enabled: false,
                ..default()
            })
        })
        .collect();
    commands.spawn((
        Name::new("Cracks"),
        Mesh3d(meshes.add(empty_mesh())),
        MeshMaterial3d(stages[0].clone()),
        Cracks {
            stages,
            ..default()
        },
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
        NotShadowReceiver,
    ));
}

/// Puts the overlay on the block being mined, at its stage, or hides it.
fn show_cracks(
    content: Res<Content>,
    miners: Query<&Mining>,
    voxels: Voxels,
    mut meshes: ResMut<Assets<Mesh>>,
    overlay: Single<(
        &mut Cracks,
        &mut Transform,
        &mut Visibility,
        &Mesh3d,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
) {
    let (mut cracks, mut transform, mut visibility, mesh, mut material) = overlay.into_inner();
    let Some((mining, (pos, state))) = miners
        .iter()
        .find_map(|mining| Some((mining, mining.target?)))
    else {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    let hidden = Direction::ALL
        .iter()
        .enumerate()
        .filter(|&(_, &side)| {
            let neighbour = voxels.get(pos + side.normal()).unwrap_or(BlockState::AIR);
            content.state(neighbour).occludes(side.opposite())
        })
        .fold(0u8, |bits, (i, _)| bits | 1 << i);
    if cracks.drawn != Some((state, hidden)) {
        cracks.drawn = Some((state, hidden));
        let shown = |side: Direction| {
            let i = Direction::ALL.iter().position(|&d| d == side).unwrap();
            hidden & 1 << i == 0
        };
        if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
            *mesh = crack_mesh(&content, state, shown);
        }
    }
    cracks.stage = stage(mining.progress);
    let wanted = cracks.stages[cracks.stage].clone();
    if material.0 != wanted {
        material.0 = wanted;
    }
    transform.translation = pos.as_vec3();
    visibility.set_if_neq(Visibility::Inherited);
}

fn empty_mesh() -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, Vec::<[f32; 3]>::new())
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, Vec::<[f32; 3]>::new())
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, Vec::<[f32; 2]>::new())
    .with_inserted_indices(Indices::U32(Vec::new()))
}

/// The faces of `state`, in its block's coordinates, lifted off them by
/// [`LIFT`], keeping faces that can be hidden only on sides `shown`
/// allows.
pub fn crack_mesh(content: &Content, state: BlockState, shown: impl Fn(Direction) -> bool) -> Mesh {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    for face in &content.state(state).faces {
        if face.cull.is_some_and(|side| !shown(side)) {
            continue;
        }
        let base = positions.len() as u32;
        for vertex in &face.vertices {
            positions.push((vertex.pos + face.normal * LIFT).to_array());
            normals.push(face.normal.to_array());
            uvs.push(vertex.uv.to_array());
        }
        // A triangle, or a quad as two counter-clockwise triangles, as the
        // chunk mesh has them.
        let order: &[u32] = if face.vertices.len() == 3 {
            &[0, 1, 2]
        } else {
            &[0, 1, 2, 0, 2, 3]
        };
        indices.extend(order.iter().map(|i| base + i));
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        content::{base, block},
        terrain::{ChunkCoord, ChunkMap, VoxelChanged},
        voxel::Chunk,
    };

    #[test]
    fn stages_split_the_progress_evenly() {
        assert_eq!(stage(0.0), 0);
        assert_eq!(stage(0.09), 0);
        assert_eq!(stage(0.1), 1);
        assert_eq!(stage(0.55), 5);
        assert_eq!(stage(0.99), 9);
        assert_eq!(stage(1.0), 9);
    }

    /// Every stage has its image, and each has every crack the one before
    /// had (the cracks only spread).
    #[test]
    fn every_stage_has_an_image_and_the_cracks_only_spread() {
        let opaque = |stage: usize| {
            let path = std::path::Path::new("assets").join(image_path(stage));
            let image = Image::from_buffer(
                &std::fs::read(&path).unwrap(),
                bevy::image::ImageType::Extension("png"),
                default(),
                true,
                ImageSampler::Default,
                RenderAssetUsages::MAIN_WORLD,
            )
            .unwrap();
            let pixels = image.data.unwrap();
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[3] > 0)
                .collect::<Vec<_>>()
        };
        let mut before = opaque(0);
        assert!(before.iter().any(|&p| p), "stage 0 has no crack");
        for stage in 1..STAGES {
            let now = opaque(stage);
            assert!(
                before.iter().zip(&now).all(|(&b, &n)| !b || n),
                "stage {stage} lost a crack"
            );
            assert!(now.iter().filter(|&&p| p).count() > before.iter().filter(|&&p| p).count());
            before = now;
        }
    }

    fn positions(mesh: &Mesh) -> Vec<Vec3> {
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            .unwrap()
            .as_float3()
            .unwrap()
            .iter()
            .map(|&p| Vec3::from(p))
            .collect()
    }

    /// The overlay fits the block's shape: a slab's is half as tall, just
    /// outside its faces.
    #[test]
    fn the_mesh_fits_the_shape_of_the_block() {
        let content = base();
        let full = positions(&crack_mesh(content, block("stone"), |_| true));
        let slab = positions(&crack_mesh(content, block("stone_slab"), |_| true));
        assert_eq!(full.len(), 6 * 4);
        assert_eq!(slab.len(), 6 * 4);
        let top = |points: &[Vec3]| points.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!((top(&full) - (1.0 + LIFT)).abs() < 1e-6);
        assert!((top(&slab) - (0.5 + LIFT)).abs() < 1e-6);
        // Outside the block on every side.
        for p in full {
            assert!(p.cmple(Vec3::splat(1.0 + LIFT) + 1e-6).all());
            assert!(p.cmpge(Vec3::splat(-LIFT) - 1e-6).all());
            assert!(p.min_element() < 0.0 || p.max_element() > 1.0);
        }
    }

    /// Sides a neighbour covers get no cracks; a slab's top, which nothing
    /// covers, always does.
    #[test]
    fn covered_sides_are_left_out() {
        let content = base();
        let only_up = |side| side == Direction::Up;
        assert_eq!(
            positions(&crack_mesh(content, block("stone"), only_up)).len(),
            4
        );
        assert_eq!(
            positions(&crack_mesh(content, block("stone_slab"), |_| false)).len(),
            4
        );
    }

    /// A chunk with a slab on a stone, a camera with [`Mining`], and the
    /// overlay; no rendering.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_asset::<StandardMaterial>()
            .insert_resource(base().clone())
            .init_resource::<ChunkMap>()
            .add_message::<VoxelChanged>()
            .add_systems(Startup, spawn_cracks)
            .add_systems(Update, show_cracks);
        let mut chunk = Chunk::default();
        chunk.set(5, 1, 5, block("stone_slab"));
        chunk.set(5, 0, 5, block("stone"));
        let entity = app.world_mut().spawn((ChunkCoord(IVec3::ZERO), chunk)).id();
        app.world_mut()
            .resource_mut::<ChunkMap>()
            .insert(ChunkCoord(IVec3::ZERO), entity);
        app.world_mut().spawn(Mining::default());
        app
    }

    fn mine(app: &mut App, target: Option<(IVec3, BlockState)>, progress: f32) {
        let mut miners = app.world_mut().query::<&mut Mining>();
        let mut mining = miners.single_mut(app.world_mut()).unwrap();
        mining.target = target;
        mining.progress = progress;
        app.update();
    }

    fn overlay(app: &mut App) -> (usize, Vec3, Visibility, Vec<Vec3>, bool) {
        let mut query = app.world_mut().query::<(
            &Cracks,
            &Transform,
            &Visibility,
            &Mesh3d,
            &MeshMaterial3d<StandardMaterial>,
        )>();
        let (cracks, transform, visibility, mesh, material) = query.single(app.world()).unwrap();
        let mesh = app.world().resource::<Assets<Mesh>>().get(&mesh.0).unwrap();
        (
            cracks.stage,
            transform.translation,
            *visibility,
            positions(mesh),
            material.0 == cracks.stages[cracks.stage],
        )
    }

    #[test]
    fn the_overlay_follows_the_block_being_mined() {
        let mut app = app();
        mine(&mut app, None, 0.0);
        assert_eq!(overlay(&mut app).2, Visibility::Hidden);

        let slab = IVec3::new(5, 1, 5);
        mine(&mut app, Some((slab, block("stone_slab"))), 0.55);
        let (stage, at, visibility, points, material) = overlay(&mut app);
        assert_eq!(
            (stage, at, visibility),
            (5, slab.as_vec3(), Visibility::Inherited)
        );
        assert!(material);
        // The slab's faces but its bottom, which the stone under it covers.
        assert_eq!(points.len(), 5 * 4);
        assert!(points.iter().all(|p| p.y >= 0.0));

        mine(&mut app, Some((slab, block("stone_slab"))), 0.95);
        assert_eq!(overlay(&mut app).0, 9);
        mine(&mut app, None, 0.0);
        assert_eq!(overlay(&mut app).2, Visibility::Hidden);

        // The stone under it: every face but its top, under the slab, and
        // its bottom, which is open.
        let stone = IVec3::new(5, 0, 5);
        mine(&mut app, Some((stone, block("stone"))), 0.0);
        let (stage, at, visibility, points, _) = overlay(&mut app);
        assert_eq!(
            (stage, at, visibility),
            (0, stone.as_vec3(), Visibility::Inherited)
        );
        assert_eq!(points.len(), 5 * 4);
        assert!(points.iter().all(|p| p.y <= 1.0));
    }
}
