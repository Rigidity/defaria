//! A value per voxel of a chunk, kept as one value while every voxel has
//! the same: what [`Chunk`]s keep their block states in and
//! [`ChunkLight`] its light.
//!
//! Most loaded chunks are one value throughout: open sky is all air in
//! full sky light, deep rock all stone in the dark. Measured at the
//! default radius (#71), 94% of the chunks' blocks and light were in such
//! chunks, 315 of 336 MiB. A uniform array holds no voxels at all; the
//! first voxel set to something else makes it dense, and [`compact`]
//! makes a dense one uniform again, once a frame for the chunks that
//! changed, so a chunk that's dug out to air or relit evenly gives its
//! memory back.

use std::ops::Index;

use bevy::prelude::*;

use super::{CHUNK_VOLUME, Chunk};
use crate::light::ChunkLight;

/// [`CHUNK_VOLUME`] values in [`Chunk::index_of`] order.
#[derive(Reflect, Debug, Clone)]
pub enum ChunkArray<T> {
    /// Every voxel has this value; nothing else is kept.
    Uniform(T),
    /// A value per voxel. It may still be one value throughout until
    /// [`ChunkArray::compact`] notices.
    Dense(Vec<T>),
}

impl<T: Copy + PartialEq> ChunkArray<T> {
    /// The values in `values`, which must have [`CHUNK_VOLUME`] of them,
    /// kept uniform if they're all the same.
    pub fn from_vec(values: Vec<T>) -> Self {
        assert_eq!(values.len(), CHUNK_VOLUME, "a chunk's worth of values");
        let mut array = Self::Dense(values);
        array.compact();
        array
    }

    /// The value at an index from [`Chunk::index_of`].
    #[inline]
    pub fn get(&self, index: usize) -> T {
        match self {
            Self::Uniform(value) => {
                assert!(index < CHUNK_VOLUME, "voxel {index} is outside the chunk");
                *value
            }
            Self::Dense(values) => values[index],
        }
    }

    /// Sets the value at an index, making the array dense if it was
    /// uniform with another value.
    #[inline]
    pub fn set(&mut self, index: usize, value: T) {
        match self {
            Self::Uniform(current) => {
                assert!(index < CHUNK_VOLUME, "voxel {index} is outside the chunk");
                if *current != value {
                    let mut values = vec![*current; CHUNK_VOLUME];
                    values[index] = value;
                    *self = Self::Dense(values);
                }
            }
            Self::Dense(values) => values[index] = value,
        }
    }

    /// The one value every voxel has, if they all have the same.
    pub fn uniform(&self) -> Option<T> {
        match self {
            Self::Uniform(value) => Some(*value),
            Self::Dense(values) => values[1..]
                .iter()
                .all(|value| *value == values[0])
                .then_some(values[0]),
        }
    }

    /// Makes a dense array that's one value throughout uniform, dropping
    /// its voxels. Returns whether it did.
    pub fn compact(&mut self) -> bool {
        if let Self::Dense(_) = self
            && let Some(value) = self.uniform()
        {
            *self = Self::Uniform(value);
            return true;
        }
        false
    }

    /// Bytes held beside the array itself: none while it's uniform.
    pub fn heap_bytes(&self) -> usize {
        match self {
            Self::Uniform(_) => 0,
            Self::Dense(values) => values.capacity() * size_of::<T>(),
        }
    }
}

impl<T> ChunkArray<T> {
    /// Every value, in [`Chunk::index_of`] order.
    pub fn iter(&self) -> Iter<'_, T> {
        match self {
            Self::Uniform(value) => Iter::Uniform(std::iter::repeat_n(value, CHUNK_VOLUME)),
            Self::Dense(values) => Iter::Dense(values.iter()),
        }
    }

    /// Always [`CHUNK_VOLUME`].
    pub fn len(&self) -> usize {
        CHUNK_VOLUME
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

impl<T: Copy + PartialEq> Index<usize> for ChunkArray<T> {
    type Output = T;

    fn index(&self, index: usize) -> &T {
        match self {
            Self::Uniform(value) => {
                assert!(index < CHUNK_VOLUME, "voxel {index} is outside the chunk");
                value
            }
            Self::Dense(values) => &values[index],
        }
    }
}

/// Equal when every voxel is, however each is kept.
impl<T: Copy + PartialEq> PartialEq for ChunkArray<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Uniform(a), Self::Uniform(b)) => a == b,
            (Self::Dense(a), Self::Dense(b)) => a == b,
            (Self::Uniform(value), Self::Dense(values))
            | (Self::Dense(values), Self::Uniform(value)) => {
                values.iter().all(|other| other == value)
            }
        }
    }
}

impl<T: Copy + Eq> Eq for ChunkArray<T> {}

impl<'a, T> IntoIterator for &'a ChunkArray<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

