//! Our own material and shader for chunk meshes, replacing Bevy's PBR
//! `StandardMaterial`.
//!
//! Bevy's lights don't touch it. The shader lights the block texture's pixels
//! with the light baked into the mesh (sky and block light, and ambient
//! occlusion; see `meshing` and `light`) and with the dynamic lights in
//! [`LightingUniform`]. The shader is `assets/shaders/voxel.wesl` and hot
//! reloads in dev builds. Its vertex stage is our own too, since chunk
//! meshes carry their light in custom vertex attributes.
//!
//! The material doesn't sample the layer PNGs directly. Once every image
//! the mods' textures use has loaded, and again whenever one hot reloads,
//! [`block_texture::build`] colorizes and stacks them into a mipmapped array
//! texture, one layer per [`Content`] texture, and the material samples
//! that.

use bevy::{
    asset::{AssetLoadFailedEvent, RenderAssetUsages},
    image::ImageLoaderSettings,
    mesh::{MeshVertexAttribute, MeshVertexBufferLayoutRef},
    pbr::{MaterialPipeline, MaterialPipelineKey},
    platform::collections::{HashMap, HashSet},
    prelude::*,
    render::{
        render_resource::{
            AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
        },
        storage::ShaderBuffer,
    },
    shader::ShaderRef,
};

use crate::{
    block_texture,
    content::Content,
    meshing::{ATTRIBUTE_CORNER, ATTRIBUTE_FACE, ATTRIBUTE_FLOW, ATTRIBUTE_LIGHT},
};

/// The most dynamic lights drawn at once: the ones nearest the camera.
/// Must match `MAX_DYNAMIC_LIGHTS` in the shader.
pub const MAX_DYNAMIC_LIGHTS: usize = 16;

/// The chunk shader, relative to `assets/`.
pub const SHADER_PATH: &str = "shaders/voxel.wesl";

pub struct VoxelMaterialPlugin;

impl Plugin for VoxelMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<VoxelMaterial>::default())
            .init_resource::<BlockMaterial>()
            .init_resource::<BlockTiles>()
            .add_systems(Update, build_block_texture);
    }
}

/// The block texture's tiles as last built, kept on the CPU for whatever
/// else draws with them (item icons): tile `i` is [`Content`] texture `i`,
/// [`block_texture::TILE_SIZE`]-square sRGB RGBA8 pixels. Empty until the
/// layers have loaded, and replaced on every rebuild, so `is_changed` means
/// the tiles changed.
#[derive(Resource, Debug, Clone, Default)]
pub struct BlockTiles(pub Vec<Vec<u8>>);

/// The chunk material: samples the block texture (an array with one layer
/// per [`Content`] texture) and lights it. Bindings must match
/// `assets/shaders/voxel.wesl`.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct VoxelMaterial {
    #[texture(0, dimension = "2d_array")]
    #[sampler(1)]
    pub blocks: Handle<Image>,
    #[uniform(2)]
    pub lighting: LightingUniform,
    /// Each texture's frames and how it moves
    /// ([`block_texture::Animations::words`]).
    #[storage(3, read_only, visibility(fragment))]
    pub animations: Handle<ShaderBuffer>,
    /// Opaque, cut out (alpha to coverage, with holes where the texture is
    /// transparent) or blended.
    pub alpha_mode: AlphaMode,
    /// Draws after every other blended mesh, whatever their distances: the
    /// translucent faces near the eyes (see `meshing::near`), which are in
    /// front of every other chunk's.
    pub draws_last: bool,
}

/// What [`VoxelMaterial::draws_last`] adds to a mesh's distance when Bevy
/// sorts blended meshes: more than any distance in the loaded world.
const DRAW_LAST_BIAS: f32 = 1.0e9;

