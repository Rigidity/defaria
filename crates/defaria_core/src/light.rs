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
//!   hits an opaque block or a covered side. From every voxel it reaches it
//!   spreads sideways and into caves like block light, dropping by one per
//!   block. The world
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
//! Chunk meshes bake the light into their vertices (see `meshing`), so a chunk
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
    content::{BlockState, Content},
    world::{
        Chunk, ChunkCoord, ChunkMap, FluidChanged, PropertiesUpdated, VoxelChanged, VoxelFluid,
    },
    world::{ChunkArray, RemeshChunks},
};
pub use propagate::{ChunkStore, LightQueue, LightTable, light_alone, part_full_dim};
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
                    .in_set(LightSystems)
                    .in_set(RemeshChunks),
            );
    }
}

/// Where changed chunks are relit, in [`RemeshChunks`]; drawing them
/// comes after.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct LightSystems;

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
#[derive(Reflect, Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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

    #[cfg(any(test, feature = "test-support"))]
    pub const fn sky(self) -> u8 {
        self.get(SKY)
    }

    /// Red, green and blue block light.
    #[cfg(any(test, feature = "test-support"))]
    pub const fn block(self) -> [u8; 3] {
        [self.get(1), self.get(2), self.get(3)]
    }
}

/// The light at every voxel of a [`Chunk`], in the same order. Every chunk
/// has one; it starts dark and [`propagate_light`] fills it in.
#[derive(Component, Reflect, Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[reflect(Component)]
pub struct ChunkLight {
    levels: ChunkArray<Light>,
}

impl Default for ChunkLight {
    fn default() -> Self {
        Self {
            levels: ChunkArray::Uniform(Light::DARK),
        }
    }
}

