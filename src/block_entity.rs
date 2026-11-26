//! Block entities: data a voxel keeps beyond its block state, such as a
//! machine's stored energy or a script's table, saved with its chunk.
//!
//! A block whose definition has `block_entity` gives each of its voxels
//! one. While its chunk is loaded, each is an entity under the chunk's
//! entity, with a [`BlockEntity`] naming its voxel and a component for each
//! of its *parts*. Feature plugins add parts with
//! [`RegisterBlockEntityPart::register_block_entity_part`]: a component
//! that implements [`BlockEntityPart`], which says which blocks' block
//! entities start with it and how it's saved as a plain [`Value`].
//! Gameplay systems query those components like any others, and find the
//! one at a voxel with [`BlockEntities`]. [`BlockData`], a table of plain
//! values, is the part every feature can share.
//!
//! - **Loading:** when a chunk spawns, the block entities it was saved with
//!   become entities, parts read back from their saved values. A voxel
//!   whose block has a block entity but that wasn't saved with one gets a
//!   new one, and a saved one whose block doesn't have one any more (or
//!   whose part a feature doesn't add any more) is dropped, and its chunk
//!   marked edited so the save drops it too.
//! - **Placing and breaking:** a voxel set to a different block loses its
//!   block entity, and gets a new one with the block's defaults if the new
//!   block has one. Changing only its state (a furnace turning lit) keeps
//!   it.
//! - **Saving:** whenever a part's component changes to something other
//!   than what's saved, its saved form is written to the block entity's
//!   [`SavedParts`] (which a loaded one starts with), and its chunk is marked
//!   edited (see [`BlockEntityChanged`]), so it's saved. [`ChunkSaver`]
//!   gives the copy of a chunk that's saved, with its block entities in it.
//!
//! In the save, a block entity is its voxel's index, its *kind* (the id of
//! the block that has it) and its parts, each a key, a version and a plain
//! value (see `docs/world-format.md`).

use std::collections::BTreeMap;

use bevy::{
    ecs::system::SystemParam,
    platform::collections::{HashMap, HashSet},
    prelude::*,
};

use crate::{
    content::{BlockState, Content},
    terrain::{ChunkCoord, ChunkMap, RemeshChunks, VoxelChanged},
    voxel::Chunk,
};

mod value;

pub use value::{MAX_DEPTH, Value, read_text, read_value, write_text, write_value};

pub struct BlockEntityPlugin;

impl Plugin for BlockEntityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BlockEntityParts>()
            .init_resource::<BlockEntities>()
            .add_message::<BlockEntityChanged>()
            .add_observer(index_block_entity)
            .add_observer(unindex_block_entity)
            .add_observer(spawn_chunk_block_entities)
            .configure_sets(Update, BlockEntitySystems.after(RemeshChunks))
            .add_systems(
                Update,
                follow_block_changes
                    .in_set(BlockEntitySystems)
                    .before(NoteChanges),
            )
            .configure_sets(Update, NoteChanges.in_set(BlockEntitySystems))
            .register_block_entity_part::<BlockData>();
    }
}

/// Where block entities follow the blocks placed and broken this frame,
/// and changes to their parts are noted. After every edit (they're all
/// before [`RemeshChunks`]); chunks are saved and unloaded after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockEntitySystems;

/// Where changes to parts are noted, after block entities have followed
/// the blocks.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
struct NoteChanges;

/// A voxel's block entity, while its chunk is loaded: an entity under the
/// chunk's, with this and a component for each of its parts.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
#[require(SavedParts)]
pub struct BlockEntity {
    /// The voxel's world position.
    pub pos: IVec3,
}

/// A block entity's parts as they're saved, kept up to date as they
/// change, by key. (Written as they change, rather than when the chunk is
/// saved, since what saves the chunk can't read every component.)
#[derive(Component, Debug, Clone, Default, PartialEq, Eq)]
pub struct SavedParts(pub BTreeMap<String, SavedPart>);

/// A block entity as it's kept in a chunk that isn't spawned, and saved.
#[derive(Reflect, Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockEntityData {
    /// The id of the block that has it. On loading, one whose voxel isn't
    /// that block any more is dropped.
    pub kind: String,
    /// Each part's saved value, by its [`BlockEntityPart::KEY`].
    pub parts: BTreeMap<String, SavedPart>,
}

