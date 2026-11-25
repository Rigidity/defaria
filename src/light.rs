//! Block light and sky light, stored per voxel and spread through the world
//! the way Minecraft does it.
//!
//! Every voxel has four light levels from 0 to [`MAX_LIGHT`]: the sky's, and
//! red, green and blue block light. They're independent: each spreads on its
//! own and the shader mixes them.
//!
//! - **Block light** starts at blocks that give off light (their `light` in
//!   the mod files) and drops by one level per block it travels, around
//!   corners but not through opaque blocks, so a level-15 lamp lights blocks
//!   up to 14 away. Nor does it pass through a side a block covers (its
//!   model's `occludes`), in or out, so a slab lets light in from above and
//!   the sides but not through its bottom. Light sources shine out of every
//!   side.
//! - **Sky light** comes down at full strength into the top of each chunk
//!   with no loaded chunk above it (where [`sky::OutsideSky`] says so; for
//!   now, everywhere), and stays at full strength straight down until it
//!   hits an opaque block or a covered side. From every voxel it reaches it spreads sideways
//!   and into caves like block light, dropping by one per block. The world
//!   has no top, so what's above the loaded chunks is the one thing the
//!   light can't see; `sky` is where that's decided, and
//!   `docs/decisions.md` says why it's done the way it is.
//!
//! Light is kept up to date incrementally by [`propagate`]'s flood fill:
//! when a chunk loads, its own sources, the sky if nothing's above it, and
//! its loaded neighbours' light spread into it (a chunk that arrives
//! [`Prelit`], lit by itself on a loading task, only has its borders to
//! settle), and it takes the outside sky away from the chunk below; when a
//! voxel changes, the light that came through it is taken back and spread
//! again around it. Apart from the sky, a chunk that isn't loaded lets no
//! light through, and a chunk that loads later fills in, so the result
//! doesn't depend on the order chunks load in. Because light reaches less
//! than a chunk's width, a change only ever touches the chunk it's in and
//! the chunks around it, except that sky light falls straight down through
//! any number of open chunks.
//!
//! Chunk meshes bake the light into their vertices (see `voxel`), so a chunk
//! is remeshed when its light, or the light just across its border,
//! changes. Its `ChunkLight` is marked changed for that.
//!
//! Unloading a chunk takes back the light it spread into the chunks around
//! it, as if it had gone dark, and gives the chunk below it the outside sky
//! again. So the light only ever depends on which chunks are loaded and
//! the outside sky. The chunks that lose light that way are at the edge of
//! what's loaded, which isn't drawn.

mod propagate;
pub mod sky;

use bevy::{platform::collections::HashSet, prelude::*};

pub use crate::content::MAX_LIGHT;
use crate::{
    content::Content,
    terrain::{ChunkCoord, ChunkMap, RemeshChunks, VoxelChanged, remesh_changed_chunks},
    voxel::{CHUNK_SIZE, CHUNK_VOLUME, Chunk},
};
pub use propagate::{ChunkStore, LightQueue, LightTable, light_alone};
use sky::Sky;

/// The sky's channel in a [`Light`]; red, green and blue follow it.
pub const SKY: usize = 0;
/// Light channels: the sky, then red, green and blue block light.
pub const CHANNELS: usize = 4;

pub struct LightPlugin;

impl Plugin for LightPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LightTable>()
            .init_resource::<LightQueue>()
            .init_resource::<SkyLight>()
            .init_resource::<Sky>()
            .add_observer(queue_unloaded_chunk)
            .add_systems(
                Update,
                (queue_loaded_chunks, queue_changed_voxels, propagate_light)
                    .chain()
                    .in_set(RemeshChunks)
                    .before(remesh_changed_chunks),
            );
    }
}

/// The colour of sky light at full strength. One colour for the whole world.
#[derive(Resource, Reflect, Debug, Clone, Copy)]
#[reflect(Resource)]
pub struct SkyLight(pub Color);

impl Default for SkyLight {
    fn default() -> Self {
        Self(Color::WHITE)
    }
}

/// The light levels at one voxel: four channels of 4 bits, the sky's in the
/// lowest bits, then red, green and blue.
#[derive(Reflect, Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Light(pub u16);

impl Light {
    pub const DARK: Self = Self(0);

    pub const fn new(sky: u8, [red, green, blue]: [u8; 3]) -> Self {
        Self(sky as u16 | (red as u16) << 4 | (green as u16) << 8 | (blue as u16) << 12)
    }

