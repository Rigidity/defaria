//! Loading and unloading the world around the player, a chunk at a time,
//! and saving it.
//!
//! The world has no edges in any direction, so what's loaded is a cylinder
//! of chunks around each [`ChunkLoader`] (the local player has one, at its eyes): every
//! chunk within [`StreamingSettings::radius`] chunks sideways and from
//! [`StreamingSettings::below`] chunks below to
//! [`StreamingSettings::above`] above. Both matter: what's below is the
//! ground you look down at from the air, and what's above is all the sky
//! light can see (see `light::sky`), so a cave reaching higher than that
//! is lit as if open. Chunks are loaded nearest first, and chunks that end up
//! further out than that plus [`StreamingSettings::unload_margin`] are
//! saved and unloaded, the furthest first and spread over frames (see
//! [`StreamingSettings::unload_frames`]): taking away a chunk's light
//! costs more than giving it, and a chunk border crossed puts a whole row
//! out of reach at once. The margin keeps a player wandering back and forth
//! over a chunk border from loading and unloading the same chunks over and
//! over.
//!
//! Loading happens on the async task pool, many chunks at once: each task
//! reads the chunk from the [`WorldStore`] or, if it's never been saved,
//! generates it from the terrain, and lights it as if it were alone
//! ([`light_alone`]). Finished chunks are spawned on the main thread,
//! nearest first and at most [`StreamingSettings::spawns_per_frame`] a
//! frame, since settling the light across a new chunk's borders happens
//! there (see `light`). A chunk is meshed only once all 26 chunks around it
//! are loaded (until then it's [`AwaitingNeighbours`]), so it isn't meshed
//! again each time a neighbour arrives, and its faces against chunks that
//! aren't there yet are never drawn. So the edge of what's drawn is a chunk
//! inside the edge of what's loaded.
//!
//! A chunk is saved when it unloads, if it was edited or has never been
//! saved (so the world stays as it was first generated, even if the terrain
//! generator changes); edited chunks are also saved every
//! [`StreamingSettings::autosave_seconds`], and everything is saved when the
//! game exits. Saved chunks are written to disk in the background by
//! [`WorldStore::flush`].
//!
//! Before chunks are saved, [`ChunksSaving`] says which (the ones
//! unloading, and whether the loaded ones are being saved too), and
//! systems in [`BeforeChunkSave`] can write into their block entities what
//! they keep for them elsewhere, such as the fluid a network's pipes hold.

use std::sync::Arc;

use bevy::{
    app::MainScheduleOrder,
    ecs::schedule::ScheduleLabel,
    platform::collections::{HashMap, HashSet},
    prelude::*,
    tasks::{AsyncComputeTaskPool, IoTaskPool, Task, block_on, futures::check_ready},
};

use defaria_core::streaming::{StreamingSettings, StreamingStatus, StreamingStatusSystems};

use crate::{
    block_entity::{BlockEntityChanged, BlockEntitySystems, ChunkSaver},
    light::{LightQueue, LightTable, Prelit, light_alone, sky::Sky},
    save::{WorldStore, region},
    worldgen::{Terrain, TerrainBlocks, TerrainSettings},
};
use defaria_core::{
    content::Content,
    light::ChunkLight,
    player::{EYE_HEIGHT, Player},
    save::WorldFolder,
    world::{
        AwaitingNeighbours, Chunk, ChunkCoord, ChunkMap, FluidChanged, PropertiesUpdated,
        RemeshChunks, VoxelChanged, spawn_chunk,
    },
};

pub struct StreamingPlugin;

impl Plugin for StreamingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StreamingSettings>()
            .init_resource::<TerrainSettings>()
            .init_resource::<Chunks>()
            .init_resource::<StreamingStatus>()
            .init_resource::<ChunksSaving>()
            .init_resource::<SaveNow>()
            .add_message::<BlockEntityChanged>()
            .add_message::<ChunkUnloaded>()
            .add_observer(load_around_the_player)
            .add_systems(Startup, open_world)
            .add_systems(
                Update,
                // After edits and meshing, so this frame's edits are marked
                // before their chunks can unload.
                (
                    (plan_chunks, plan_saves).chain().before(BeforeChunkSave),
                    (
                        mark_edited_chunks,
                        unload_chunks,
                        start_loads,
                        spawn_loaded_chunks,
                        autosave,
                    )
                        .chain()
                        .after(BeforeChunkSave),
                )
                    .after(RemeshChunks)
                    .after(BlockEntitySystems),
            )
            .add_systems(PostUpdate, update_status.in_set(StreamingStatusSystems))
            .init_schedule(SaveOnExit)
            .add_systems(
                SaveOnExit,
                (
                    plan_exit_save.before(BeforeChunkSave),
                    save_on_exit.after(BeforeChunkSave),
                ),
            );
        app.world_mut()
            .resource_mut::<MainScheduleOrder>()
            .insert_after(Last, SaveOnExit);
    }
}

/// Runs after [`Last`], so it sees an [`AppExit`] sent anywhere in the
/// frame (it's the frame's end: the app stops after it).
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SaveOnExit;

/// Save now rather than at the next autosave (the `save` command): the
/// loaded chunks, and the players. Each is cleared once it's done.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct SaveNow {
    pub chunks: bool,
    pub players: bool,
}

/// Which chunks are saved this frame, worked out before they are, for the
/// systems in [`BeforeChunkSave`].
#[derive(Resource, Debug, Clone, Default)]
pub struct ChunksSaving {
    /// The chunks that unload (and are saved) this frame, by coordinate.
    pub unloading: Vec<IVec3>,
    /// Whether the loaded chunks are saved this frame too: an autosave
    /// (the edited ones) or the game exiting (all of them).
    pub loaded: bool,
}

