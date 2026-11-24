//! Our own material and shader for chunk meshes, replacing Bevy's PBR
//! `StandardMaterial`.
//!
//! No lighting: the shader shows the block atlas's pixels as they are, dimmed
//! by a fixed amount per face direction so block edges stay readable. The
//! shader is `assets/shaders/voxel.wesl` and hot reloads in dev builds.
//!
//! The material doesn't sample the atlas PNG directly. Whenever the PNG
//! loads (or hot reloads), [`block_texture::build`] turns it into a
//! mipmapped array texture, one layer per tile, and the material samples
//! that.

use bevy::{prelude::*, render::render_resource::AsBindGroup, shader::ShaderRef};

use crate::block_texture::{self, ATLAS_PATH};

/// The chunk shader, relative to `assets/`.
pub const SHADER_PATH: &str = "shaders/voxel.wesl";

pub struct VoxelMaterialPlugin;

impl Plugin for VoxelMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<VoxelMaterial>::default())
            .init_resource::<BlockMaterial>()
            .add_systems(Update, build_block_texture);
    }
}

/// An unlit material that samples the block texture: an array with one
/// layer per [`block_texture::Tile`]. Bindings must match `assets/shaders/voxel.wesl`.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct VoxelMaterial {
    #[texture(0, dimension = "2d_array")]
    #[sampler(1)]
    pub blocks: Handle<Image>,
}

impl Material for VoxelMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    /// Nothing is lit, so there's nothing to cast shadows onto.
    fn enable_shadows() -> bool {
        false
    }
}

/// The material every chunk mesh shares, and the textures behind it.
#[derive(Resource, Debug, Clone)]
pub struct BlockMaterial {
    pub material: Handle<VoxelMaterial>,
    /// The atlas PNG as loaded. The material's texture is built from it.
    pub atlas: Handle<Image>,
}

impl FromWorld for BlockMaterial {
    fn from_world(world: &mut World) -> Self {
        let atlas = world.resource::<AssetServer>().load(ATLAS_PATH);
        // A handle with no image behind it yet: chunks don't draw until the
        // atlas has loaded and the real texture replaces it.
        let blocks = world.resource::<Assets<Image>>().reserve_handle();
        let material = world
            .resource_mut::<Assets<VoxelMaterial>>()
            .add(VoxelMaterial { blocks });
        Self { material, atlas }
    }
}

/// Rebuilds the block texture whenever the atlas PNG loads or changes.
///
/// Each rebuild is a new image asset, not an overwrite of the old one: when
/// an image's size and format don't change, Bevy updates the existing GPU
/// texture by copying in mip 0 only, which would leave stale mips and read
/// our layer-major data in the wrong order. A new asset is uploaded whole.
fn build_block_texture(
    mut events: MessageReader<AssetEvent<Image>>,
    block: Res<BlockMaterial>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<VoxelMaterial>>,
) {
    let changed = events.read().any(|event| match event {
        AssetEvent::LoadedWithDependencies { id } | AssetEvent::Modified { id } => {
            *id == block.atlas.id()
        }
        _ => false,
    });
    if !changed {
        return;
    }
    let Some(atlas) = images.get(&block.atlas) else {
        return;
    };
    let texture = match block_texture::build(atlas) {
        Ok(texture) => texture,
        Err(error) => {
            error!("can't build block texture from {ATLAS_PATH}: {error}");
            return;
        }
    };
    let texture = images.add(texture);
    // Assigning through `get_mut` marks the material modified, so its bind
    // group is rebuilt with the new texture. The old texture is dropped.
    if let Some(mut material) = materials.get_mut(&block.material) {
        material.blocks = texture;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_texture::Tile;

    /// The shader declares the bindings the material provides, at the same
    /// indices, and the fragment entry point Bevy looks for.
    #[test]
    fn shader_matches_material_bindings() {
        let shader = std::fs::read_to_string(format!("assets/{SHADER_PATH}")).unwrap();
        assert!(shader.contains("@binding(0) var blocks: texture_2d_array<f32>"));
        assert!(shader.contains("@binding(1) var blocks_sampler: sampler"));
        assert!(shader.contains("fn fragment("));
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Image>()
            .init_asset::<VoxelMaterial>()
            .register_asset_loader(bevy::image::ImageLoader::new(
                bevy::image::CompressedImageFormats::NONE,
            ))
            .init_resource::<BlockMaterial>()
            .add_systems(Update, build_block_texture);
        app
    }

    /// The material's texture: runs the app until the atlas has loaded and
    /// a texture has been built from it.
    fn built_texture(app: &mut App) -> Handle<Image> {
        for _ in 0..400 {
            app.update();
            let block = app.world().resource::<BlockMaterial>();
            let materials = app.world().resource::<Assets<VoxelMaterial>>();
            let blocks = materials.get(&block.material).unwrap().blocks.clone();
            if app.world().resource::<Assets<Image>>().contains(&blocks) {
                return blocks;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("block texture never built");
    }

    #[test]
    fn block_texture_is_built_once_the_atlas_loads() {
        let mut app = app();
        let blocks = built_texture(&mut app);
        let texture = app
            .world()
            .resource::<Assets<Image>>()
            .get(&blocks)
            .unwrap();
        let size = texture.texture_descriptor.size;
        assert_eq!(size.depth_or_array_layers, Tile::COUNT as u32);
        assert_eq!(texture.texture_descriptor.mip_level_count, 5);
    }

    /// Changing the atlas (as hot reload does) builds a fresh texture asset,
    /// so it's uploaded whole, and marks the material modified, so its bind
    /// group picks the new texture up.
    #[test]
    fn changing_the_atlas_rebuilds_the_texture_and_updates_the_material() {
        let mut app = app();
        let old = built_texture(&mut app);
        // Read material events from here on.
        let mut material_events = app
            .world()
            .resource::<Messages<AssetEvent<VoxelMaterial>>>()
            .get_cursor_current();

        // Repaint the atlas solid red, through `get_mut` like a reload.
        let atlas = app.world().resource::<BlockMaterial>().atlas.clone();
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .get_mut(&atlas)
            .unwrap()
            .data
            .as_mut()
            .unwrap()
            .chunks_mut(4)
            .for_each(|pixel| pixel.copy_from_slice(&[255, 0, 0, 255]));
        app.update();
        app.update();

        let block = app.world().resource::<BlockMaterial>();
        let new = app
            .world()
            .resource::<Assets<VoxelMaterial>>()
            .get(&block.material)
            .unwrap()
            .blocks
            .clone();
        assert_ne!(new.id(), old.id(), "rebuilt texture must be a new asset");
        let texture = app.world().resource::<Assets<Image>>().get(&new).unwrap();
        // Every layer and mip is red now; check the last layer's smallest mip.
        let data = texture.data.as_ref().unwrap();
        assert_eq!(data[data.len() - 4..], [255, 0, 0, 255]);

        let modified = material_events
            .read(
                app.world()
                    .resource::<Messages<AssetEvent<VoxelMaterial>>>(),
            )
            .any(|event| event.is_modified(&block.material));
        assert!(modified, "material wasn't marked modified");
    }
}
