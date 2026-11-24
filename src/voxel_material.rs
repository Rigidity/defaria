//! Our own material and shader for chunk meshes, replacing Bevy's PBR
//! `StandardMaterial`.
//!
//! No lighting: the shader shows the block texture's pixels as they are, dimmed
//! by a fixed amount per face direction so block edges stay readable. The
//! shader is `assets/shaders/voxel.wesl` and hot reloads in dev builds.
//!
//! The material doesn't sample the layer PNGs directly. Once they've all
//! loaded, and again whenever one hot reloads, [`block_texture::build`]
//! colorizes and stacks them into a mipmapped array texture, one layer per
//! tile, and the material samples that.

use bevy::{
    asset::RenderAssetUsages, image::ImageLoaderSettings, prelude::*,
    render::render_resource::AsBindGroup, shader::ShaderRef,
};

use crate::block_texture;

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

/// The material every chunk mesh shares, and the images behind it.
#[derive(Resource, Debug, Clone)]
pub struct BlockMaterial {
    pub material: Handle<VoxelMaterial>,
    /// The grayscale layer images as loaded, by asset path. The material's
    /// texture is built from them.
    pub layers: Vec<(String, Handle<Image>)>,
}

impl FromWorld for BlockMaterial {
    fn from_world(world: &mut World) -> Self {
        let server = world.resource::<AssetServer>();
        let layers = block_texture::layer_paths()
            .into_iter()
            .map(|path| {
                // Read on the CPU only; the GPU gets the built texture.
                let handle = server
                    .load_builder()
                    .with_settings(|settings: &mut ImageLoaderSettings| {
                        settings.asset_usage = RenderAssetUsages::MAIN_WORLD;
                    })
                    .load(path.clone());
                (path, handle)
            })
            .collect();
        // A handle with no image behind it yet: chunks don't draw until the
        // layers have loaded and the real texture replaces it.
        let blocks = world.resource::<Assets<Image>>().reserve_handle();
        let material = world
            .resource_mut::<Assets<VoxelMaterial>>()
            .add(VoxelMaterial { blocks });
        Self { material, layers }
    }
}

/// Rebuilds the block texture once every layer image has loaded, and again
/// whenever one changes.
///
/// Each rebuild is a new image asset, not an overwrite of the old one: when
/// an image's size and format don't change, Bevy updates the existing GPU
/// texture by copying in mip 0 only, which would leave stale mips and read
/// our layer-major data in the wrong order. A new asset is uploaded whole.
fn build_block_texture(
    mut events: MessageReader<AssetEvent<Image>>,
    block: Res<BlockMaterial>,
    // A layer has loaded or changed since the texture was last built.
    mut stale: Local<bool>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<VoxelMaterial>>,
) {
    let changed = events.read().any(|event| match event {
        AssetEvent::LoadedWithDependencies { id } | AssetEvent::Modified { id } => {
            block.layers.iter().any(|(_, layer)| layer.id() == *id)
        }
        _ => false,
    });
    if changed {
        *stale = true;
    }
    if !*stale {
        return;
    }
    let loaded = |path: &str| {
        let (_, handle) = block.layers.iter().find(|(p, _)| p == path)?;
        images.get(handle)
    };
    // Wait for the rest to load.
    if block.layers.iter().any(|(path, _)| loaded(path).is_none()) {
        return;
    }
    let built = block_texture::build(loaded);
    *stale = false;
    let texture = match built {
        Ok(texture) => texture,
        Err(error) => {
            error!("can't build the block texture: {error}");
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
    use bevy::color::ColorToPacked;

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

    /// The material's texture: runs the app until the layers have loaded
    /// and a texture has been built from them.
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
    fn block_texture_is_built_once_the_layers_load() {
        let mut app = app();
        let blocks = built_texture(&mut app);
        let texture = app
            .world()
            .resource::<Assets<Image>>()
            .get(&blocks)
            .unwrap();
        let size = texture.texture_descriptor.size;
        assert_eq!(size.depth_or_array_layers, Tile::ALL.len() as u32);
        assert_eq!(texture.texture_descriptor.mip_level_count, 5);
    }

    /// Changing a layer (as hot reload does) builds a fresh texture asset,
    /// so it's uploaded whole, and marks the material modified, so its bind
    /// group picks the new texture up.
    #[test]
    fn changing_a_layer_rebuilds_the_texture_and_updates_the_material() {
        let mut app = app();
        let old = built_texture(&mut app);
        // Read material events from here on.
        let mut material_events = app
            .world()
            .resource::<Messages<AssetEvent<VoxelMaterial>>>()
            .get_cursor_current();

        // Repaint stone solid black, through `get_mut` like a reload.
        let stone = block_texture::layer_path("stone");
        let (_, layer) = app
            .world()
            .resource::<BlockMaterial>()
            .layers
            .iter()
            .find(|(path, _)| *path == stone)
            .unwrap()
            .clone();
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .get_mut(&layer)
            .unwrap()
            .data
            .as_mut()
            .unwrap()
            .chunks_mut(4)
            .for_each(|pixel| pixel.copy_from_slice(&[0, 0, 0, 255]));
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
        // Stone is the first array layer; its first pixel is the darkest
        // stone color now.
        let darkest = Tile::Stone.layers()[0].palette.color(0);
        let data = texture.data.as_ref().unwrap();
        assert_eq!(data[..4], darkest.to_u8_array());

        let modified = material_events
            .read(
                app.world()
                    .resource::<Messages<AssetEvent<VoxelMaterial>>>(),
            )
            .any(|event| event.is_modified(&block.material));
        assert!(modified, "material wasn't marked modified");
    }
}
