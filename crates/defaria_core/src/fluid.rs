//! Fluid in the world, as both sides see it: where its surface is, for
//! swimming and the view from inside it ([`FluidView`]), and an amount of
//! one fluid ([`FluidAmount`]). A fluid is a block with `fluid` set (see
//! `docs/modding.md`), and a voxel holds an amount of one beside its block
//! (see [`Chunk`]). How it flows is the server's (`defaria_server::fluid`,
//! whose module docs say how).
//!
//! With the eyes inside a fluid, the fog is the fluid's color and short,
//! as the fluid's `view` says (see `docs/modding.md`).

use bevy::{ecs::system::SystemParam, prelude::*};

use crate::{
    content::{BlockState, Content, Fluid},
    world::{Chunk, ChunkCoord, ChunkMap, FluidTop, VoxelFluid, fluid_surface},
};

/// A cell gives each lower side this fraction of the difference between
/// them, at most: one over this. Eight is the most that keeps fluid moving
/// only downhill when a cell gives to four sides at once.
pub const SHARE: u16 = 8;

/// The smallest `ripple` a fluid can have: a unit flowing between two cells
/// a unit apart would only swap them.
pub const MIN_RIPPLE: u16 = 2;

/// Some amount of one fluid, in [`FULL_FLUID`](crate::content::FULL_FLUID)
/// units (thousandths of a block, as the world counts fluid). The fluid is
/// its block's id, as a machine's [`Tank`](crate::machine::Tank) names it.
/// What anything holding "some of one fluid" (a pipe, a bucket) keeps.
#[derive(Reflect, Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FluidAmount {
    pub fluid: String,
    pub amount: u64,
}

/// The fluid in the loaded world, to look at: where its surface is, for
/// swimming and the view from inside it, and (on the server) as cells for
/// flow.
#[derive(SystemParam)]
pub struct FluidView<'w, 's> {
    pub map: Res<'w, ChunkMap>,
    pub chunks: Query<'w, 's, &'static Chunk>,
    pub content: Res<'w, Content>,
}

impl FluidView<'_, '_> {
    /// The block and fluid in the voxel at `pos`, if it's loaded.
    pub fn get_with_fluid(&self, pos: IVec3) -> Option<(BlockState, Option<VoxelFluid>)> {
        let (coord, local) = ChunkCoord::split(pos);
        let chunk = self.chunks.get(self.map.get(coord)?).ok()?;
        Some((chunk.at(local), chunk.fluid_at(local)))
    }

    /// The fluid in the voxel at `pos` and how high it's drawn there (see
    /// [`fluid_surface`]), if it's loaded and holds any.
    pub fn voxel(&self, pos: IVec3) -> Option<(BlockState, f32)> {
        let (coord, local) = ChunkCoord::split(pos);
        let chunk = self.chunks.get(self.map.get(coord)?).ok()?;
        let fluid = chunk.fluid_at(local)?;
        let room = self.content.state(chunk.at(local)).fluid_room;
        Some((fluid.kind, fluid_surface(chunk.fluid_step_at(local), room)))
    }

    /// The fluid in the voxel at `pos`, and its surface, as it's drawn
    /// (see [`FluidTop`]).
    pub fn surface(&self, pos: IVec3) -> Option<(BlockState, FluidTop)> {
        let (kind, _) = self.voxel(pos)?;
        Some((kind, FluidTop::of(kind, pos, |at| self.voxel(at))))
    }

    /// The fluid `point` is in, below its surface.
    pub fn fluid_at(&self, point: Vec3) -> Option<BlockState> {
        let pos = point.floor().as_ivec3();
        let (kind, top) = self.surface(pos)?;
        let height = top.at(point.xz() - pos.xz().as_vec2());
        (point.y < pos.y as f32 + height).then_some(kind)
    }

    /// How a fluid flows and feels.
    pub fn fluid(&self, kind: BlockState) -> Option<&Fluid> {
        self.content.state(kind).fluid.as_ref()
    }

    /// How much of the height from `bottom` to `top`, up the column of
    /// voxels at `(x, z)`, is under fluid, as a fraction from 0 to 1, and the
    /// fluid most of that is. `None` if none of it is.
    pub fn immersion(&self, x: f64, z: f64, bottom: f64, top: f64) -> Option<(BlockState, f64)> {
        let mut depths: Vec<(BlockState, f64)> = Vec::new();
        for y in bottom.floor() as i32..=top.floor() as i32 {
            let pos = IVec3::new(x.floor() as i32, y, z.floor() as i32);
            let Some((kind, surface)) = self.surface(pos) else {
                continue;
            };
            let height = surface.at(Vec2::new((x - x.floor()) as f32, (z - z.floor()) as f32));
            let under = (y as f64 + height as f64).min(top) - (y as f64).max(bottom);
            if under <= 0.0 {
                continue;
            }
            match depths.iter_mut().find(|(k, _)| *k == kind) {
                Some((_, depth)) => *depth += under,
                None => depths.push((kind, under)),
            }
        }
        let total: f64 = depths.iter().map(|(_, depth)| depth).sum();
        let (kind, _) = depths.into_iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
        Some((kind, (total / (top - bottom)).min(1.0)))
    }
}
