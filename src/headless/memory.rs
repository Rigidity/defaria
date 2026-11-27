//! The `memory` and `lookups` commands: how much the loaded chunks hold,
//! and what finding voxels by world position costs.

use std::{
    collections::HashSet,
    fmt,
    hint::black_box,
    time::{Duration, Instant},
};

use bevy::prelude::*;

use crate::{
    content::{BlockState, FULL_FLUID},
    light::ChunkLight,
    world::{CHUNK_SIZE, Chunk},
    world::{ChunkCoord, ChunkMap, Voxels},
};

const MIB: f64 = 1024.0 * 1024.0;

/// The most voxels a `lookups` box may hold: 16 Mi, a fraction of a
/// second a way. More is a mistake (it would take minutes, or count past
/// what fits).
pub const MAX_LOOKUP_VOXELS: u64 = 1 << 24;

/// What the loaded chunks hold, counted from their data structures (not
/// the process's memory), so it's the same every run of the same world.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChunkMemory {
    pub chunks: usize,
    /// Chunks that are air throughout.
    pub air: usize,
    /// Chunks that are one other block state throughout.
    pub solid: usize,
    /// Chunks whose light is one level throughout.
    pub even_light: usize,
    /// Bytes of block states.
    pub blocks: usize,
    /// Bytes of light levels.
    pub light: usize,
    /// Of those bytes of blocks and light, the ones held for chunks that
    /// are one value throughout (and so could hold none; see
    /// [`crate::world::ChunkArray::compact`]).
    pub uniform_dense: usize,
    /// Chunks whose blocks or light are one value throughout but still
    /// kept a value per voxel: 0 once compaction has caught up.
    pub uniform_dense_chunks: usize,
    /// Voxels holding less than a full block of fluid, which each chunk
    /// keeps in a map.
    pub partial_fluid: usize,
    /// Bytes of those maps' entries, leaving out their spare capacity.
    pub fluid: usize,
    /// Meshes on chunks and their parts.
    pub meshes: usize,
    /// Bytes of their vertices and indices kept in the main world (a
    /// copy of what's on the GPU), for the meshes that still have it.
    pub mesh_bytes: usize,
}

impl ChunkMemory {
    pub fn measure(world: &mut World) -> Self {
        let mut memory = Self::default();
        let mut chunk_entities = HashSet::new();
        let mut chunks = world.query::<(Entity, &Chunk, Option<&ChunkLight>)>();
        for (entity, chunk, light) in chunks.iter(world) {
            chunk_entities.insert(entity);
            memory.add_chunk(chunk, light);
        }
        let mut meshes = world.query::<(Entity, &Mesh3d, Option<&ChildOf>)>();
        let assets = world.resource::<Assets<Mesh>>();
        for (entity, mesh, parent) in meshes.iter(world) {
            let on_chunk = chunk_entities.contains(&entity)
                || parent.is_some_and(|parent| chunk_entities.contains(&parent.parent()));
            if !on_chunk {
                continue;
            }
            memory.meshes += 1;
            memory.mesh_bytes += assets.get(&mesh.0).map_or(0, mesh_bytes);
        }
        memory
    }

    fn add_chunk(&mut self, chunk: &Chunk, light: Option<&ChunkLight>) {
        self.chunks += 1;
        let voxels = chunk.voxels();
        self.blocks += voxels.heap_bytes();
        match voxels.uniform() {
            Some(BlockState::AIR) => self.air += 1,
            Some(_) => self.solid += 1,
            None => {}
        }
        let mut uniform_dense = 0;
        if voxels.uniform().is_some() {
            uniform_dense += voxels.heap_bytes();
        }
        let fluid = chunk.fluid_layer();
        self.partial_fluid += chunk
            .fluids()
            .filter(|(_, fluid)| fluid.amount < FULL_FLUID)
            .count();
        self.fluid += fluid.heap_bytes();
        if fluid.uniform().is_some() {
            uniform_dense += fluid.heap_bytes();
        }
        if let Some(light) = light {
            let levels = light.levels();
            self.light += levels.heap_bytes();
            if levels.uniform().is_some() {
                self.even_light += 1;
                uniform_dense += levels.heap_bytes();
            }
        }
        self.uniform_dense += uniform_dense;
        if uniform_dense > 0 {
            self.uniform_dense_chunks += 1;
        }
    }
}