/// The light that isn't baked into chunk meshes. Kept up to date by
/// `dynamic_light`.
#[derive(ShaderType, Debug, Clone, Copy, PartialEq)]
pub struct LightingUniform {
    /// Sky light's linear colour at full strength; `w` is unused.
    pub sky: Vec4,
    /// How many of `dynamic` are in use.
    pub dynamic_count: u32,
    pub dynamic: [GpuDynamicLight; MAX_DYNAMIC_LIGHTS],
}

impl Default for LightingUniform {
    fn default() -> Self {
        Self {
            sky: Vec4::ONE,
            dynamic_count: 0,
            dynamic: [GpuDynamicLight::default(); MAX_DYNAMIC_LIGHTS],
        }
    }
}

/// A dynamic light as the shader sees it.
#[derive(ShaderType, Debug, Clone, Copy, PartialEq, Default)]
pub struct GpuDynamicLight {
    /// World position; `w` is unused.
    pub position: Vec4,
    /// Red, green and blue light levels, as for block light; `w` is unused.
    pub levels: Vec4,
}

impl Material for VoxelMaterial {
    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    /// Bevy sorts blended meshes by their distance plus this, drawing the
    /// nearest (the largest) last.
    fn depth_bias(&self) -> f32 {
        if self.draws_last { DRAW_LAST_BIAS } else { 0.0 }
    }

    fn vertex_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    /// Bevy's lights don't reach this material, so there's nothing to cast
    /// shadows onto.
    fn enable_shadows() -> bool {
        false
    }

    /// The prepass would use Bevy's vertex shader, which doesn't know our
    /// vertex layout, and nothing needs one.
    fn enable_prepass() -> bool {
        false
    }

    /// Feeds the shader our vertex attributes, at the locations it reads
    /// them from.
    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let inputs: Vec<_> = VERTEX_INPUTS
            .iter()
            .enumerate()
            .map(|(location, (attribute, _))| attribute.at_shader_location(location as u32))
            .collect();
        descriptor.vertex.buffers = vec![layout.0.get_layout(&inputs)?];
        Ok(())
    }
}

/// The shader's vertex inputs, at locations 0, 1, 2, ... in this order:
/// each mesh attribute and how the shader declares it.
const VERTEX_INPUTS: [(MeshVertexAttribute, &str); 7] = [
    (Mesh::ATTRIBUTE_POSITION, "position: vec3<f32>"),
    (Mesh::ATTRIBUTE_NORMAL, "normal: vec3<f32>"),
    (Mesh::ATTRIBUTE_UV_0, "uv: vec2<f32>"),
    (ATTRIBUTE_CORNER, "corner: vec2<f32>"),
    (ATTRIBUTE_LIGHT, "light: vec4<u32>"),
    (ATTRIBUTE_FACE, "face: vec2<u32>"),
    (ATTRIBUTE_FLOW, "flow: vec2<f32>"),
];

/// The materials chunk meshes are drawn with, one per
/// [`Render`](crate::content::Render) and one for the translucent faces
/// near the eyes, and the images behind them. They differ only in how they
/// blend and sort: whatever changes one's texture or lighting changes them
/// all (see [`BlockMaterial::all`]).
#[derive(Resource, Debug, Clone)]
pub struct BlockMaterial {
    pub material: Handle<VoxelMaterial>,
    pub cutout: Handle<VoxelMaterial>,
    pub translucent: Handle<VoxelMaterial>,
    /// The translucent faces near the eyes, merged into one mesh (see
    /// `meshing::near`): blended, and drawn after every other chunk's.
    pub near_translucent: Handle<VoxelMaterial>,
    /// The grayscale layer images as loaded, by asset path. The material's
    /// texture is built from them.
    pub layers: HashMap<String, Handle<Image>>,
}