/// Where features write what they keep for chunks outside them into the
/// chunks' block entities, just before the chunks in [`ChunksSaving`] are
/// saved. In [`Update`] for unloading and autosaves, and in [`SaveOnExit`]
/// when the game exits. A system here that changes a block entity writes
/// its [`SavedParts`](crate::block_entity::SavedParts) itself (the usual
/// noting of changes has already run) and sends [`BlockEntityChanged`], so
/// its chunk counts as edited and is saved.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BeforeChunkSave;

/// The chunk a [`ChunkLoader`] at `transform` loads around.
pub fn loader_chunk(transform: &GlobalTransform) -> IVec3 {
    ChunkCoord::split(transform.translation().floor().as_ivec3())
        .0
        .0
}

/// A chunk was unloaded (saved first, if it needed to be).
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkUnloaded(pub ChunkCoord);

/// Loads the world around itself, using its [`GlobalTransform`].
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
#[require(Transform)]
pub struct ChunkLoader;

/// Each player loads the world around its eyes (not its feet, so the
/// levels above and below are counted from what it sees), with a loader
/// that's its child at eye height.
fn load_around_the_player(add: On<Add<Player>>, mut commands: Commands) {
    commands.spawn((
        Name::new("Chunk Loader"),
        ChunkLoader,
        Transform::from_xyz(0.0, EYE_HEIGHT as f32, 0.0),
        ChildOf(add.entity),
    ));
}

/// Where chunks come from: the save, and the terrain for chunks that have
/// never been saved, and how to light them. Shared with the loading tasks.
#[derive(Resource, Clone)]
pub struct WorldSource {
    pub store: Arc<WorldStore>,
    pub terrain: Arc<Terrain>,
    pub light: Arc<LightTable>,
    pub sky: Sky,
}

/// The chunks that are loaded or on their way.
#[derive(Resource, Default)]
pub struct Chunks {
    loaded: HashMap<IVec3, LoadedChunk>,
    loading: HashMap<IVec3, Task<NewChunk>>,
    /// Loaded and waiting to be spawned.
    ready: Vec<(IVec3, NewChunk)>,
    /// Wanted chunks that aren't loaded or loading, furthest first.
    queue: Vec<IVec3>,
    /// The loaders' chunks when the queue was last worked out.
    centers: Vec<IVec3>,
    /// Chunks out of reach left to unload on later frames.
    waiting_to_unload: usize,
    /// Chunks were spawned this frame, so their light is still to settle:
    /// it's queued when they're seen next frame (see `light`).
    spawned_this_frame: bool,
    flush: Option<Task<()>>,
    since_save: f32,
}

#[derive(Debug)]
struct LoadedChunk {
    entity: Entity,
    /// Changed since it was last saved.
    edited: bool,
    /// The store has a copy (maybe older, if `edited`).
    saved: bool,
    /// All 26 chunks around it are loaded.
    complete: bool,
}

/// A chunk ready to spawn.
struct NewChunk {
    chunk: Chunk,
    light: ChunkLight,
    /// Whether it came from the save.
    from_save: bool,
}

impl Chunks {
    /// Whether `chunk` is loaded, loading, or loaded and waiting to spawn.
    fn has(&self, chunk: IVec3) -> bool {
        self.loaded.contains_key(&chunk)
            || self.loading.contains_key(&chunk)
            || self.ready.iter().any(|(c, _)| *c == chunk)
    }
}

impl LoadedChunk {
    /// Copies the chunk into the store if it has changes the store doesn't
    /// have: edits, or (unless `edits_only`) never having been saved.
    fn save(&mut self, coord: IVec3, store: &WorldStore, data: &ChunkSaver, edits_only: bool) {
        if !self.edited && (self.saved || edits_only) {
            return;
        }
        if let Some(chunk) = data.chunk(self.entity) {
            store.save(coord, chunk);
        }
        self.edited = false;
        self.saved = true;
    }
}

/// Whether `chunk` is within reach of a loader in one of `centers`,
/// `extra` chunks further out than what's loaded.
fn wanted(centers: &[IVec3], chunk: IVec3, settings: &StreamingSettings, extra: i32) -> bool {
    centers
        .iter()
        .any(|&center| settings.reaches(chunk - center, extra))
}

/// How far a chunk is from the nearest of `centers`, for loading the
/// nearest first.
fn distance_squared(centers: &[IVec3], chunk: IVec3) -> i64 {
    centers
        .iter()
        .map(|&center| (chunk - center).as_i64vec3().length_squared())
        .min()
        .unwrap_or(i64::MAX)
}

/// Opens the world in [`WorldFolder`] (keeping its seed), or one in memory
/// without it.
fn open_world(
    mut commands: Commands,
    folder: Option<Res<WorldFolder>>,
    mut settings: ResMut<TerrainSettings>,
    content: Res<Content>,
    light: Res<LightTable>,
    sky: Res<Sky>,
) {
    // Chunks generated without the terrain's blocks would be saved as air
    // for good, so a world isn't saved unless they loaded.
    let missing = TerrainBlocks::missing(&content);
    let folder = folder.filter(|_| {
        if !missing.is_empty() {
            error!(
                "the terrain's blocks {missing:?} didn't load; playing a world that won't be saved"
            );
        }
        missing.is_empty()
    });
    let folder =
        folder.and_then(
            |folder| match defaria_core::save::open_world(&folder.0, settings.seed) {
                Ok(seed) => {
                    info!("playing the world in {}", folder.0.display());
                    settings.seed = seed;
                    Some(folder.0.clone())
                }
                Err(error) => {
                    error!("{error}; playing a world that won't be saved");
                    None
                }
            },
        );
    commands.insert_resource(WorldSource {
        store: Arc::new(WorldStore::new(folder.as_deref(), &content)),
        terrain: Arc::new(Terrain::new(
            settings.clone(),
            TerrainBlocks::from_content(&content),
        )),
        light: Arc::new(light.clone()),
        sky: sky.clone(),
    });
}