impl fmt::Display for ChunkMemory {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mib = |bytes: usize| bytes as f64 / MIB;
        let storage = self.blocks + self.light + self.fluid;
        write!(
            f,
            "memory: {} chunks ({} all air, {} one other block throughout, {} one light \
             throughout); blocks {:.1} MiB, light {:.1} MiB, fluid {:.2} MiB ({} partial \
             voxels): {:.1} MiB of chunk storage, {} KiB of it in {} chunks of one value \
             kept dense; {} meshes holding {:.1} MiB",
            self.chunks,
            self.air,
            self.solid,
            self.even_light,
            mib(self.blocks),
            mib(self.light),
            mib(self.fluid),
            self.partial_fluid,
            mib(storage),
            self.uniform_dense / 1024,
            self.uniform_dense_chunks,
            self.meshes,
            mib(self.mesh_bytes),
        )
    }
}

/// The bytes of vertices and indices a mesh keeps in the main world: 0 once
/// they've gone to the render world only.
fn mesh_bytes(mesh: &Mesh) -> usize {
    if mesh.try_attributes().is_err() {
        return 0;
    }
    mesh.get_vertex_buffer_size() + mesh.get_index_buffer_bytes().map_or(0, <[u8]>::len)
}

/// What reading every voxel in a box cost, two ways.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookupCost {
    /// Voxels in the box.
    pub voxels: u64,
    /// Of them, the ones in loaded chunks.
    pub loaded: u64,
    /// Each through [`Voxels::get`]: a hash and a query per voxel.
    pub by_position: Duration,
    /// A chunk at a time: a hash and a query per chunk, then its array.
    pub by_chunk: Duration,
}

impl fmt::Display for LookupCost {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let each = |time: Duration| time.as_nanos() as f64 / self.voxels.max(1) as f64;
        write!(
            f,
            "lookups: {} voxels ({} loaded): by position {:.2} ms ({:.1} ns each), a chunk \
             at a time {:.2} ms ({:.1} ns each)",
            self.voxels,
            self.loaded,
            self.by_position.as_secs_f64() * 1000.0,
            each(self.by_position),
            self.by_chunk.as_secs_f64() * 1000.0,
            each(self.by_chunk),
        )
    }
}