    /// The level in one channel: [`SKY`], or 1 to 3 for red, green and blue.
    pub const fn get(self, channel: usize) -> u8 {
        ((self.0 >> (channel * 4)) & 0xF) as u8
    }

    pub fn set(&mut self, channel: usize, level: u8) {
        debug_assert!(level <= MAX_LIGHT);
        let shift = channel * 4;
        self.0 = (self.0 & !(0xF << shift)) | (level as u16) << shift;
    }

    #[cfg(test)]
    pub const fn sky(self) -> u8 {
        self.get(SKY)
    }

    /// Red, green and blue block light.
    #[cfg(test)]
    pub const fn block(self) -> [u8; 3] {
        [self.get(1), self.get(2), self.get(3)]
    }
}

/// The light at every voxel of a [`Chunk`], in the same order. Every chunk
/// has one; it starts dark and [`propagate_light`] fills it in.
#[derive(Component, Reflect, Debug, Clone, PartialEq)]
#[reflect(Component)]
pub struct ChunkLight {
    levels: Vec<Light>,
}

impl Default for ChunkLight {
    fn default() -> Self {
        Self {
            levels: vec![Light::DARK; CHUNK_VOLUME],
        }
    }
}

impl ChunkLight {
    /// The light at a chunk-local position.
    pub fn at(&self, pos: IVec3) -> Light {
        self.levels[Chunk::index_of(pos)]
    }

    #[cfg(test)]
    pub fn set_at(&mut self, pos: IVec3, light: Light) {
        self.levels[Chunk::index_of(pos)] = light;
    }

    fn get(&self, index: usize) -> Light {
        self.levels[index]
    }

    fn set(&mut self, index: usize, light: Light) {
        self.levels[index] = light;
    }
}

/// A chunk that was spawned with its own light already worked out by
/// [`light_alone`], so only its borders with the chunks around it are left
/// to settle.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct Prelit;

/// Queues the light for chunks that just loaded: their own sources, the sky
/// if nothing's loaded above them, and their loaded neighbours' light, or
/// for [`Prelit`] chunks only the light across their borders.
fn queue_loaded_chunks(
    chunks: Query<(&ChunkCoord, &Chunk, &ChunkLight, Has<Prelit>), Added<Chunk>>,
    lights: Query<(&Chunk, &ChunkLight)>,
    map: Res<ChunkMap>,
    table: Res<LightTable>,
    sky: Res<Sky>,
    mut queue: ResMut<LightQueue>,
) {
    for (coord, chunk, light, prelit) in &chunks {
        let neighbour = |c| lights.get(map.get(ChunkCoord(c))?).ok();
        if prelit {
            queue.lit_chunk_loaded(coord.0, chunk, light, &table, &**sky, neighbour);
        } else {
            queue.chunk_loaded(coord.0, chunk, &table, &**sky, neighbour);
        }
    }
}

/// Takes away the light an unloaded chunk gave the chunks around it, and
/// gives the chunk below it the outside sky again.
fn queue_unloaded_chunk(
    remove: On<Remove<Chunk>>,
    chunks: Query<(&ChunkCoord, &ChunkLight)>,
    map: Res<ChunkMap>,
    sky: Res<Sky>,
    mut queue: ResMut<LightQueue>,
) {
    if let Ok((coord, light)) = chunks.get(remove.entity) {
        let loaded = |c| map.get(ChunkCoord(c)).is_some_and(|e| e != remove.entity);
        queue.chunk_unloaded(coord.0, light, &**sky, loaded);
    }
}

/// Queues the light around every voxel that changed.
fn queue_changed_voxels(
    mut changes: MessageReader<VoxelChanged>,
    map: Res<ChunkMap>,
    sky: Res<Sky>,
    mut queue: ResMut<LightQueue>,
) {
    for change in changes.read() {
        let (coord, local) = ChunkCoord::split(change.pos);
        let outside_sky = local.y == CHUNK_SIZE as i32 - 1
            && map.get(ChunkCoord(coord.0 + IVec3::Y)).is_none()
            && sky.shines_into(coord.0, local.x, local.z);
        queue.voxel_changed(change.pos, outside_sky);
    }
}

/// The chunks, for the flood fill: change detection is left alone, and the
/// chunks whose light changed are marked afterwards.
struct EcsChunks<'a, 'w, 's> {
    map: &'a ChunkMap,
    chunks: &'a mut Query<'w, 's, (&'static Chunk, &'static mut ChunkLight)>,
}