/// A part of a block entity, saved.
#[derive(Reflect, Debug, Clone, PartialEq, Eq)]
pub struct SavedPart {
    /// The [`BlockEntityPart::VERSION`] that saved it.
    pub version: u32,
    pub value: Value,
}

/// A component a feature adds to block entities, saved with them.
pub trait BlockEntityPart: Component + Sized {
    /// What it's saved under, unique among parts.
    const KEY: &'static str;
    /// Its saved form's version, passed back to [`BlockEntityPart::load`].
    /// Bump it when the form changes.
    const VERSION: u32 = 1;
    /// Whether it's saved. A part that's worked out from the voxel's state
    /// (a machine's ports) isn't: it's made with [`BlockEntityPart::new`]
    /// every time its block entity is, and never written to the save.
    const SAVED: bool = true;

    /// What a new block entity in a voxel of `state` starts with: `None`
    /// if that block's block entities don't have this part.
    fn new(state: BlockState, content: &Content) -> Option<Self>;

    /// Its saved form. `content` names what needs naming (items, say),
    /// since indices into it aren't kept between runs.
    fn save(&self, content: &Content) -> Value;

    /// Reads back what [`BlockEntityPart::save`] saved, at `version`, for a
    /// voxel of `state`. `None` if it can't, and the block entity gets a
    /// new part instead.
    fn load(value: &Value, version: u32, state: BlockState, content: &Content) -> Option<Self>;
}

/// The parts block entities can have.
#[derive(Resource, Default)]
pub struct BlockEntityParts(Vec<PartFns>);

struct PartFns {
    key: &'static str,
    add: fn(&mut EntityCommands, Option<&SavedPart>, BlockState, &Content),
}

impl BlockEntityParts {
    fn has(&self, key: &str) -> bool {
        self.0.iter().any(|part| part.key == key)
    }
}

/// Adds block entity parts to an [`App`].
pub trait RegisterBlockEntityPart {
    /// Makes `P` a part of block entities: the ones whose
    /// [`BlockEntityPart::new`] gives one get it, and it's saved with them.
    fn register_block_entity_part<P: BlockEntityPart>(&mut self) -> &mut Self;
}

impl RegisterBlockEntityPart for App {
    fn register_block_entity_part<P: BlockEntityPart>(&mut self) -> &mut Self {
        let mut parts = self.world_mut().get_resource_or_init::<BlockEntityParts>();
        assert!(
            !parts.has(P::KEY),
            "two block entity parts are `{}`",
            P::KEY
        );
        parts.0.push(PartFns {
            key: P::KEY,
            add: add_part::<P>,
        });
        if P::SAVED {
            self.add_systems(Update, save_changes::<P>.in_set(NoteChanges));
        }
        self
    }
}

fn add_part<P: BlockEntityPart>(
    entity: &mut EntityCommands,
    saved: Option<&SavedPart>,
    state: BlockState,
    content: &Content,
) {
    let loaded = saved.filter(|_| P::SAVED).and_then(|saved| {
        let part = P::load(&saved.value, saved.version, state, content);
        if part.is_none() {
            warn!(
                "a block entity's `{}` (version {}) couldn't be read",
                P::KEY,
                saved.version
            );
        }
        part
    });
    if let Some(part) = loaded.or_else(|| P::new(state, content)) {
        entity.insert(part);
    } else {
        // Saved with one it can't read and doesn't have any more.
        if let Some(saved) = saved {
            warn!(
                "dropped a block entity's `{}` (version {}): its block doesn't have one any more",
                P::KEY,
                saved.version
            );
        }
        entity.entry::<SavedParts>().and_modify(|mut saved| {
            saved.0.remove(P::KEY);
        });
    }
}

/// A table of plain values a block entity keeps, for whatever wants one:
/// a block's `block_entity: { data: { ... } }` gives its block entities
/// one to start with, and scripts keep theirs here.
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq, Eq)]
#[reflect(Component)]
pub struct BlockData(pub BTreeMap<String, Value>);

impl BlockEntityPart for BlockData {
    const KEY: &'static str = "data";

    /// The block's `data` to start with; an empty table for a scripted
    /// block that has a block entity but no `data`, so its script has
    /// somewhere to keep things.
    fn new(state: BlockState, content: &Content) -> Option<Self> {
        let block = content.block_of(state);
        let entity = block.block_entity.as_ref()?;
        match &entity.data {
            Some(data) => Some(Self(data.clone())),
            None => block.script.is_some().then(Self::default),
        }
    }

    fn save(&self, _: &Content) -> Value {
        Value::Table(self.0.clone())
    }