impl ChunkLight {
    /// The light at a chunk-local position.
    pub fn at(&self, pos: IVec3) -> Light {
        self.levels.get(Chunk::index_of(pos))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_at(&mut self, pos: IVec3, light: Light) {
        self.levels.set(Chunk::index_of(pos), light);
    }

    /// The light at every voxel, in [`Chunk::index_of`] order.
    pub fn levels(&self) -> &ChunkArray<Light> {
        &self.levels
    }

    /// Drops the levels if they're all the same (see [`ChunkArray`]).
    pub fn compact(&mut self) -> bool {
        self.levels.compact()
    }

    fn get(&self, index: usize) -> Light {
        self.levels.get(index)
    }

    fn set(&mut self, index: usize, light: Light) {
        self.levels.set(index, light);
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

/// Queues the light around every voxel that changed, and every one whose
/// properties or fluid changed in a way light sees.
fn queue_changed_voxels(
    mut changes: MessageReader<VoxelChanged>,
    mut updates: MessageReader<PropertiesUpdated>,
    mut fluid_changes: MessageReader<FluidChanged>,
    table: Res<LightTable>,
    map: Res<ChunkMap>,
    chunks: Query<&Chunk>,
    mut queue: ResMut<LightQueue>,
) {
    // A property update keeps the block, so its `fluid_room`, which is
    // the block's and the same in every state: whether fluid fills the
    // voxel can't change with it. If room ever varies by state, compare
    // `same_fluid_light` for the voxel's fluid here too.
    let updated = updates
        .read()
        .filter(|update| !table.same_light(update.before, update.after))
        .map(|update| update.pos);
    // Only another fluid matters to light, or the fluid filling its voxel
    // or no longer filling it, not every unit more or less of one.
    let block = |pos: IVec3| {
        let (coord, local) = ChunkCoord::split(pos);
        map.get(coord)
            .and_then(|entity| chunks.get(entity).ok())
            .map_or(BlockState::AIR, |chunk| chunk.at(local))
    };
    let amount = |fluid: Option<VoxelFluid>| fluid.map(|fluid| (fluid.kind, fluid.amount));
    let fluids = fluid_changes
        .read()
        .filter(|change| {
            !table.same_fluid_light(
                block(change.pos),
                amount(change.before),
                amount(change.after),
            )
        })
        .map(|change| change.pos);
    for pos in changes
        .read()
        .map(|change| change.pos)
        .chain(updated)
        .chain(fluids)
    {
        queue.voxel_changed(pos);
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
    sky: Res<Sky>,
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
        &**sky,
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
        testing::{ChunkMapPlugin, patch_app, test_content},
        world::{CHUNK_SIZE, Voxels},
    };
    use bevy::ecs::system::RunSystemOnce;

    /// The terrain and its light, with no rendering.
    fn app() -> App {
        patch_app(base().clone())
    }

    fn set(app: &mut App, pos: IVec3, state: BlockState) {
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| {
                assert!(voxels.set(pos, state));
            })
            .unwrap();
        app.update();
    }

    /// When the chunk's light last changed, which is what redraws it.
    fn relit(app: &App, coord: IVec3) -> Option<bevy::ecs::change_detection::Tick> {
        let entity = app.world().resource::<ChunkMap>().get(ChunkCoord(coord))?;
        app.world()
            .entity(entity)
            .get_change_ticks::<ChunkLight>()
            .map(|ticks| ticks.changed)
    }

    /// Lava arriving in a voxel's fluid, as flow moves it with no block
    /// changing, lights what's around it, and its light goes when it
    /// drains away.
    #[test]
    fn fluid_coming_and_going_relights() {
        let mut app = app();
        // High in the air, where the terrain doesn't reach.
        let pos = IVec3::new(40, 56, 40);
        let set_fluid = |app: &mut App, fluid: Option<VoxelFluid>| {
            app.world_mut()
                .run_system_once(move |mut voxels: Voxels| {
                    assert!(voxels.flow_fluid(pos, fluid));
                })
                .unwrap();
            app.update();
            let (coord, local) = ChunkCoord::split(pos + IVec3::X);
            let entity = app.world().resource::<ChunkMap>().get(coord).unwrap();
            app.world()
                .get::<ChunkLight>(entity)
                .unwrap()
                .at(local)
                .block()[0]
        };
        let lava = base().state(block("lava")).light[0];
        assert_eq!(
            set_fluid(&mut app, VoxelFluid::of(block("lava"), 300)),
            lava - 1
        );
        assert_eq!(set_fluid(&mut app, None), 0);
    }

    /// Only a change of fluid that light sees queues light work: another
    /// fluid, water coming or going, since it dims light (its
    /// `light_filter`), water filling its voxel or no longer filling it,
    /// or part-full water crossing a step of how much it dims the light
    /// it passes on; not every unit more or less of one.
    #[test]
    fn fluid_changes_queue_light_only_for_another_fluid() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(LightTable::new(base()))
            .init_resource::<ChunkMap>()
            .init_resource::<Sky>()
            .init_resource::<LightQueue>()
            .add_message::<VoxelChanged>()
            .add_message::<PropertiesUpdated>()
            .add_message::<FluidChanged>()
            .add_systems(Update, queue_changed_voxels);
        let mut send = |before: Option<(&str, u16)>, after: Option<(&str, u16)>| {
            let fluid = |f: Option<(&str, u16)>| f.and_then(|(id, n)| VoxelFluid::of(block(id), n));
            app.world_mut().write_message(FluidChanged {
                pos: IVec3::ZERO,
                by_flow: true,
                before: fluid(before),
                after: fluid(after),
            });
            app.update();
            let queued = !app.world().resource::<LightQueue>().is_empty();
            app.world_mut().insert_resource(LightQueue::default());
            queued
        };
        assert!(send(None, Some(("lava", 300))));
        assert!(send(Some(("lava", 300)), Some(("water", 300))));
        assert!(send(Some(("lava", 300)), None));
        assert!(!send(Some(("lava", 300)), Some(("lava", 900))));
        assert!(
            !send(Some(("water", 300)), Some(("water", 600))),
            "both dim one"
        );
        assert!(
            !send(Some(("water", 100)), Some(("water", 200))),
            "both films"
        );
        assert!(
            send(Some(("water", 300)), Some(("water", 900))),
            "dims one more"
        );
        assert!(send(None, Some(("water", 1000))), "water dims light");
        assert!(send(Some(("water", 900)), Some(("water", 1000))), "filling");
        assert!(
            send(Some(("water", 1000)), Some(("water", 900))),
            "no longer"
        );
        assert!(
            !send(Some(("lava", 300)), Some(("lava", 1000))),
            "lava doesn't dim"
        );
    }

    /// A lamp's light reaching into a chunk its edit didn't touch changes
    /// that chunk's light too (so it's redrawn); chunks out of reach keep
    /// theirs untouched.
    #[test]
    fn light_changes_reach_the_chunks_they_light() {
        let mut app = app();
        // High in the air, where the terrain doesn't reach: a stone in the
        // middle of one chunk, and a lamp 3 voxels into the next.
        let (lit, lamp_chunk) = (IVec3::new(1, 3, 2), IVec3::new(2, 3, 2));
        let origin = |coord: IVec3| coord * CHUNK_SIZE as i32;
        set(&mut app, origin(lit) + IVec3::splat(8), block("stone"));
        let before = relit(&app, lit).expect("the stone's chunk is lit");
        let far = IVec3::new(7, 0, 7);
        let far_before = relit(&app, far);
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
        assert_ne!(
            relit(&app, lit),
            Some(before),
            "lit chunk's light unchanged"
        );
        assert_eq!(relit(&app, far), far_before, "far chunk's light changed");
    }

    /// The base game and a lamp lit in one state only.
    fn with_switched_lamp() -> Content {
        test_content(&[(
            "blocks/switched.json5",
            r#"{ properties: { lit: ["false", "true"], tint: ["a", "b"] },
                 model: "defaria:cube", textures: { all: "defaria:red_lamp" },
                 variants: [{ when: { lit: "true" }, light: [15, 15, 15] }] }"#,
        )])
    }

