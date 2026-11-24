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

use crate::{block_texture, voxel::Tile};

/// The block texture atlas, relative to `assets/`. See [`crate::voxel::Tile`]
/// for its layout.
pub const ATLAS_PATH: &str = "textures/blocks.png";
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
/// layer per [`Tile`]. Bindings must match `assets/shaders/voxel.wesl`.
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
    /// The atlas PNG as loaded.
    pub atlas: Handle<Image>,
    /// The array texture built from [`Self::atlas`]. Empty until the atlas
    /// has loaded; chunks don't draw until then.
    pub blocks: Handle<Image>,
}

impl FromWorld for BlockMaterial {
    fn from_world(world: &mut World) -> Self {
        let atlas = world.resource::<AssetServer>().load(ATLAS_PATH);
        let blocks = world.resource::<Assets<Image>>().reserve_handle();
        let material = VoxelMaterial {
            blocks: blocks.clone(),
        };
        let material = world.resource_mut::<Assets<VoxelMaterial>>().add(material);
        Self {
            material,
            atlas,
            blocks,
        }
    }
}

/// Rebuilds the array texture whenever the atlas PNG loads or changes.
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
    match block_texture::build(atlas, Tile::COUNT as u32) {
        Ok(texture) => {
            images.insert(&block.blocks, texture).unwrap();
            // Touch the material so its bind group picks up the new texture.
            materials.get_mut(&block.material);
        }
        Err(error) => error!("can't build block texture from {ATLAS_PATH}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The atlas on disk must have one 16×16 tile per [`Tile`].
    #[test]
    fn atlas_matches_tile_layout() {
        let bytes = std::fs::read(format!("assets/{ATLAS_PATH}")).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        // IHDR is the first chunk: width and height are its first 8 bytes.
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        assert_eq!((width, height), (16 * Tile::COUNT as u32, 16));
    }

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

    #[test]
    fn block_texture_is_built_once_the_atlas_loads() {
        let mut app = app();
        let blocks = app.world().resource::<BlockMaterial>().blocks.clone();
        for _ in 0..200 {
            app.update();
            if app.world().resource::<Assets<Image>>().contains(&blocks) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let images = app.world().resource::<Assets<Image>>();
        let texture = images.get(&blocks).expect("block texture never built");
        let size = texture.texture_descriptor.size;
        assert_eq!(size.depth_or_array_layers, Tile::COUNT as u32);
        assert_eq!(texture.texture_descriptor.mip_level_count, 5);

        let block = app.world().resource::<BlockMaterial>();
        let materials = app.world().resource::<Assets<VoxelMaterial>>();
        assert_eq!(materials.get(&block.material).unwrap().blocks, blocks);
    }
}