    fn load(value: &Value, version: u32, _: BlockState, _: &Content) -> Option<Self> {
        (version == 1).then(|| value.as_table().cloned().map(Self))?
    }
}

/// A part of the block entity at `pos` changed from what it was saved
/// with, or was added or removed. Its chunk is saved.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEntityChanged {
    pub pos: IVec3,
}

/// Writes the saved form of each part `P` that's changed (or been added)
/// to its block entity's [`SavedParts`], and notes the change.
fn save_changes<P: BlockEntityPart>(
    mut parts: Query<(&BlockEntity, Ref<P>, &mut SavedParts)>,
    mut removed: RemovedComponents<P>,
    mut without: Query<(&BlockEntity, &mut SavedParts), Without<P>>,
    mut changes: MessageWriter<BlockEntityChanged>,
    content: Res<Content>,
) {
    for (entity, part, mut saved) in &mut parts {
        if !part.is_changed() {
            continue;
        }
        let value = SavedPart {
            version: P::VERSION,
            value: part.save(&content),
        };
        // Decided by the value rather than by change ticks: a block entity
        // loaded from its chunk starts with its saved parts, so reading
        // one back isn't a change, while changing it on the frame after
        // its chunk spawned (in `FixedUpdate`, before this) is.
        if saved.0.get(P::KEY) != Some(&value) {
            saved.0.insert(P::KEY.to_string(), value);
            changes.write(BlockEntityChanged { pos: entity.pos });
        }
    }
    for entity in removed.read() {
        if let Ok((entity, mut saved)) = without.get_mut(entity)
            && saved.0.remove(P::KEY).is_some()
        {
            changes.write(BlockEntityChanged { pos: entity.pos });
        }
    }
}

/// Every loaded block entity, by its voxel's world position.
#[derive(Resource, Debug, Default)]
pub struct BlockEntities(HashMap<IVec3, Entity>);