/// The values of a [`ChunkArray`], in order.
#[derive(Debug, Clone)]
pub enum Iter<'a, T> {
    Uniform(std::iter::RepeatN<&'a T>),
    Dense(std::slice::Iter<'a, T>),
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    #[inline]
    fn next(&mut self) -> Option<&'a T> {
        match self {
            Self::Uniform(values) => values.next(),
            Self::Dense(values) => values.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Uniform(values) => values.size_hint(),
            Self::Dense(values) => values.size_hint(),
        }
    }
}

impl<T> ExactSizeIterator for Iter<'_, T> {}

/// Compacts the blocks and light of the chunks that changed this frame
/// (or spawned), which may have become one value throughout. Without
/// change detection, so nothing remeshes for it.
pub(super) fn compact_changed_chunks(
    mut blocks: Query<&mut Chunk, Changed<Chunk>>,
    mut light: Query<&mut ChunkLight, Changed<ChunkLight>>,
) {
    for mut chunk in &mut blocks {
        chunk.bypass_change_detection().compact();
    }
    for mut light in &mut light {
        light.bypass_change_detection().compact();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::BlockState;

    #[test]
    fn uniform_until_a_voxel_differs() {
        let mut array = ChunkArray::Uniform(BlockState::AIR);
        assert_eq!(array.heap_bytes(), 0);
        array.set(7, BlockState::AIR);
        assert!(matches!(array, ChunkArray::Uniform(_)), "same value");
        array.set(7, BlockState(3));
        assert!(matches!(array, ChunkArray::Dense(_)));
        assert_eq!(array.get(7), BlockState(3));
        assert_eq!(array[6], BlockState::AIR);
        assert_eq!(array.get(CHUNK_VOLUME - 1), BlockState::AIR);
        assert_eq!(array.heap_bytes(), CHUNK_VOLUME * 2);
        assert_eq!(array.uniform(), None);
        assert!(!array.compact());
    }

    #[test]
    fn compacts_when_every_voxel_matches_again() {
        let mut array = ChunkArray::Uniform(BlockState::AIR);
        array.set(100, BlockState(3));
        array.set(100, BlockState::AIR);
        assert_eq!(array.uniform(), Some(BlockState::AIR));
        assert!(array.compact());
        assert!(matches!(array, ChunkArray::Uniform(BlockState::AIR)));
        assert_eq!(array.heap_bytes(), 0);
    }

    #[test]
    fn from_vec_keeps_one_value_uniform() {
        let same = ChunkArray::from_vec(vec![BlockState(2); CHUNK_VOLUME]);
        assert!(matches!(same, ChunkArray::Uniform(BlockState(2))));
        let mut values = vec![BlockState(2); CHUNK_VOLUME];
        values[0] = BlockState(1);
        let mixed = ChunkArray::from_vec(values.clone());
        assert!(matches!(mixed, ChunkArray::Dense(_)));
        assert_eq!(mixed.iter().copied().collect::<Vec<_>>(), values);
    }

    #[test]
    fn iterates_and_compares_by_value_however_kept() {
        let uniform = ChunkArray::Uniform(BlockState(5));
        let dense = ChunkArray::Dense(vec![BlockState(5); CHUNK_VOLUME]);
        assert_eq!(uniform, dense);
        assert_eq!(dense, uniform);
        assert_eq!(uniform.iter().len(), CHUNK_VOLUME);
        assert!(uniform.iter().all(|&state| state == BlockState(5)));
        let mut other = dense.clone();
        other.set(9, BlockState(1));
        assert_ne!(uniform, other);
        assert_ne!(other, uniform);
        assert_eq!(other.iter().nth(9), Some(&BlockState(1)));
    }

    #[test]
    #[should_panic(expected = "outside the chunk")]
    fn a_uniform_array_still_checks_its_bounds() {
        ChunkArray::Uniform(BlockState::AIR).get(CHUNK_VOLUME);
    }

    /// Chunks that changed this frame are compacted, without marking them
    /// changed again.
    #[test]
    fn compacts_changed_chunks() {
        let mut app = App::new();
        app.add_systems(Update, compact_changed_chunks);
        let mut chunk = Chunk::filled(BlockState::AIR);
        chunk.set(0, 0, 0, BlockState(2));
        chunk.set(0, 0, 0, BlockState::AIR);
        let mut light = ChunkLight::default();
        light.set_at(IVec3::ZERO, crate::light::Light(15));
        light.set_at(IVec3::ZERO, crate::light::Light::DARK);
        let entity = app.world_mut().spawn((chunk, light)).id();
        assert!(matches!(
            app.world().get::<Chunk>(entity).unwrap().voxels(),
            ChunkArray::Dense(_)
        ));
        app.update();
        let world = app.world();
        assert!(matches!(
            world.get::<Chunk>(entity).unwrap().voxels(),
            ChunkArray::Uniform(BlockState::AIR)
        ));
        assert!(matches!(
            world.get::<ChunkLight>(entity).unwrap().levels(),
            ChunkArray::Uniform(_)
        ));
    }
}