impl ChunkStore for EcsChunks<'_, '_, '_> {
    fn with_chunk(&mut self, coord: IVec3, f: impl FnOnce(&Chunk, &mut ChunkLight)) -> bool {
        let Some(entity) = self.map.get(ChunkCoord(coord)) else {
            return false;
        };
        let Ok((chunk, mut light)) = self.chunks.get_mut(entity) else {
            return false;
        };
        f(chunk, light.bypass_change_detection());
        true
    }

    fn is_loaded(&self, coord: IVec3) -> bool {
        self.map.get(ChunkCoord(coord)).is_some()
    }
}

/// Runs the flood fill for everything queued, then marks the chunks whose
/// meshes show light that changed.
pub fn propagate_light(
    mut queue: ResMut<LightQueue>,
    table: Res<LightTable>,
    map: Res<ChunkMap>,
    mut chunks: Query<(&'static Chunk, &'static mut ChunkLight)>,
) {
    if queue.is_empty() {
        return;
    }
    let remesh: HashSet<IVec3> = queue.run(
        &mut EcsChunks {
            map: &map,
            chunks: &mut chunks,
        },
        &table,
    );
    for coord in remesh {
        if let Some(entity) = map.get(ChunkCoord(coord))
            && let Ok((_, mut light)) = chunks.get_mut(entity)
        {
            light.set_changed();
        }
    }
}

impl FromWorld for LightTable {
    fn from_world(world: &mut World) -> Self {
        Self::new(world.resource::<Content>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        content::{BlockState, base, block},
        terrain::{
            TerrainPlugin, Voxels,
            testing::{finish_meshing, spawn_patch},
        },
        voxel::CHUNK_SIZE,
        voxel_material::BlockMaterial,
    };
    use bevy::ecs::system::RunSystemOnce;

    /// The terrain and its light, with no rendering.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(base().clone())
            .add_plugins((AssetPlugin::default(), TerrainPlugin, LightPlugin))
            .init_asset::<Mesh>()
            .insert_resource(BlockMaterial {
                material: Handle::default(),
                layers: default(),
            });
        spawn_patch(app.world_mut());
        finish_meshing(&mut app);
        app
    }

    fn set(app: &mut App, pos: IVec3, state: BlockState) {
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| {
                assert!(voxels.set(pos, state));
            })
            .unwrap();
        app.update();
    }

    fn mesh(app: &App, coord: IVec3) -> Option<AssetId<Mesh>> {
        let entity = app.world().resource::<ChunkMap>().get(ChunkCoord(coord))?;
        app.world().get::<Mesh3d>(entity).map(|mesh| mesh.id())
    }

    /// A lamp's light reaching into a chunk its edit didn't touch remeshes
    /// that chunk too; chunks out of reach keep their meshes.
    #[test]
    fn light_changes_remesh_the_chunks_they_reach() {
        let mut app = app();
        // High in the air, where the terrain doesn't reach: a stone in the
        // middle of one chunk, and a lamp 3 voxels into the next.
        let (lit, lamp_chunk) = (IVec3::new(1, 3, 2), IVec3::new(2, 3, 2));
        let origin = |coord: IVec3| coord * CHUNK_SIZE as i32;
        set(&mut app, origin(lit) + IVec3::splat(8), block("stone"));
        let before = mesh(&app, lit).expect("the stone has a mesh");
        let far = IVec3::new(7, 0, 7);
        let far_before = mesh(&app, far);
        assert!(far_before.is_some());

        let lamp = origin(lamp_chunk) + IVec3::new(3, 8, 8);
        set(&mut app, lamp, block("red_lamp"));
        let entity = app
            .world()
            .resource::<ChunkMap>()
            .get(ChunkCoord(lit))
            .unwrap();
        let light = app.world().get::<ChunkLight>(entity).unwrap();
        assert!(
            light.at(IVec3::new(9, 8, 8)).block()[0] > 0,
            "the light got there"
        );
        assert_ne!(mesh(&app, lit), Some(before), "lit chunk wasn't remeshed");
        assert_eq!(mesh(&app, far), far_before, "far chunk was remeshed");
    }

    #[test]
    fn light_packs_four_channels() {
        let mut light = Light::new(15, [1, 2, 3]);
        assert_eq!((light.sky(), light.block()), (15, [1, 2, 3]));
        light.set(2, 9);
        assert_eq!((light.sky(), light.block()), (15, [1, 9, 3]));
        light.set(SKY, 0);
        assert_eq!(light, Light::new(0, [1, 9, 3]));
    }
}