    /// A lamp turned on and off by updating its properties, as the engine
    /// or a script does, lights and darkens the voxels around it.
    #[test]
    fn property_updates_relight() {
        let content = with_switched_lamp();
        let state = |lit| {
            content
                .block_state("test:switched", &[("lit", lit)])
                .unwrap()
        };
        let mut app = patch_app(content.clone());

        // High in the air, inside a chunk, where the terrain doesn't reach.
        let lamp = IVec3::new(1, 3, 2) * CHUNK_SIZE as i32 + IVec3::splat(8);
        set(&mut app, lamp, state("false"));
        let beside = |app: &mut App| {
            let (coord, local) = ChunkCoord::split(lamp + IVec3::X);
            let entity = app.world().resource::<ChunkMap>().get(coord).unwrap();
            app.world()
                .get::<ChunkLight>(entity)
                .unwrap()
                .at(local)
                .block()
        };
        let update = |app: &mut App, to| {
            app.world_mut()
                .run_system_once(move |mut voxels: Voxels| assert!(voxels.update(lamp, to)))
                .unwrap();
            app.update();
        };
        assert_eq!(beside(&mut app), [0; 3]);
        update(&mut app, state("true"));
        assert_eq!(beside(&mut app), [14; 3]);
        update(&mut app, state("false"));
        assert_eq!(beside(&mut app), [0; 3]);
    }

    /// Only updates that change how a voxel lets light through or what it
    /// gives off queue any light work.
    #[test]
    fn updates_light_doesnt_see_queue_nothing() {
        let content = with_switched_lamp();
        let state = |lit, tint| {
            content
                .block_state("test:switched", &[("lit", lit), ("tint", tint)])
                .unwrap()
        };
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(LightTable::new(&content))
            .add_plugins(ChunkMapPlugin)
            .init_resource::<Sky>()
            .init_resource::<LightQueue>()
            .add_message::<VoxelChanged>()
            .add_message::<PropertiesUpdated>()
            .add_message::<FluidChanged>()
            .add_systems(Update, queue_changed_voxels);
        let mut send = |before, after| {
            app.world_mut().write_message(PropertiesUpdated {
                pos: IVec3::ZERO,
                before,
                after,
            });
            app.update();
            let queued = !app.world().resource::<LightQueue>().is_empty();
            app.world_mut().insert_resource(LightQueue::default());
            queued
        };
        assert!(!send(state("false", "a"), state("false", "b")));
        assert!(!send(state("true", "a"), state("true", "b")));
        assert!(send(state("false", "a"), state("true", "a")));
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
