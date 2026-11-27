//! Where sky light comes from, in a world with no top.
//!
//! Inside the loaded chunks, sky light spreads like any other light (see
//! `propagate`). The one thing the loaded chunks can't say is what's above
//! them: when the chunk above a chunk isn't loaded, does full sky light
//! shine down into its top layer? [`OutsideSky`] answers that, and it's the
//! only place that decides it: the flood fill, the chunks lit on loading
//! tasks and the light updates on loading, unloading and editing all ask
//! it.
//!
//! For now the answer is [`AssumeOpenSky`]: yes, everywhere. That's a
//! deliberate first version, recorded in `docs/decisions.md` with what's
//! wrong with it and what should replace it (a saved, CubicChunks2-style
//! tree of each column's highest opaque block, backed by the generated and
//! saved chunks). To replace it, implement [`OutsideSky`] and insert it as
//! the [`Sky`] resource before startup.

use std::sync::Arc;

use bevy::prelude::*;

/// Answers whether sky light comes down into a chunk from the unloaded
/// world above it.
pub trait OutsideSky: Send + Sync + 'static {
    /// Whether full sky light shines down into the top voxel of column
    /// `(x, z)` (chunk-local, 0 to 15) of the chunk at `chunk`, when the
    /// chunk above it isn't loaded.
    ///
    /// The answer must not change while that column is in use, since
    /// nothing tells the light to look again; a model whose answers change
    /// needs a way to queue the light updates (see `LightQueue`).
    fn shines_into(&self, chunk: IVec3, x: i32, z: i32) -> bool;
}

/// The first version: whatever isn't loaded above a chunk is open sky. So
/// caves that reach the top of the loaded chunks are lit, and blocks above
/// them cast no shadow until they load. See `docs/decisions.md`.
#[derive(Debug, Clone, Copy, Default)]
pub struct AssumeOpenSky;

impl OutsideSky for AssumeOpenSky {
    fn shines_into(&self, _: IVec3, _: i32, _: i32) -> bool {
        true
    }
}

/// The world's [`OutsideSky`], shared with the loading tasks.
#[derive(Resource, Clone)]
pub struct Sky(pub Arc<dyn OutsideSky>);

impl Default for Sky {
    fn default() -> Self {
        Self(Arc::new(AssumeOpenSky))
    }
}

impl std::ops::Deref for Sky {
    type Target = dyn OutsideSky;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}