/// Marks the chunks of edited voxels for saving, including those whose
/// fluid has moved, whose properties the engine updated or whose block
/// entities have changed.
fn mark_edited_chunks(
    mut changes: MessageReader<VoxelChanged>,
    mut fluid_changes: MessageReader<FluidChanged>,
    mut updates: MessageReader<PropertiesUpdated>,
    mut block_entity_changes: MessageReader<BlockEntityChanged>,
    mut chunks: ResMut<Chunks>,
) {
    let changed = changes
        .read()
        .map(|change| change.pos)
        .chain(fluid_changes.read().map(|change| change.pos))
        .chain(updates.read().map(|update| update.pos))
        .chain(block_entity_changes.read().map(|change| change.pos));
    for pos in changed {
        let (coord, _) = ChunkCoord::split(pos);
        if let Some(chunk) = chunks.loaded.get_mut(&coord.0) {
            chunk.edited = true;
        }
    }
}

/// Works out which chunks to load, when a loader has moved to another
/// chunk.
fn plan_chunks(
    loaders: Query<&GlobalTransform, With<ChunkLoader>>,
    settings: Res<StreamingSettings>,
    mut chunks: ResMut<Chunks>,
) {
    let mut centers: Vec<IVec3> = loaders.iter().map(loader_chunk).collect();
    centers.sort_by_key(|c| c.to_array());
    centers.dedup();
    if centers == chunks.centers {
        return;
    }
    chunks.centers = centers;
    let mut queue: Vec<IVec3> = (chunks.centers.iter())
        .flat_map(|&center| settings.window(center))
        .collect();
    queue.sort_by_key(|c| c.to_array());
    queue.dedup();
    queue.retain(|&c| !chunks.has(c));
    // Furthest first, so the nearest pops off the end.
    queue.sort_by_key(|&c| std::cmp::Reverse(distance_squared(&chunks.centers, c)));
    chunks.queue = queue;
    // Loads that aren't wanted any more are dropped, which cancels them.
    let Chunks {
        loading,
        ready,
        centers,
        ..
    } = &mut *chunks;
    loading.retain(|&c, _| wanted(centers, c, &settings, 0));
    ready.retain(|(c, _)| wanted(centers, *c, &settings, 0));
}

/// Works out which chunks are saved this frame: those gone out of reach,
/// which unload, and the edited ones if an autosave is due.
fn plan_saves(
    time: Res<Time>,
    settings: Res<StreamingSettings>,
    mut chunks: ResMut<Chunks>,
    mut saving: ResMut<ChunksSaving>,
    mut save_now: ResMut<SaveNow>,
) {
    let mut unloading: Vec<IVec3> = chunks
        .loaded
        .keys()
        .copied()
        .filter(|&c| !wanted(&chunks.centers, c, &settings, settings.unload_margin))
        .collect();
    // Furthest first, a frame's worth: crossing a chunk border leaves a
    // whole row out of reach at once, and taking its light away all on one
    // frame (see `light`) is most of what crossing used to cost.
    unloading.sort_by_key(|&c| {
        (
            std::cmp::Reverse(distance_squared(&chunks.centers, c)),
            c.to_array(),
        )
    });
    let unloads = settings.unloads_this_frame(unloading.len());
    chunks.waiting_to_unload = unloading.len().saturating_sub(unloads);
    unloading.truncate(unloads);
    unloading.sort_by_key(|c| c.to_array());
    chunks.since_save += time.delta_secs();
    let autosave = (chunks.since_save >= settings.autosave_seconds || save_now.chunks)
        && chunks.flush.is_none();
    if autosave {
        chunks.since_save = 0.0;
        save_now.chunks = false;
    }
    *saving = ChunksSaving {
        unloading,
        loaded: autosave,
    };
}

/// Saves and despawns the chunks that have gone out of reach.
fn unload_chunks(
    mut commands: Commands,
    mut unloaded: MessageWriter<ChunkUnloaded>,
    source: Res<WorldSource>,
    saving: Res<ChunksSaving>,
    mut chunks: ResMut<Chunks>,
    mut map: ResMut<ChunkMap>,
    data: ChunkSaver,
) {
    let gone = &saving.unloading;
    if gone.is_empty() {
        return;
    }
    for &coord in gone {
        let Some(mut loaded) = chunks.loaded.remove(&coord) else {
            continue;
        };
        loaded.save(coord, &source.store, &data, false);
        map.remove(ChunkCoord(coord));
        commands.entity(loaded.entity).despawn();
        unloaded.write(ChunkUnloaded(ChunkCoord(coord)));
    }
    let around: HashSet<IVec3> = gone.iter().flat_map(|&c| around(c)).collect();
    update_completeness(&mut commands, &mut chunks, around);
}