impl BlockEntities {
    /// The block entity of the voxel at `pos`, if it's loaded and has one.
    pub fn get(&self, pos: IVec3) -> Option<Entity> {
        self.0.get(&pos).copied()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn index_block_entity(
    add: On<Add<BlockEntity>>,
    entities: Query<&BlockEntity>,
    mut index: ResMut<BlockEntities>,
) {
    if let Ok(entity) = entities.get(add.entity) {
        index.0.insert(entity.pos, add.entity);
    }
}

fn unindex_block_entity(
    remove: On<Remove<BlockEntity>>,
    entities: Query<&BlockEntity>,
    mut index: ResMut<BlockEntities>,
) {
    if let Ok(entity) = entities.get(remove.entity)
        && index.get(entity.pos) == Some(remove.entity)
    {
        index.0.remove(&entity.pos);
    }
}

/// Spawns the block entity of the voxel at `pos`, of `state`, under the
/// entity of its chunk, with its parts from `saved` or new.
fn spawn_block_entity(
    commands: &mut Commands,
    parts: &BlockEntityParts,
    content: &Content,
    chunk: Entity,
    pos: IVec3,
    state: BlockState,
    saved: Option<&BlockEntityData>,
) -> Entity {
    let id = &content.block_of(state).id;
    // What it was saved with, so reading its parts back isn't a change.
    let kept = saved.map_or_else(BTreeMap::new, |saved| {
        let parts = saved.parts.iter().filter(|(key, _)| parts.has(key));
        parts
            .map(|(key, part)| (key.clone(), part.clone()))
            .collect()
    });
    let mut entity = commands.spawn((
        Name::new(format!("{id} at ({}, {}, {})", pos.x, pos.y, pos.z)),
        BlockEntity { pos },
        SavedParts(kept),
        ChildOf(chunk),
    ));
    for part in &parts.0 {
        let saved = saved.and_then(|saved| saved.parts.get(part.key));
        (part.add)(&mut entity, saved, state, content);
    }
    if let Some(saved) = saved {
        for key in saved.parts.keys().filter(|key| !parts.has(key)) {
            warn!("dropped a block entity's `{key}` at {pos}: no feature adds it any more");
        }
        // Once its parts are in: if it dropped any, they're still in the
        // save, so save its chunk again without them.
        let count = saved.parts.len();
        entity.queue(move |mut entity: EntityWorldMut| {
            let left = entity.get::<SavedParts>().map_or(0, |saved| saved.0.len());
            if left < count {
                entity.world_scope(|world| world.write_message(BlockEntityChanged { pos }));
            }
        });
    }
    entity.id()
}

/// Turns the block entities a chunk was loaded with into entities, as it
/// spawns, and gives the voxels whose block has a block entity but that
/// came without one a new one.
fn spawn_chunk_block_entities(
    add: On<Add<Chunk>>,
    mut chunks: Query<(&mut Chunk, &ChunkCoord)>,
    content: Res<Content>,
    parts: Res<BlockEntityParts>,
    mut commands: Commands,
) {
    let Ok((mut chunk, coord)) = chunks.get_mut(add.entity) else {
        return;
    };
    let chunk = chunk.bypass_change_detection();
    let mut saved = chunk.take_block_entities();
    let origin = coord.origin();
    for (index, &state) in chunk.voxels().iter().enumerate() {
        let block = content.block_of(state);
        if !block.has_block_entity() {
            continue;
        }
        let pos = origin + Chunk::position_of(index);
        let data = saved.remove(&(index as u16)).filter(|data| {
            let kept = data.kind == block.id;
            if !kept {
                warn!(
                    "dropped a `{}` block entity at {pos}: the block there is `{}`",
                    data.kind, block.id
                );
                commands.write_message(BlockEntityChanged { pos });
            }
            kept
        });
        spawn_block_entity(
            &mut commands,
            &parts,
            &content,
            add.entity,
            pos,
            state,
            data.as_ref(),
        );
    }
    for (index, data) in saved {
        let pos = origin + Chunk::position_of(index as usize);
        warn!(
            "dropped a `{}` block entity at {pos}: the block there doesn't have one",
            data.kind
        );
        // It's still in the save: save the chunk again without it.
        commands.write_message(BlockEntityChanged { pos });
    }
}

/// Removes the block entities of voxels set to a different block, and
/// gives the new block's its defaults.
fn follow_block_changes(
    mut changes: MessageReader<VoxelChanged>,
    content: Res<Content>,
    parts: Res<BlockEntityParts>,
    map: Res<ChunkMap>,
    index: Res<BlockEntities>,
    mut commands: Commands,
) {
    // What this frame's changes have done so far, ahead of the index,
    // which only catches up when the commands are applied.
    let mut now: HashMap<IVec3, Option<Entity>> = HashMap::new();
    for change in changes.read() {
        let (before, after) = (content.state(change.before), content.state(change.after));
        if before.block == after.block {
            continue;
        }
        let pos = change.pos;
        let current = now.get(&pos).copied().unwrap_or_else(|| index.get(pos));
        if let Some(entity) = current {
            commands.entity(entity).try_despawn();
        }
        let chunk = map.get(ChunkCoord::split(pos).0);
        let spawned = chunk
            .filter(|_| content.block_of(change.after).has_block_entity())
            .map(|chunk| {
                spawn_block_entity(
                    &mut commands,
                    &parts,
                    &content,
                    chunk,
                    pos,
                    change.after,
                    None,
                )
            });
        now.insert(pos, spawned);
    }
}

/// Reads loaded chunks as they're saved: with their block entities
/// written back in from their components.
#[derive(SystemParam)]
pub struct ChunkSaver<'w, 's> {
    chunks: Query<'w, 's, (&'static Chunk, Option<&'static Children>)>,
    entities: Query<'w, 's, (&'static BlockEntity, &'static SavedParts)>,
    content: Res<'w, Content>,
}

impl ChunkSaver<'_, '_> {
    /// A copy of the chunk of entity `chunk`, block entities and all, to
    /// save. `None` if it isn't a chunk.
    pub fn chunk(&self, chunk: Entity) -> Option<Chunk> {
        let (data, children) = self.chunks.get(chunk).ok()?;
        let mut saved = data.clone();
        let children = children.map(|children| &children[..]).unwrap_or_default();
        let mut seen = HashSet::new();
        for (entity, parts) in self.entities.iter_many(children).flatten() {
            let (_, local) = ChunkCoord::split(entity.pos);
            if !seen.insert(local) {
                continue;
            }
            // Nothing to keep (a wire with every face open): it's made
            // afresh when the chunk loads.
            if parts.0.is_empty() {
                continue;
            }
            let kind = self.content.block_of(saved.at(local)).id.clone();
            let parts = parts.0.clone();
            saved.set_block_entity_at(local, BlockEntityData { kind, parts });
        }
        Some(saved)
    }
}

#[cfg(test)]
mod tests;