/// Reads every voxel between two corners (both included), first one at a
/// time by world position as most systems do, then a chunk at a time.
/// A box of more than [`MAX_LOOKUP_VOXELS`] is an error.
pub fn measure_lookups(
    In((min, max)): In<(IVec3, IVec3)>,
    mut params: ParamSet<(Voxels, (Res<ChunkMap>, Query<&Chunk>))>,
) -> Result<LookupCost, String> {
    let (min, max) = (min.min(max), min.max(max));
    let count = (max.as_i64vec3() - min.as_i64vec3() + bevy::math::I64Vec3::ONE)
        .as_u64vec3()
        .element_product();
    if count > MAX_LOOKUP_VOXELS {
        return Err(format!(
            "lookups: the box holds {count} voxels, over the most, {MAX_LOOKUP_VOXELS}"
        ));
    }

    let start = Instant::now();
    let mut loaded = 0;
    let mut sum = 0u64;
    let voxels = params.p0();
    for x in min.x..=max.x {
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                if let Some(state) = voxels.get(black_box(IVec3::new(x, y, z))) {
                    loaded += 1;
                    sum += state.0 as u64;
                }
            }
        }
    }
    let by_position = start.elapsed();

    let (map, chunks) = params.p1();
    let start = Instant::now();
    let mut chunk_sum = 0u64;
    let (low, _) = ChunkCoord::split(min);
    let (high, _) = ChunkCoord::split(max);
    for cx in low.0.x..=high.0.x {
        for cy in low.0.y..=high.0.y {
            for cz in low.0.z..=high.0.z {
                let coord = ChunkCoord(IVec3::new(cx, cy, cz));
                let Some(chunk) = map.get(coord).and_then(|entity| chunks.get(entity).ok()) else {
                    continue;
                };
                let origin = coord.0 * CHUNK_SIZE as i32;
                let from = (min - origin).max(IVec3::ZERO);
                let to = (max - origin).min(IVec3::splat(CHUNK_SIZE as i32 - 1));
                for x in from.x..=to.x {
                    for y in from.y..=to.y {
                        for z in from.z..=to.z {
                            chunk_sum += chunk.at(black_box(IVec3::new(x, y, z))).0 as u64;
                        }
                    }
                }
            }
        }
    }
    let by_chunk = start.elapsed();
    debug_assert_eq!(sum, chunk_sum);
    black_box((sum, chunk_sum));
    Ok(LookupCost {
        voxels: count,
        loaded,
        by_position,
        by_chunk,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light::Light;

    fn world_with(chunks: Vec<(IVec3, Chunk)>) -> World {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.insert_resource(crate::content::base().clone());
        world.init_resource::<ChunkMap>();
        world.init_resource::<Messages<crate::world::VoxelChanged>>();
        world.init_resource::<Messages<crate::world::FluidChanged>>();
        world.init_resource::<Messages<crate::world::PropertiesUpdated>>();
        for (coord, chunk) in chunks {
            let entity = world.spawn((chunk, ChunkLight::default())).id();
            world
                .resource_mut::<ChunkMap>()
                .insert(ChunkCoord(coord), entity);
        }
        world
    }

    /// A mesh handed to the render world keeps no vertices in the main
    /// world, and counts as none; one that kept its copy counts it.
    #[test]
    fn extracted_meshes_count_no_bytes() {
        use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology};
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 3]);
        assert_eq!(mesh_bytes(&mesh), 3 * 12);
        mesh.take_gpu_data().unwrap();
        assert_eq!(mesh_bytes(&mesh), 0);
    }

    #[test]
    fn memory_counts_storage_and_uniform_chunks() {
        let stone = BlockState(1);
        let mut mixed = Chunk::filled(BlockState::AIR);
        mixed.set(0, 0, 0, stone);
        // Dug back out to air, but not compacted yet.
        let mut dug = mixed.clone();
        dug.set(0, 0, 0, BlockState::AIR);
        let mut world = world_with(vec![
            (IVec3::ZERO, Chunk::filled(BlockState::AIR)),
            (IVec3::X, Chunk::filled(stone)),
            (IVec3::Y, mixed),
            (IVec3::Z, dug),
        ]);
        let lit = world
            .query_filtered::<Entity, With<Chunk>>()
            .iter(&world)
            .next()
            .unwrap();
        world
            .get_mut::<ChunkLight>(lit)
            .unwrap()
            .set_at(IVec3::ONE, Light(1));

        let memory = ChunkMemory::measure(&mut world);
        assert_eq!(memory.chunks, 4);
        assert_eq!(memory.air, 2);
        assert_eq!(memory.solid, 1);
        assert_eq!(memory.even_light, 3);
        // Only the mixed and dug chunks and the lit one's light hold voxels.
        assert_eq!(memory.blocks, 2 * 4096 * 2);
        assert_eq!(memory.light, 4096 * 2);
        assert_eq!(memory.uniform_dense, 4096 * 2);
        assert_eq!(memory.uniform_dense_chunks, 1);
        assert_eq!(memory.partial_fluid, 0);
        assert_eq!(memory.meshes, 0);
        let line = memory.to_string();
        assert!(
            line.starts_with("memory: 4 chunks (2 all air, 1 one other block"),
            "{line}"
        );
    }

    #[test]
    fn lookups_read_every_loaded_voxel_both_ways() {
        let mut world = world_with(vec![
            (IVec3::ZERO, Chunk::filled(BlockState(2))),
            (IVec3::X, Chunk::filled(BlockState(3))),
        ]);
        // Half in the two loaded chunks, half in unloaded ones above.
        let cost = world
            .run_system_cached_with(
                measure_lookups,
                (IVec3::new(8, 0, 0), IVec3::new(23, 31, 15)),
            )
            .unwrap()
            .unwrap();
        assert_eq!(cost.voxels, 16 * 32 * 16);
        assert_eq!(cost.loaded, 16 * 16 * 16);
        assert!(
            cost.to_string()
                .starts_with("lookups: 8192 voxels (4096 loaded)")
        );
    }

    /// A box too big to read in a sensible time (and whose voxels don't
    /// fit in a `u32`) is refused rather than read.
    #[test]
    fn lookups_refuse_a_huge_box() {
        let mut world = world_with(vec![(IVec3::ZERO, Chunk::filled(BlockState(2)))]);
        let error = world
            .run_system_cached_with(measure_lookups, (IVec3::splat(-1000), IVec3::splat(1000)))
            .unwrap()
            .unwrap_err();
        assert!(error.contains("8012006001 voxels"), "{error}");
        let side = 1 << 8;
        assert!(
            world
                .run_system_cached_with(measure_lookups, (IVec3::ZERO, IVec3::splat(side - 1)))
                .unwrap()
                .is_ok(),
            "exactly the most"
        );
        assert!(
            world
                .run_system_cached_with(measure_lookups, (IVec3::ZERO, IVec3::splat(side)))
                .unwrap()
                .is_err()
        );
    }
}