/// Starts loading the nearest wanted chunks, up to the limit.
fn start_loads(
    settings: Res<StreamingSettings>,
    source: Res<WorldSource>,
    mut chunks: ResMut<Chunks>,
) {
    let pool = AsyncComputeTaskPool::get();
    while chunks.loading.len() + chunks.ready.len() < settings.max_loading {
        let Some(coord) = chunks.queue.pop() else {
            break;
        };
        if chunks.has(coord) {
            continue;
        }
        let source = source.clone();
        let task = pool.spawn(async move { load_chunk(&source, coord) });
        chunks.loading.insert(coord, task);
    }
}

/// Reads a chunk from the store, or generates it, and lights it. Runs on
/// the task pool.
fn load_chunk(source: &WorldSource, coord: IVec3) -> NewChunk {
    let (chunk, from_save) = match source.store.load(coord) {
        Some(chunk) => (chunk, true),
        None => (source.terrain.chunk(ChunkCoord(coord)), false),
    };
    let light = light_alone(&[(coord, &chunk)], &source.light, &*source.sky)
        .pop()
        .expect("one chunk in, one light out");
    NewChunk {
        chunk,
        light,
        from_save,
    }
}

/// Spawns the nearest chunks that have finished loading, and takes the
/// [`AwaitingNeighbours`] mark off the chunks they complete.
fn spawn_loaded_chunks(
    mut commands: Commands,
    settings: Res<StreamingSettings>,
    mut chunks: ResMut<Chunks>,
    mut map: ResMut<ChunkMap>,
) {
    let chunks = &mut *chunks;
    chunks.spawned_this_frame = false;
    chunks
        .loading
        .retain(|&coord, task| match check_ready(task) {
            Some(new) => {
                chunks.ready.push((coord, new));
                false
            }
            None => true,
        });
    if chunks.ready.is_empty() {
        return;
    }
    // Nearest last, to pop.
    let centers = &chunks.centers;
    chunks
        .ready
        .sort_by_key(|(c, _)| std::cmp::Reverse(distance_squared(centers, *c)));
    let mut spawned = HashSet::new();
    for _ in 0..settings.spawns_per_frame {
        let Some((coord, new)) = chunks.ready.pop() else {
            break;
        };
        if chunks.loaded.contains_key(&coord) {
            continue;
        }
        let extra = (new.light, Prelit, AwaitingNeighbours);
        let entity = spawn_chunk(&mut commands, &mut map, ChunkCoord(coord), new.chunk, extra);
        chunks.loaded.insert(
            coord,
            LoadedChunk {
                entity,
                edited: false,
                saved: new.from_save,
                complete: false,
            },
        );
        spawned.insert(coord);
    }
    chunks.spawned_this_frame = !spawned.is_empty();
    let candidates: HashSet<IVec3> = spawned.iter().flat_map(|&c| around(c).chain([c])).collect();
    update_completeness(&mut commands, chunks, candidates);
}

/// The 26 chunks around one.
fn around(coord: IVec3) -> impl Iterator<Item = IVec3> {
    ChunkCoord(coord).neighbours().map(|c| c.0)
}

/// Marks or unmarks these chunks as [`AwaitingNeighbours`], by whether the
/// chunks around each one are all loaded.
fn update_completeness(
    commands: &mut Commands,
    chunks: &mut Chunks,
    candidates: impl IntoIterator<Item = IVec3>,
) {
    for coord in candidates {
        let complete = around(coord).all(|c| chunks.loaded.contains_key(&c));
        let Some(loaded) = chunks.loaded.get_mut(&coord) else {
            continue;
        };
        if loaded.complete == complete {
            continue;
        }
        loaded.complete = complete;
        if complete {
            commands
                .entity(loaded.entity)
                .remove::<AwaitingNeighbours>();
        } else {
            commands.entity(loaded.entity).insert(AwaitingNeighbours);
        }
    }
}

/// Every so often, saves the edited chunks and writes the store to disk.
fn autosave(
    saving: Res<ChunksSaving>,
    source: Res<WorldSource>,
    mut chunks: ResMut<Chunks>,
    data: ChunkSaver,
) {
    if let Some(task) = &mut chunks.flush
        && check_ready(task).is_some()
    {
        chunks.flush = None;
        let in_use: HashSet<IVec3> = chunks
            .loaded
            .keys()
            .chain(chunks.loading.keys())
            .chain(chunks.ready.iter().map(|(c, _)| c))
            .map(|&c| region::split(c).0)
            .collect();
        source.store.evict(|region| in_use.contains(&region));
    }
    if !saving.loaded {
        return;
    }
    for (&coord, loaded) in chunks.loaded.iter_mut() {
        loaded.save(coord, &source.store, &data, true);
    }
    let store = source.store.clone();
    chunks.flush = Some(IoTaskPool::get().spawn(async move {
        store.flush();
    }));
}

/// When the game exits, notes that every loaded chunk is about to be saved.
fn plan_exit_save(mut exits: MessageReader<AppExit>, mut saving: ResMut<ChunksSaving>) {
    *saving = ChunksSaving {
        unloading: Vec::new(),
        loaded: exits.read().count() > 0,
    };
}

/// Saves everything when the game exits, and waits for it to be written.
fn save_on_exit(
    saving: Res<ChunksSaving>,
    mut block_entity_changes: MessageReader<BlockEntityChanged>,
    source: Option<Res<WorldSource>>,
    mut chunks: ResMut<Chunks>,
    data: ChunkSaver,
) {
    if !saving.loaded {
        return;
    }
    let Some(source) = source else {
        return;
    };
    // What changed just now, in `BeforeChunkSave`.
    for change in block_entity_changes.read() {
        let (coord, _) = ChunkCoord::split(change.pos);
        if let Some(chunk) = chunks.loaded.get_mut(&coord.0) {
            chunk.edited = true;
        }
    }
    if let Some(task) = chunks.flush.take() {
        block_on(task);
    }
    for (&coord, loaded) in chunks.loaded.iter_mut() {
        loaded.save(coord, &source.store, &data, false);
    }
    let written = source.store.flush();
    info!("saved the world ({written} regions written)");
}

