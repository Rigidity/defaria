//! The light at every voxel, which both sides keep for each chunk: a
//! [`Light`] (the sky's level, and red, green and blue block light) in a
//! chunk's [`ChunkLight`]. The server works it out (how it spreads is
//! `defaria_server::light`) and sends it with the chunk; the client draws
//! by it.

use bevy::prelude::*;

pub use crate::content::MAX_LIGHT;
use crate::world::{Chunk, ChunkArray};

/// The sky's channel in a [`Light`]; red, green and blue follow it.
pub const SKY: usize = 0;
/// Light channels: the sky, then red, green and blue block light.
pub const CHANNELS: usize = 4;

/// Where changed chunks are relit, in
/// [`RemeshChunks`](crate::world::RemeshChunks); drawing them comes after.
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
#[derive(
    Reflect, Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
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
/// has one; it starts dark and the server's light fills it in.
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

    /// The light at a voxel, by its [`Chunk::index_of`].
    pub fn get(&self, index: usize) -> Light {
        self.levels.get(index)
    }

    /// Sets the light at a voxel, by its [`Chunk::index_of`].
    pub fn set(&mut self, index: usize, light: Light) {
        self.levels.set(index, light);
    }
}

/// The levels fluid with this `filter` takes off light when it holds
/// `amount` of `room`: the filter in proportion to how full it is,
/// rounded. Meshing dims the faces drawn in or under part-full fluid by
/// this, as light dims what the voxel passes on.
pub fn part_full_dim(filter: u8, amount: u16, room: u16) -> u8 {
    if room == 0 {
        return 0;
    }
    let (filter, amount, room) = (
        u32::from(filter),
        u32::from(amount.min(room)),
        u32::from(room),
    );
    ((2 * filter * amount + room) / (2 * room)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

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
