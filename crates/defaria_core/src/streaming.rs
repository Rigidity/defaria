//! How much of the world is loaded around each player, and whether it
//! all is yet. Both sides keep these: the server loads by its
//! [`StreamingSettings`] (`defaria_server::streaming` loads, unloads and
//! saves the chunks), and the client draws and fogs by its copy, and
//! counts its own chunks into its [`StreamingStatus`].

use bevy::prelude::*;

/// How much of the world is loaded, and how fast.
#[derive(Resource, Reflect, Debug, Clone)]
#[reflect(Resource)]
pub struct StreamingSettings {
    /// Chunks within this many chunks of a loader sideways (a circle) are
    /// loaded, and drawn to one chunk less.
    pub radius: i32,
    /// How many chunks above a loader's are loaded.
    pub above: i32,
    /// How many chunks below a loader's are loaded.
    pub below: i32,
    /// How much further out a chunk can get before it unloads.
    pub unload_margin: i32,
    /// Chunks being loaded or generated at once.
    pub max_loading: usize,
    /// Most loaded chunks spawned (and lit) in one frame.
    pub spawns_per_frame: usize,
    /// Chunks unloaded (and saved) a frame, the furthest first, when that
    /// keeps up: at least this many, or [`Self::unload_frames`]'s share.
    pub unloads_per_frame: usize,
    /// Chunks out of reach are unloaded over about this many frames, or
    /// [`Self::unloads_per_frame`] a frame if that's more, so however fast
    /// the loader moves, the chunks waiting to unload don't pile up.
    pub unload_frames: usize,
    /// Seconds between saves of the edited chunks.
    pub autosave_seconds: f32,
}

impl Default for StreamingSettings {
    fn default() -> Self {
        Self {
            radius: 16,
            above: 12,
            below: 12,
            unload_margin: 2,
            max_loading: 64,
            spawns_per_frame: 24,
            unloads_per_frame: 16,
            unload_frames: 30,
            autosave_seconds: 30.0,
        }
    }
}

impl StreamingSettings {
    /// How many of `waiting` chunks out of reach unload this frame.
    pub fn unloads_this_frame(&self, waiting: usize) -> usize {
        self.unloads_per_frame
            .max(waiting.div_ceil(self.unload_frames.max(1)))
    }

    /// Whether a chunk `offset` from a loader's chunk is within reach,
    /// `extra` chunks further out than what's loaded.
    pub fn reaches(&self, offset: IVec3, extra: i32) -> bool {
        // In i64: a long jump makes the offset too big to square in i32.
        let radius = (self.radius + extra) as i64;
        let (x, z) = (offset.x as i64, offset.z as i64);
        x * x + z * z <= radius * radius
            && (-self.below - extra..=self.above + extra).contains(&offset.y)
    }

    /// The chunks a loader in chunk `center` loads: its window, without
    /// the margin chunks stay loaded in once it moves on.
    pub fn window(&self, center: IVec3) -> impl Iterator<Item = IVec3> + '_ {
        let (radius, levels) = (self.radius, -self.below..=self.above);
        (-radius..=radius)
            .flat_map(move |x| {
                let levels = levels.clone();
                (-radius..=radius)
                    .flat_map(move |z| levels.clone().map(move |y| IVec3::new(x, y, z)))
            })
            .filter(|&offset| self.reaches(offset, 0))
            .map(move |offset| center + offset)
    }

    /// Whether a chunk `offset` from a loader's chunk is one chunk in from
    /// the edge of what's loaded, where chunks are drawn: a chunk is only
    /// drawn once all its neighbours are loaded (see
    /// [`AwaitingNeighbours`](crate::world::AwaitingNeighbours)).
    ///
    /// Only nearly: a few chunks this counts, on the circle's diagonal edges,
    /// have a neighbour outside the loaded circle and are never drawn. The
    /// fog's cylinder keeps a chunk clear of the edge, so they don't reach
    /// it.
    pub fn drawn(&self, offset: IVec3) -> bool {
        self.reaches(offset, -1)
    }

    /// The levels, relative to a loader's chunk, that can be drawn.
    pub fn drawn_levels(&self) -> std::ops::RangeInclusive<i32> {
        1 - self.below..=self.above - 1
    }
}

/// Whether everything the loaders want is loaded (and drawn, where the
/// chunks are drawn: drawing unsettles it after [`StreamingStatusSystems`]
/// while chunks wait for their meshes), and what they don't want (past
/// the margin) unloaded.
#[derive(Resource, Reflect, Debug, Clone, Default)]
#[reflect(Resource)]
pub struct StreamingStatus {
    pub settled: bool,
    pub loaded_chunks: usize,
}

/// Give up if the world hasn't finished loading this many frames into a
/// headless `settle` (or the start), or a test's. Only a guard against a hang: loading is on
/// other threads, so how many frames it takes depends on the machine. The
/// mountains at the full render distance take about 1,900 here, and CI's
/// less optimized build, on fewer cores, takes more.
pub const SETTLE_TIMEOUT_FRAMES: u32 = 12_000;

/// Where [`StreamingStatus`] is worked out, in `PostUpdate`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamingStatusSystems;