/// Blocks until every chunk load started so far has finished, so the
/// next update picks them all up, as if the background threads always
/// kept up. For tests whose answer shouldn't depend on how fast the
/// machine is.
#[cfg(any(test, feature = "test-support"))]
pub fn finish_loads(world: &mut World) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let loads_done = world
            .resource::<Chunks>()
            .loading
            .values()
            .all(Task::is_finished);
        if loads_done {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "background work never finished"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Works out [`StreamingStatus`]. Settled includes the light: chunks
/// spawned this frame are lit across their borders next frame, and
/// chunks unloaded this frame have queued the light they take away, so
/// streaming isn't settled until a frame after the last of them.
fn update_status(
    chunks: Res<Chunks>,
    light: Option<Res<LightQueue>>,
    mut status: ResMut<StreamingStatus>,
) {
    status.loaded_chunks = chunks.loaded.len();
    status.settled = chunks.queue.is_empty()
        && chunks.waiting_to_unload == 0
        && chunks.loading.is_empty()
        && chunks.ready.is_empty()
        && !chunks.spawned_this_frame
        && light.is_none_or(|light| light.is_empty())
        && !chunks.centers.is_empty();
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::{
        testing::{move_loader, patch_app, settle, world_app},
        worldgen::testing::PATCH,
    };
    use defaria_core::{
        content::{BlockState, base, block},
        light::MAX_LIGHT,
        streaming::SETTLE_TIMEOUT_FRAMES,
        testing::temp_folder,
        world::Voxels,
    };

    fn settings() -> StreamingSettings {
        StreamingSettings {
            radius: 3,
            above: 2,
            below: 2,
            ..default()
        }
    }

    /// The terrain, light and streaming with no rendering, loading around
    /// a loader at `at`, saving to `folder` if there is one.
    fn app(folder: Option<&Path>, at: Vec3) -> App {
        app_with_seed(folder, at, TerrainSettings::default().seed)
    }

    fn app_with_seed(folder: Option<&Path>, at: Vec3, seed: u64) -> App {
        world_app(base().clone())
            .resource(settings())
            .resource(TerrainSettings { seed, ..default() })
            .folder(folder)
            .home(at)
            .build()
    }

    fn loaded(app: &App) -> HashSet<IVec3> {
        app.world()
            .resource::<Chunks>()
            .loaded
            .keys()
            .copied()
            .collect()
    }

    /// The chunks within reach of a loader in chunk `center`.
    fn cylinder(center: IVec3, extra: i32) -> HashSet<IVec3> {
        let settings = settings();
        let r = settings.radius + extra + settings.above + settings.below;
        let mut chunks = HashSet::new();
        for x in -r..=r {
            for y in -r..=r {
                for z in -r..=r {
                    let offset = IVec3::new(x, y, z);
                    if settings.reaches(offset, extra) {
                        chunks.insert(center + offset);
                    }
                }
            }
        }
        chunks
    }

    fn get(app: &mut App, pos: IVec3) -> Option<BlockState> {
        app.world_mut()
            .run_system_once(move |voxels: Voxels| voxels.get(pos))
            .unwrap()
    }

    fn set(app: &mut App, pos: IVec3, state: BlockState) {
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| assert!(voxels.set(pos, state)))
            .unwrap();
        app.update();
    }

    fn light(app: &App, coord: IVec3) -> ChunkLight {
        let entity = app
            .world()
            .resource::<ChunkMap>()
            .get(ChunkCoord(coord))
            .unwrap();
        app.world().get::<ChunkLight>(entity).unwrap().clone()
    }

    /// Sideways a circle, and up and down the set number of chunks, around
    /// the loader's chunk, wherever that is: the world has no edges.
    #[test]
    fn chunks_within_reach_load() {
        let app = app(None, Vec3::new(8.0, 40.0, 8.0));
        assert_eq!(loaded(&app), cylinder(IVec3::new(0, 2, 0), 0));
        // 29 columns, 5 chunks tall.
        let status = app.world().resource::<StreamingStatus>();
        assert_eq!(status.loaded_chunks, 29 * 5);
        let map = app.world().resource::<ChunkMap>();
        assert!(map.get(ChunkCoord(IVec3::new(-3, 4, 0))).is_some());
        assert!(map.get(ChunkCoord(IVec3::new(-3, 5, 0))).is_none());
        assert!(map.get(ChunkCoord(IVec3::new(0, -1, 0))).is_none());

        let deep = self::app(None, Vec3::new(8.0, -100_000.0, 8.0));
        assert!(loaded(&deep).contains(&IVec3::new(0, -6250, 0)));
    }

    /// Only chunks with all 26 chunks around them loaded are ready to
    /// draw; the rest await their neighbours.
    #[test]
    fn the_edge_waits_for_its_neighbours() {
        let mut app = app(None, Vec3::new(8.0, 40.0, 8.0));
        let all = cylinder(IVec3::new(0, 2, 0), 0);
        let mut chunks = app
            .world_mut()
            .query::<(&ChunkCoord, Has<AwaitingNeighbours>)>();
        let mut ready = 0;
        for (coord, awaiting) in chunks.iter(app.world()) {
            let inner = around(coord.0).all(|c| all.contains(&c));
            assert_eq!(awaiting, !inner, "{coord:?}");
            ready += !awaiting as usize;
        }
        assert!(ready > 0);
    }

    /// Moving away unloads what's out of reach (past the margin) and loads
    /// what's come into it; chunks within the margin stay. Up and down too.
    #[test]
    fn moving_loads_ahead_and_unloads_behind() {
        let mut app = app(None, Vec3::new(8.0, 40.0, 8.0));
        let margin = settings().unload_margin;
        for center in [IVec3::new(4, 2, 0), IVec3::new(4, -3, 0)] {
            move_loader(&mut app, (center * 16).as_vec3() + 8.0);
            let now = loaded(&app);
            assert!(now.is_superset(&cylinder(center, 0)));
            assert!(now.is_subset(&cylinder(center, margin)));
            // The chunk entities went with them.
            let mut chunks = app.world_mut().query::<&ChunkCoord>();
            assert_eq!(chunks.iter(app.world()).count(), now.len());
        }
        let now = loaded(&app);
        assert!(now.contains(&IVec3::new(4, -5, 0)), "below, newly loaded");
        assert!(
            !now.contains(&IVec3::new(4, 2, 0)),
            "above, past the margin"
        );
        assert!(
            !now.contains(&IVec3::new(-1, -3, 0)),
            "behind, past the margin"
        );
    }

    /// Chunks out of reach unload a few a frame, the furthest first, so a
    /// border crossed doesn't unload a whole row (and take its light away)
    /// on one frame.
    #[test]
    fn chunks_out_of_reach_unload_a_few_a_frame_furthest_first() {
        let mut app = app(None, Vec3::new(8.0, 40.0, 8.0));
        {
            let mut settings = app.world_mut().resource_mut::<StreamingSettings>();
            settings.unloads_per_frame = 2;
            settings.unload_frames = 1000;
        }
        let center = IVec3::new(6, 2, 0);
        let mut loaders = app
            .world_mut()
            .query_filtered::<&mut Transform, With<ChunkLoader>>();
        loaders.single_mut(app.world_mut()).unwrap().translation = (center * 16).as_vec3() + 8.0;
        let margin = settings().unload_margin;
        let out_of_reach = |app: &App| -> HashSet<IVec3> {
            loaded(app)
                .difference(&cylinder(center, margin))
                .copied()
                .collect()
        };
        let distance = |c: &IVec3| distance_squared(&[center], *c);
        let mut waiting = out_of_reach(&app);
        assert!(waiting.len() > 4, "{waiting:?}");
        let mut frames = 0;
        while !waiting.is_empty() {
            app.update();
            frames += 1;
            let now = out_of_reach(&app);
            let gone: Vec<&IVec3> = waiting.difference(&now).collect();
            assert!(gone.len() <= 2, "{gone:?}");
            let nearest_gone = gone.iter().map(|c| distance(c)).min();
            let furthest_left = now.iter().map(distance).max();
            if let (Some(gone), Some(left)) = (nearest_gone, furthest_left) {
                assert!(gone >= left, "unloaded {gone} before {left}");
            }
            waiting = now;
            assert!(frames < 1000, "never unloaded");
        }
        assert!(frames >= 3, "spread over frames: {frames}");
    }

    /// The share of the chunks waiting to unload that go each frame.
    #[test]
    fn unloads_keep_up_with_what_waits() {
        let settings = StreamingSettings {
            unloads_per_frame: 16,
            unload_frames: 30,
            ..default()
        };
        assert_eq!(settings.unloads_this_frame(0), 16);
        assert_eq!(settings.unloads_this_frame(100), 16);
        assert_eq!(settings.unloads_this_frame(1000), 34);
        assert_eq!(settings.unloads_this_frame(30_000), 1000);
    }

    /// Moving while loaded chunks are still waiting to be spawned doesn't
    /// load them a second time: every chunk has one entity.
    #[test]
    fn moving_mid_load_loads_each_chunk_once() {
        let mut app = app(None, Vec3::new(8.0, 40.0, 8.0));
        app.world_mut()
            .resource_mut::<StreamingSettings>()
            .spawns_per_frame = 1;
        let mut loaders = app
            .world_mut()
            .query_filtered::<&mut Transform, With<ChunkLoader>>();
        for step in 1..=6 {
            loaders.single_mut(app.world_mut()).unwrap().translation =
                Vec3::new(8.0 + 16.0 * step as f32, 40.0, 8.0);
            for _ in 0..3 {
                app.update();
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        app.world_mut()
            .resource_mut::<StreamingSettings>()
            .spawns_per_frame = 24;
        settle(&mut app);
        let mut chunks = app.world_mut().query::<&ChunkCoord>();
        let coords: Vec<ChunkCoord> = chunks.iter(app.world()).copied().collect();
        let unique: HashSet<ChunkCoord> = coords.iter().copied().collect();
        assert_eq!(coords.len(), unique.len(), "a chunk was spawned twice");
        assert_eq!(coords.len(), loaded(&app).len());
    }

    #[test]
    fn edits_survive_unloading() {
        let mut app = app(None, Vec3::new(8.0, 40.0, 8.0));
        let pos = IVec3::new(3, 50, -5);
        set(&mut app, pos, block("lamp"));
        move_loader(&mut app, Vec3::new(8.0, 5000.0, 8.0));
        assert_eq!(get(&mut app, pos), None, "unloaded");
        move_loader(&mut app, Vec3::new(8.0, 40.0, 8.0));
        assert_eq!(get(&mut app, pos), Some(block("lamp")));
    }

    /// A chunk that came from the save keeps an edit made to it through
    /// unloading too, not only one that was generated.
    #[test]
    fn edits_to_saved_chunks_survive_unloading() {
        let mut app = app(None, Vec3::new(8.0, 40.0, 8.0));
        let pos = IVec3::new(3, 50, -5);
        let coord = ChunkCoord::split(pos).0.0;
        // Away and back, so the chunk comes back from the save.
        move_loader(&mut app, Vec3::new(8.0, 5000.0, 8.0));
        move_loader(&mut app, Vec3::new(8.0, 40.0, 8.0));
        assert!(app.world().resource::<Chunks>().loaded[&coord].saved);
        set(&mut app, pos, block("lamp"));
        move_loader(&mut app, Vec3::new(8.0, 5000.0, 8.0));
        move_loader(&mut app, Vec3::new(8.0, 40.0, 8.0));
        assert_eq!(get(&mut app, pos), Some(block("lamp")));
    }

    /// A change to only the amount of fluid in a voxel marks its chunk
    /// edited, so it's saved and comes back after unloading.
    #[test]
    fn fluid_amounts_survive_unloading() {
        let mut app = app(None, Vec3::new(8.0, 40.0, 8.0));
        let pos = IVec3::new(3, 50, -5);
        let coord = ChunkCoord::split(pos).0.0;
        set(&mut app, pos, block("water"));
        // Away and back, so the chunk comes back from the save.
        move_loader(&mut app, Vec3::new(8.0, 5000.0, 8.0));
        move_loader(&mut app, Vec3::new(8.0, 40.0, 8.0));
        assert!(app.world().resource::<Chunks>().loaded[&coord].saved);
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| {
                let fluid = defaria_core::world::VoxelFluid::of(block("water"), 321);
                assert!(voxels.set_fluid(pos, fluid));
            })
            .unwrap();
        app.update();
        move_loader(&mut app, Vec3::new(8.0, 5000.0, 8.0));
        move_loader(&mut app, Vec3::new(8.0, 40.0, 8.0));
        let (coord, local) = ChunkCoord::split(pos);
        let entity = app.world().resource::<ChunkMap>().get(coord).unwrap();
        let chunk = app.world().get::<Chunk>(entity).unwrap();
        assert_eq!(chunk.fluid_amount_at(local), 321);
    }

    /// Generated chunks are saved when they unload, edited or not, so the
    /// world stays as it was first generated.
    #[test]
    fn untouched_generated_chunks_are_saved_on_unloading() {
        let mut app = app(None, Vec3::new(8.0, 40.0, 8.0));
        let coord = IVec3::new(0, 2, 0);
        let store = app.world().resource::<WorldSource>().store.clone();
        assert!(store.load(coord).is_none(), "not saved while loaded");
        move_loader(&mut app, Vec3::new(8.0, 5000.0, 8.0));
        assert!(store.load(coord).is_some());
    }

    /// The world is just as there far away; distances that would overflow
    /// when squared in `i32` don't.
    #[test]
    fn very_long_jumps_work() {
        let mut app = app(None, Vec3::new(8.0, 40.0, 8.0));
        move_loader(&mut app, Vec3::new(1.0e7, 40.0, -1.0e7));
        let center = ChunkCoord::split(IVec3::new(10_000_000, 40, -10_000_000))
            .0
            .0;
        assert_eq!(loaded(&app), cylinder(center, 0));
    }

    /// A world on disk keeps its edits and its seed through a restart, and
    /// saves the chunks it generated.
    #[test]
    fn worlds_on_disk_survive_a_restart() {
        let folder = temp_folder("restart");
        let pos = IVec3::new(-20, 50, 30);
        {
            let mut app = app(Some(&folder), Vec3::new(8.0, 40.0, 8.0));
            set(&mut app, pos, block("red_lamp"));
            app.world_mut().write_message(AppExit::Success);
            app.update();
        }
        let info = std::fs::read_to_string(folder.join("world.json5")).unwrap();
        assert!(info.contains(&format!("seed: {}", TerrainSettings::default().seed)));
        let regions: Vec<_> = std::fs::read_dir(folder.join("regions"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        // Chunks -3..=3 sideways and 0..=4 up straddle four regions.
        assert_eq!(regions.len(), 4, "{regions:?}");

        // A different seed in the settings doesn't change a saved world.
        let mut app = app_with_seed(Some(&folder), Vec3::new(8.0, 40.0, 8.0), 1);
        assert_eq!(get(&mut app, pos), Some(block("red_lamp")));
        let seed = app.world().resource::<TerrainSettings>().seed;
        assert_eq!(seed, TerrainSettings::default().seed);
    }

    /// Without the terrain's blocks, nothing is written to the world: what
    /// was generated would be saved as air.
    #[test]
    fn worlds_arent_saved_without_the_terrains_blocks() {
        let folder = temp_folder("no-blocks");
        let mut app = world_app(defaria_core::content::test_mod(&[]))
            .resource(settings())
            .folder(Some(&folder))
            .home(Vec3::splat(8.0))
            .build();
        app.world_mut().write_message(AppExit::Success);
        app.update();
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
    }

    /// Edits reach the disk while the game runs, not only when it exits.
    #[test]
    fn edits_are_saved_as_the_game_runs() {
        let folder = temp_folder("autosave");
        let mut app = app(Some(&folder), Vec3::new(8.0, 40.0, 8.0));
        app.world_mut()
            .resource_mut::<StreamingSettings>()
            .autosave_seconds = 0.0;
        let pos = IVec3::new(5, 50, 5);
        set(&mut app, pos, block("blue_lamp"));
        let on_disk = || {
            let (coord, local) = ChunkCoord::split(pos);
            let saved = WorldStore::new(Some(&folder), base()).load(coord.0)?;
            Some(saved.at(local))
        };
        // The write is on the IO pool, which other tests share: wait for
        // it, with a deadline only against a hang.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while on_disk() != Some(block("blue_lamp")) {
            assert!(
                std::time::Instant::now() < deadline,
                "the edit never reached the disk: {:?}",
                on_disk()
            );
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Chunks lit on their loading tasks and joined to the world end up
    /// with the same light as chunks lit in place, all at once.
    #[test]
    fn streamed_light_matches_light_worked_out_in_place() {
        let streamed = app(None, Vec3::new(4.0 * 16.0, 40.0, 4.0 * 16.0));
        let patch = patch_app(base().clone());
        // Chunks in the middle of both, away from either's edge.
        for x in 3..=5 {
            for y in 0..PATCH.y {
                let coord = IVec3::new(x, y, 4);
                assert!(light(&streamed, coord) == light(&patch, coord), "{coord}");
            }
        }
    }

    /// Streaming isn't settled until the light of the chunks it spawned
    /// last has spread, the next frame: here a roof's chunk spawns on the
    /// same frame as every other, the last, and the moment streaming says
    /// it's settled the roof already shades what's under it. Loads are
    /// finished before each frame, so the last frame always spawns
    /// something, however fast the machine is.
    #[test]
    fn settled_includes_the_light_of_the_last_chunks_spawned() {
        let mut app = app(None, Vec3::new(8.0, 20.5 * 16.0, 8.0));
        for x in 0..16 {
            for z in 0..16 {
                set(&mut app, IVec3::new(x, 22 * 16 + 8, z), block("stone"));
            }
        }
        let (probe, local) = (IVec3::new(0, 18, 0), IVec3::new(8, 12, 8));
        let sky = |app: &App| light(app, probe).at(local).sky();
        move_loader(&mut app, Vec3::new(8.0, 17.5 * 16.0, 8.0));
        assert_eq!(sky(&app), MAX_LIGHT);
        app.world_mut()
            .resource_mut::<StreamingSettings>()
            .spawns_per_frame = 1000;
        let mut loaders = app
            .world_mut()
            .query_filtered::<&mut Transform, With<ChunkLoader>>();
        loaders.single_mut(app.world_mut()).unwrap().translation = Vec3::new(8.0, 20.5 * 16.0, 8.0);
        app.update();
        app.update();
        let mut frames = 0;
        loop {
            finish_loads(app.world_mut());
            app.update();
            if app.world().resource::<StreamingStatus>().settled {
                break;
            }
            frames += 1;
            assert!(frames < SETTLE_TIMEOUT_FRAMES, "never settled");
        }
        assert!(sky(&app) < MAX_LIGHT, "the roof's light hadn't spread");
    }

    /// Flying up and down moves where the sky comes in: a roof that's
    /// loaded shades what's under it, and once it's out of reach what's
    /// under it is lit again, since nothing is known above what's loaded.
    /// See `docs/decisions.md`.
    #[test]
    fn the_sky_comes_in_above_whatever_is_loaded() {
        // High in the air, with no terrain about.
        let mut app = app(None, Vec3::new(8.0, 20.5 * 16.0, 8.0));
        // A roof over the whole of chunk column (0, 0), in chunk 22.
        for x in 0..16 {
            for z in 0..16 {
                set(&mut app, IVec3::new(x, 22 * 16 + 8, z), block("stone"));
            }
        }
        // Halfway between the roof's edges, in chunk 18.
        let (probe, local) = (IVec3::new(0, 18, 0), IVec3::new(8, 12, 8));
        let sky = |app: &App| light(app, probe).at(local).sky();
        assert!(sky(&app) < MAX_LIGHT, "shaded: {}", sky(&app));
        // Down, so the roof unloads but the probe doesn't.
        move_loader(&mut app, Vec3::new(8.0, 17.5 * 16.0, 8.0));
        assert_eq!(get(&mut app, IVec3::new(8, 22 * 16 + 8, 8)), None);
        assert_eq!(sky(&app), MAX_LIGHT);
        // Back up: shaded again.
        move_loader(&mut app, Vec3::new(8.0, 20.5 * 16.0, 8.0));
        assert!(sky(&app) < MAX_LIGHT);
    }

    /// The local player's loader is at its eyes, not its feet, so the
    /// levels loaded above and below count from what it sees.
    #[test]
    fn the_player_loads_around_its_eyes() {
        use defaria_core::player::{Eyes, tests::local_player_at};

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin))
            .add_observer(load_around_the_player);
        let at = Vec3::new(3.5, 40.0, -7.5);
        app.world_mut()
            .spawn(local_player_at(Transform::from_translation(at)));
        app.update();
        let world = app.world_mut();
        let eyes = world
            .query_filtered::<Eyes, With<defaria_core::player::LocalPlayer>>()
            .single(world)
            .unwrap()
            .transform();
        assert!(eyes.translation.distance(at) < 1e-4, "{eyes:?}");
        let loader = *world
            .query_filtered::<&GlobalTransform, With<ChunkLoader>>()
            .single(world)
            .unwrap();
        assert!(
            loader.translation().distance(eyes.translation) < 1e-4,
            "{loader:?} {eyes:?}"
        );
    }
}