impl FromWorld for BlockMaterial {
    fn from_world(world: &mut World) -> Self {
        let paths = world.resource::<Content>().image_paths();
        let server = world.resource::<AssetServer>();
        let layers = paths
            .into_iter()
            .map(|path| {
                // The GPU only needs the built texture, so keep the layers
                // on the CPU. Hot reloads reapply these settings.
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
        let animations = world.resource::<Assets<ShaderBuffer>>().reserve_handle();
        let mut materials = world.resource_mut::<Assets<VoxelMaterial>>();
        let mut add = |alpha_mode, draws_last| {
            materials.add(VoxelMaterial {
                blocks: blocks.clone(),
                lighting: default(),
                animations: animations.clone(),
                alpha_mode,
                draws_last,
            })
        };
        Self {
            material: add(AlphaMode::Opaque, false),
            cutout: add(AlphaMode::AlphaToCoverage, false),
            translucent: add(AlphaMode::Blend, false),
            near_translucent: add(AlphaMode::Blend, true),
            layers,
        }
    }
}

impl BlockMaterial {
    /// Every material: opaque, cut-out, translucent and near translucent.
    pub fn all(&self) -> [&Handle<VoxelMaterial>; 4] {
        [
            &self.material,
            &self.cutout,
            &self.translucent,
            &self.near_translucent,
        ]
    }

    /// Materials with nothing behind them, for tests that don't draw.
    #[cfg(test)]
    pub fn unloaded(material: Handle<VoxelMaterial>) -> Self {
        Self {
            material,
            cutout: Handle::default(),
            translucent: Handle::default(),
            near_translucent: Handle::default(),
            layers: default(),
        }
    }
}

/// Rebuilds the block texture once every layer image has loaded, and again
/// whenever one changes.
///
/// Reacts to `Added` (first load) and `Modified` (hot reload, or a change
/// through `get_mut`). Not `LoadedWithDependencies`: a hot reload sends it
/// as well as `Modified`, often a frame apart, which would build twice.
/// Images have no dependencies, so `Added` already means loaded.
///
/// An image that fails to load counts as arrived, and its textures show the
/// placeholder tile, so the rest of the world still draws. That includes a
/// hot reload that fails: Bevy keeps the old image in `Assets`, but it's
/// treated as missing until a reload succeeds.
///
/// The first build waits until an event has arrived for every layer, not
/// just until every image is present: an image lands in `Assets` a frame
/// before its event can be read, so building on presence alone would build
/// once early and again when the last event arrives.
///
/// The animation table is rebuilt with it, since how many frames an image
/// holds can change in a reload too.
///
/// Each rebuild is a new image asset, not an overwrite of the old one: when
/// an image's size and format don't change, Bevy updates the existing GPU
/// texture by copying in mip 0 only, which would leave stale mips and read
/// our layer-major data in the wrong order. A new asset is uploaded whole.
fn build_block_texture(
    mut events: MessageReader<AssetEvent<Image>>,
    mut failures: MessageReader<AssetLoadFailedEvent<Image>>,
    block: Res<BlockMaterial>,
    content: Res<Content>,
    // Layers an `Added`, `Modified` or failure event has arrived for.
    mut seen: Local<HashSet<AssetId<Image>>>,
    // Layers whose latest load failed.
    mut failed: Local<HashSet<AssetId<Image>>>,
    mut images: ResMut<Assets<Image>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut materials: ResMut<Assets<VoxelMaterial>>,
    mut tiles: ResMut<BlockTiles>,
) {
    let mut changed = false;
    // Read every event, not just up to the first match: unread events stay
    // queued and would trigger another build next frame.
    for event in events.read() {
        if let AssetEvent::Added { id } | AssetEvent::Modified { id } = event
            && block.layers.values().any(|layer| layer.id() == *id)
        {
            seen.insert(*id);
            failed.remove(id);
            changed = true;
        }
    }
    for failure in failures.read() {
        if block.layers.values().any(|layer| layer.id() == failure.id) {
            seen.insert(failure.id);
            failed.insert(failure.id);
            changed = true;
        }
    }
    // Until the last layer arrives; its own event brings us back.
    let ready = block
        .layers
        .values()
        .all(|layer| seen.contains(&layer.id()));
    if !changed || !ready {
        return;
    }
    let built = block_texture::build(&content.textures, |path| {
        let layer = block.layers.get(path)?;
        if failed.contains(&layer.id()) {
            return None;
        }
        images.get(layer)
    });
    for error in built.errors {
        error!("can't build texture {error}; showing a placeholder");
    }
    let texture = images.add(built.image);
    let table = buffers.add(ShaderBuffer::new(
        built.animations.words(),
        RenderAssetUsages::RENDER_WORLD,
    ));
    tiles.0 = built.tiles;
    // Assigning through `get_mut` marks each material modified, so its
    // bind group is rebuilt with the new texture. The old texture is
    // dropped.
    for handle in block.all() {
        if let Some(mut material) = materials.get_mut(handle) {
            material.blocks = texture.clone();
            material.animations = table.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_texture::{
        TILE_SIZE,
        layer::test_images::{flat_gray, strip},
        mip_levels,
    };
    use crate::content::{base, base_texture};
    use bevy::color::ColorToPacked;

    /// The shader declares the bindings the material provides, at the same
    /// indices, and the fragment entry point Bevy looks for.
    #[test]
    fn shader_matches_material_bindings() {
        let shader = std::fs::read_to_string(format!("assets/{SHADER_PATH}")).unwrap();
        assert!(shader.contains("@binding(0) var blocks: texture_2d_array<f32>"));
        assert!(shader.contains("@binding(1) var blocks_sampler: sampler"));
        assert!(shader.contains("@binding(2) var<uniform> lighting: Lighting"));
        assert!(shader.contains("@binding(3) var<storage, read> animations: array<Animation>"));
        // The flow bits the mesher packs (see `fluid::scroll`).
        use crate::fluid::scroll;
        for constant in [
            format!("const TEXTURE_BITS: u32 = 0x{:08x}u;", scroll::TEXTURE_BITS),
            format!("const FLOWING: u32 = 0x{:08x}u;", scroll::FLOWING),
            format!("const WRAPPED: u32 = 0x{:08x}u;", scroll::WRAPPED),
        ] {
            assert!(
                shader.contains(&constant),
                "the shader doesn't declare {constant}"
            );
        }
        // The animation table's entries, word by word as `words` writes
        // them: the struct's fields in order, each 32 bits.
        let start = shader.find("struct Animation {").unwrap();
        let body = &shader[start..start + shader[start..].find("};").unwrap()];
        let fields: Vec<&str> = body
            .lines()
            .map(str::trim)
            .filter(|l| l.ends_with(','))
            .collect();
        assert_eq!(
            fields,
            [
                "frames: u32,",
                "rest: u32,",
                "frame_time: f32,",
                "scroll: f32,",
                "blend: u32,",
            ]
        );
        assert_eq!(fields.len(), block_texture::ANIMATION_WORDS);
        // The frame math `Animations::frame_at` spells out for the tests,
        // and the flow map: a shift of `scroll` blocks a second along the
        // vertex flow, two phases half a cycle apart, cross-faded by a
        // triangle weight.
        for line in [
            "return select(animation.rest + frame - 1u, texture, frame == 0u);",
            "let frame = u32(floor(steps)) % animation.frames;",
            "out.fade = select(0.0, fract(steps), animation.blend != 0u);",
            "let shift = in.flow * animation.scroll * FLOW_CYCLE;",
            "let first = fract(pixel_art_uv(in.uv - shift * phase, size, texels_per_pixel));",
            "let second = fract(pixel_art_uv(in.uv - shift * fract(phase + 0.5), size, texels_per_pixel));",
            "texel = mix(a, b, abs(1.0 - 2.0 * phase));",
        ] {
            assert!(shader.contains(line), "the shader doesn't say {line}");
        }
        assert!(shader.contains(&format!(
            "const MAX_DYNAMIC_LIGHTS: u32 = {MAX_DYNAMIC_LIGHTS}u;"
        )));
        assert!(shader.contains("fn vertex("));
        let back = format!(
            "const BACK_FACE: u32 = 0x{:08x}u;",
            crate::meshing::BACK_FACE
        );
        assert!(shader.contains(&back), "the shader doesn't declare {back}");
        // The vertex inputs at the locations `specialize` feeds them to.
        for (location, (_, declaration)) in VERTEX_INPUTS.iter().enumerate() {
            let input = format!("@location({location}) {declaration}");
            assert!(shader.contains(&input), "the shader doesn't read {input}");
        }
        assert!(shader.contains("fn fragment("));
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Image>()
            .init_asset::<VoxelMaterial>()
            .init_asset::<ShaderBuffer>()
            .insert_resource(base().clone())
            .register_asset_loader(bevy::image::ImageLoader::new(
                bevy::image::CompressedImageFormats::NONE,
            ))
            .init_resource::<BlockMaterial>()
            .init_resource::<BlockTiles>()
            .add_systems(Update, build_block_texture);
        app
    }

    /// The material's texture: runs the app until the layers have loaded
    /// and a texture has been built from them.
    fn built_texture(app: &mut App) -> Handle<Image> {
        for _ in 0..400 {
            app.update();
            let blocks = material_texture(app);
            if app.world().resource::<Assets<Image>>().contains(&blocks) {
                return blocks;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("block texture never built");
    }

    /// The animation table the material hands the shader right now.
    fn table(app: &App) -> Vec<u32> {
        let block = app.world().resource::<BlockMaterial>();
        let materials = app.world().resource::<Assets<VoxelMaterial>>();
        let handle = &materials.get(&block.material).unwrap().animations;
        let buffers = app.world().resource::<Assets<ShaderBuffer>>();
        buffers.get(handle).unwrap().cast_slice().unwrap().to_vec()
    }

    /// The texture the material samples right now.
    fn material_texture(app: &App) -> Handle<Image> {
        let block = app.world().resource::<BlockMaterial>();
        let materials = app.world().resource::<Assets<VoxelMaterial>>();
        materials.get(&block.material).unwrap().blocks.clone()
    }

    fn layer(app: &App, name: &str) -> Handle<Image> {
        let block = app.world().resource::<BlockMaterial>();
        block.layers[&format!("mods/defaria/images/{name}.png")].clone()
    }

    /// Fills a loaded layer with one RGBA color, through `get_mut` like a
    /// hot reload.
    fn paint(app: &mut App, layer: &Handle<Image>, rgba: [u8; 4]) {
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .get_mut(layer)
            .unwrap()
            .data
            .as_mut()
            .unwrap()
            .chunks_mut(4)
            .for_each(|pixel| pixel.copy_from_slice(&rgba));
    }

    /// Reads material events from now on; `modified` says whether the
    /// material was marked modified since.
    struct MaterialEvents(bevy::ecs::message::MessageCursor<AssetEvent<VoxelMaterial>>);

    impl MaterialEvents {
        fn new(app: &App) -> Self {
            let messages = app
                .world()
                .resource::<Messages<AssetEvent<VoxelMaterial>>>();
            Self(messages.get_cursor_current())
        }

        fn modified(&mut self, app: &App) -> bool {
            self.count(app) > 0
        }

        /// How many times the material was marked modified since the last
        /// call, which is once per build.
        fn count(&mut self, app: &App) -> usize {
            let material = app.world().resource::<BlockMaterial>().material.id();
            let messages = app
                .world()
                .resource::<Messages<AssetEvent<VoxelMaterial>>>();
            self.0
                .read(messages)
                .filter(|event| event.is_modified(material))
                .count()
        }
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
        // A layer per texture, and one per extra frame of animated ones.
        let extra: u32 = table(&app)
            .chunks(block_texture::ANIMATION_WORDS)
            .map(|entry| entry[0] - 1)
            .sum();
        assert!(extra > 0, "nothing is animated");
        assert_eq!(
            size.depth_or_array_layers,
            base().textures.len() as u32 + extra
        );
        assert_eq!(texture.texture_descriptor.mip_level_count, 5);
        // The same tiles are kept for the CPU.
        let tiles = &app.world().resource::<BlockTiles>().0;
        assert_eq!(tiles.len(), base().textures.len());
        let size = (TILE_SIZE * TILE_SIZE * 4) as usize;
        assert!(tiles.iter().all(|tile| tile.len() == size));
    }

    /// Changing a layer (as hot reload does) builds a fresh texture asset,
    /// so it's uploaded whole, and marks the material modified, so its bind
    /// group picks the new texture up.
    #[test]
    fn changing_a_layer_rebuilds_the_texture_and_updates_the_material() {
        let mut app = app();
        let old = built_texture(&mut app);
        let mut material_events = MaterialEvents::new(&app);

        // Repaint stone solid black, through `get_mut` like a reload.
        let stone = layer(&app, "stone");
        paint(&mut app, &stone, [0, 0, 0, 255]);
        app.update();
        app.update();

        let new = material_texture(&app);
        assert_ne!(new.id(), old.id(), "rebuilt texture must be a new asset");
        // The cut-out and translucent materials use the new texture too.
        let block = app.world().resource::<BlockMaterial>();
        let materials = app.world().resource::<Assets<VoxelMaterial>>();
        for handle in block.all() {
            assert_eq!(materials.get(handle).unwrap().blocks, new);
        }
        let texture = app.world().resource::<Assets<Image>>().get(&new).unwrap();
        // The stone tile's first pixel is the darkest stone color now.
        let darkest = base().materials["defaria:stone"].palette.color(0);
        let tile = base().texture("defaria:stone(rock=defaria:stone)").unwrap() as usize;
        let per_layer: u32 = (0..mip_levels(TILE_SIZE))
            .map(|mip| (TILE_SIZE >> mip).pow(2) * 4)
            .sum();
        let at = tile * per_layer as usize;
        let data = texture.data.as_ref().unwrap();
        assert_eq!(data[at..at + 4], darkest.to_u8_array());

        assert!(
            material_events.modified(&app),
            "material wasn't marked modified"
        );
    }

    /// Hot reloading an animated image with a different number of frames
    /// rebuilds the animation table with the texture, and every material
    /// gets the new table.
    #[test]
    fn reloading_frames_rebuilds_the_animation_table() {
        let mut app = app();
        built_texture(&mut app);
        let water = base_texture("water") as usize;
        let frames = |app: &App| table(app)[water * block_texture::ANIMATION_WORDS];
        let before = frames(&app);
        assert!(before > 2, "water has {before} frames");

        let image = layer(&app, "water");
        *app.world_mut()
            .resource_mut::<Assets<Image>>()
            .get_mut(&image)
            .unwrap() = strip(&[2, 3]);
        app.update();
        app.update();

        assert_eq!(frames(&app), 2);
        let block = app.world().resource::<BlockMaterial>();
        let materials = app.world().resource::<Assets<VoxelMaterial>>();
        let table = materials.get(&block.material).unwrap().animations.clone();
        for handle in block.all() {
            assert_eq!(materials.get(handle).unwrap().animations, table);
        }
        // Water's first frame is what item icons show: the strip's first.
        let tiles = &app.world().resource::<BlockTiles>().0;
        let water_palette = base().materials["defaria:water"].palette;
        assert_eq!(tiles[water][..4], water_palette.color(2).to_u8_array());
    }

    /// The chunk materials blend as their names say: opaque, cut out with
    /// alpha to coverage, and blended, twice; the near translucent one
    /// sorts after every other blended mesh.
    #[test]
    fn each_material_draws_its_way() {
        let app = app();
        let block = app.world().resource::<BlockMaterial>();
        let materials = app.world().resource::<Assets<VoxelMaterial>>();
        let modes = block.all().map(|h| materials.get(h).unwrap().alpha_mode);
        assert_eq!(
            modes,
            [
                AlphaMode::Opaque,
                AlphaMode::AlphaToCoverage,
                AlphaMode::Blend,
                AlphaMode::Blend,
            ]
        );
        let biases = block.all().map(|h| materials.get(h).unwrap().depth_bias());
        assert_eq!(biases[..3], [0.0; 3]);
        assert!(biases[3] > 1.0e6, "near faces sort by {}", biases[3]);
    }

    /// Once built, nothing is rebuilt until a layer changes.
    #[test]
    fn the_texture_is_left_alone_while_no_layer_changes() {
        let mut app = app();
        let built = built_texture(&mut app);
        let mut material_events = MaterialEvents::new(&app);
        for _ in 0..10 {
            app.update();
        }
        assert_eq!(material_texture(&app), built);
        assert!(!material_events.modified(&app), "material was rebuilt");
    }

    /// The full-size pixels of base game texture `defaria:<name>` in the
    /// built block texture.
    fn tile(app: &App, texture: &Handle<Image>, name: &str) -> Vec<u8> {
        let image = app
            .world()
            .resource::<Assets<Image>>()
            .get(texture)
            .unwrap();
        let layer = crate::content::base_texture(name) as usize;
        let size = |mip: u32| ((TILE_SIZE >> mip).pow(2) * 4) as usize;
        let per_layer: usize = (0..mip_levels(TILE_SIZE)).map(size).sum();
        let at = layer * per_layer;
        image.data.as_ref().unwrap()[at..at + size(0)].to_vec()
    }

    /// A layer that breaks the rules (here, a gray that isn't one of the
    /// levels) shows the placeholder in its own tiles and leaves the rest
    /// alone, and fixing it builds the real tile again.
    #[test]
    fn a_bad_layer_shows_the_placeholder_until_fixed() {
        let mut app = app();
        let good = built_texture(&mut app);
        let dirt = tile(&app, &good, "soil");
        let stone = layer(&app, "stone");

        paint(&mut app, &stone, [100, 100, 100, 255]);
        app.update();
        app.update();
        let broken = material_texture(&app);
        assert_ne!(broken, good);
        assert_eq!(tile(&app, &broken, "stone"), block_texture::placeholder());
        assert_eq!(tile(&app, &broken, "soil"), dirt);

        paint(&mut app, &stone, [0, 0, 0, 255]);
        app.update();
        app.update();
        let fixed = material_texture(&app);
        assert_ne!(tile(&app, &fixed, "stone"), block_texture::placeholder());
    }

    /// An image that fails to load doesn't hold the texture back: it's built
    /// with the placeholder in the tiles that use the image.
    #[test]
    fn an_image_that_fails_to_load_still_builds() {
        let mut app = app();
        let missing = app
            .world()
            .resource::<AssetServer>()
            .load::<Image>("mods/defaria/images/there-is-no-such-image.png");
        app.world_mut()
            .resource_mut::<BlockMaterial>()
            .layers
            .insert("mods/defaria/images/stone.png".into(), missing);
        let texture = built_texture(&mut app);
        assert_eq!(tile(&app, &texture, "stone"), block_texture::placeholder());
        assert_ne!(tile(&app, &texture, "soil"), block_texture::placeholder());
    }

    /// Sends the failure event Bevy sends when `layer` fails to load. The
    /// system only reads the event's id.
    fn fail(app: &mut App, layer: &Handle<Image>) {
        let path = bevy::asset::AssetPath::from("failed.png");
        app.world_mut()
            .write_message(AssetLoadFailedEvent::<Image> {
                id: layer.id(),
                path: path.clone(),
                error: bevy::asset::AssetLoadError::EmptyPath(path),
            });
    }

    /// A hot reload that fails leaves the old image in `Assets`, but its
    /// tiles show the placeholder until a reload succeeds.
    #[test]
    fn a_failed_reload_shows_the_placeholder_until_it_loads() {
        let mut app = app();
        let good = built_texture(&mut app);
        let stone = layer(&app, "stone");
        let before = tile(&app, &good, "stone");

        fail(&mut app, &stone);
        app.update();
        let broken = material_texture(&app);
        assert_eq!(tile(&app, &broken, "stone"), block_texture::placeholder());
        assert_ne!(tile(&app, &broken, "soil"), block_texture::placeholder());

        // Reloaded successfully: the same image, marked modified.
        paint(&mut app, &stone, [0, 0, 0, 255]);
        app.update();
        app.update();
        let fixed = material_texture(&app);
        assert_ne!(tile(&app, &fixed, "stone"), block_texture::placeholder());
        assert_ne!(tile(&app, &fixed, "stone"), before);
    }

    /// Changes to two layers in the same frame build once, not once each.
    #[test]
    fn two_layers_changing_together_build_once() {
        let mut app = app();
        built_texture(&mut app);
        let mut material_events = MaterialEvents::new(&app);
        for name in ["stone", "dirt"] {
            let layer = layer(&app, name);
            paint(&mut app, &layer, [0, 0, 0, 255]);
        }
        let mut builds = 0;
        for _ in 0..6 {
            app.update();
            builds += material_events.count(&app);
        }
        assert_eq!(builds, 1);
    }

    /// Swaps every layer's handle for one with no image behind it, so a test
    /// can insert the images itself, in its own order.
    fn reserve_layers(app: &mut App) -> Vec<Handle<Image>> {
        let paths: Vec<String> = app
            .world()
            .resource::<BlockMaterial>()
            .layers
            .keys()
            .cloned()
            .collect();
        paths
            .into_iter()
            .map(|path| {
                let handle = app.world().resource::<Assets<Image>>().reserve_handle();
                app.world_mut()
                    .resource_mut::<BlockMaterial>()
                    .layers
                    .insert(path, handle.clone());
                handle
            })
            .collect()
    }

    /// Nothing is built while any layer is missing, and layers arriving on
    /// different frames build once, when the last arrives. Inserted by hand
    /// rather than loaded from disk, so the timing is exact: an image lands
    /// in `Assets` a frame before its `Added` event can be read.
    #[test]
    fn layers_arriving_one_per_frame_build_once_after_the_last() {
        let mut app = app();
        let missing = reserve_layers(&mut app);
        let reserved = material_texture(&app);
        let mut material_events = MaterialEvents::new(&app);
        let black = flat_gray(0);

        let (last, first) = missing.split_last().unwrap();
        for handle in first {
            app.world_mut()
                .resource_mut::<Assets<Image>>()
                .insert(handle, black.clone())
                .unwrap();
            // One frame per layer: each frame reads the previous layer's
            // event while the current layer is already present.
            app.update();
            assert_eq!(
                material_texture(&app),
                reserved,
                "built with a layer missing"
            );
        }
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .insert(last, black)
            .unwrap();
        let mut builds = 0;
        for _ in 0..6 {
            app.update();
            builds += material_events.count(&app);
        }
        assert_eq!(builds, 1);
        assert_ne!(material_texture(&app), reserved);
    }

    /// A load failure that arrives after every other layer still triggers
    /// the one build.
    #[test]
    fn a_failure_arriving_last_still_builds_once() {
        let mut app = app();
        let missing = reserve_layers(&mut app);
        let reserved = material_texture(&app);
        let mut material_events = MaterialEvents::new(&app);
        let (last, first) = missing.split_last().unwrap();
        for handle in first {
            app.world_mut()
                .resource_mut::<Assets<Image>>()
                .insert(handle, flat_gray(0))
                .unwrap();
        }
        app.update();
        app.update();
        assert_eq!(
            material_texture(&app),
            reserved,
            "built with a layer missing"
        );
        fail(&mut app, last);
        let mut builds = 0;
        for _ in 0..6 {
            app.update();
            builds += material_events.count(&app);
        }
        assert_eq!(builds, 1);
        assert_ne!(material_texture(&app), reserved);
    }
}
