//! Keeps connection properties up to date in the world: a pipe joins the
//! pipes next to it as it's placed, and lets go when one is broken.
//!
//! A block whose `connections` property becomes six sides (see
//! `content::connect`) has each side set from the block next to it,
//! through [`Content::connected`]. That happens for every voxel near one
//! that changed ([`VoxelChanged`], which `fill` in scenarios sends too),
//! or that the engine or a script updated in more than its sides
//! ([`PropertiesUpdated`]: a cable recoloured, a machine turned), and
//! for every connecting block in a chunk that's just loaded, and along the
//! faces of the loaded chunks next to it, so a pipe placed against an
//! unloaded chunk catches up when it loads. A side facing a chunk that
//! isn't loaded keeps its value, so unloading changes nothing.
//!
//! The update goes through [`Voxels::update`]: it isn't an edit, so it
//! wakes no fluid, and each chunk it touches is remeshed once.
//!
//! A conduit's faces can be blocked (with a wrench, see `wrench`): its
//! block entity's [`BlockedFaces`] names them, and a blocked face is never
//! joined, from either side. Changing them brings that voxel and its
//! neighbours up to date, as an edit would, and so does a conduit with
//! blocked faces loading or going away.

use bevy::{ecs::system::SystemParam, platform::collections::HashSet, prelude::*};

use crate::{
    block_edit::BlockEditSystems,
    block_entity::{BlockEntities, BlockEntity, BlockEntityPart, Value},
    content::{BlockState, Content, Direction},
    meshing::RemeshChunks,
    mining::MiningSystems,
    world::{CHUNK_SIZE, Chunk, ChunkCoord, ChunkMap, PropertiesUpdated, VoxelChanged, Voxels},
};

pub struct ConnectionsPlugin;

impl Plugin for ConnectionsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Unconnected>()
            .init_resource::<LoadedChunks>()
            .add_observer(note_loaded_chunk)
            .register_type::<BlockedFaces>()
            .add_observer(around_unblocked)
            .add_systems(
                Update,
                (
                    around_blocked,
                    around_edits,
                    around_updates,
                    in_loaded_chunks,
                    connect,
                )
                    .chain()
                    .in_set(ConnectionSystems)
                    .after(BlockEditSystems)
                    .after(MiningSystems)
                    .before(RemeshChunks),
            );
    }
}

/// The faces of a conduit that are blocked, which never join anything, as
/// a bit for each [`Direction`] (`1 << side`). Part of a conduit's block
/// entity only while some face is blocked (the wrench adds it, once its
/// plugin registers it, and takes it off when it opens the last), so the
/// many conduits with every face open neither keep nor save one.
#[derive(Component, Reflect, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[reflect(Component)]
pub struct BlockedFaces(pub u8);

impl BlockedFaces {
    /// Every face.
    pub const ALL: u8 = 0b11_1111;

    /// Whether `side` is blocked.
    pub fn has(self, side: Direction) -> bool {
        self.0 & (1 << side as u8) != 0
    }

    /// Blocks `side` if it's open, and opens it if it's blocked.
    pub fn toggle(&mut self, side: Direction) {
        self.0 ^= 1 << side as u8;
    }

    /// Whether no face is blocked.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BlockEntityPart for BlockedFaces {
    const KEY: &'static str = "blocked_faces";

    /// A conduit starts with every face open: none.
    fn new(_: BlockState, _: &Content) -> Option<Self> {
        None
    }

    fn save(&self, _: &Content) -> Value {
        Value::Int(self.0.into())
    }

    fn load(value: &Value, version: u32, state: BlockState, content: &Content) -> Option<Self> {
        if !content.block_of(state).is_conduit() {
            return None;
        }
        let mask = value.as_int().filter(|_| version == 1)?;
        u8::try_from(mask)
            .ok()
            .filter(|&mask| mask & !Self::ALL == 0)
            .map(Self)
    }
}

/// The blocked faces of conduits, by position.
#[derive(SystemParam)]
pub struct Blocked<'w, 's> {
    index: Option<Res<'w, BlockEntities>>,
    faces: Query<'w, 's, &'static BlockedFaces>,
}

impl Blocked<'_, '_> {
    /// The blocked faces of the conduit at `pos`: none if it isn't loaded
    /// or isn't a conduit.
    pub fn at(&self, pos: IVec3) -> BlockedFaces {
        self.index
            .as_ref()
            .and_then(|index| index.get(pos))
            .and_then(|entity| self.faces.get(entity).ok())
            .copied()
            .unwrap_or_default()
    }
}

/// Where connection properties are brought up to date: after this frame's
/// edits, and before the chunks are relit and remeshed, so a pipe placed
/// is drawn joined on the frame it's placed.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConnectionSystems;

/// Voxels whose connections may be out of date, for [`connect`].
#[derive(Resource, Debug, Default)]
struct Unconnected(HashSet<IVec3>);

impl Unconnected {
    /// `pos`, and the six around it, whose neighbour it is.
    fn around(&mut self, pos: IVec3) {
        self.0.insert(pos);
        for side in Direction::ALL {
            self.0.insert(pos + side.normal());
        }
    }
}

/// Each voxel that changed, and the six around it, whose neighbour it is.
fn around_edits(mut changes: MessageReader<VoxelChanged>, mut pending: ResMut<Unconnected>) {
    for change in changes.read() {
        pending.around(change.pos);
    }
}

/// Each voxel updated in more than its sides (a cable recoloured or a
/// machine turned, by a script's `set_state`), and the six around it: what
/// it offers them may have changed. One updated in its sides alone, as
/// [`connect`] does, offers its neighbours the same, so only it is checked
/// again (a script may have set its sides), which comes out unchanged
/// after [`connect`]'s own updates, so this settles.
fn around_updates(
    mut updates: MessageReader<PropertiesUpdated>,
    content: Res<Content>,
    mut pending: ResMut<Unconnected>,
) {
    let bare = |state| content.connected_unless(state, |_| true, |_| None);
    for update in updates.read() {
        if bare(update.before) == bare(update.after) {
            pending.0.insert(update.pos);
        } else {
            pending.around(update.pos);
        }
    }
}

/// Each conduit whose blocked faces changed, or that's new with some
/// blocked (loaded from the save), and the six around it.
fn around_blocked(
    changed: Query<(&BlockEntity, Ref<BlockedFaces>), Changed<BlockedFaces>>,
    mut pending: ResMut<Unconnected>,
) {
    for (entity, faces) in &changed {
        if !faces.is_added() || !faces.is_empty() {
            pending.around(entity.pos);
        }
    }
}

/// Each conduit with blocked faces that's gone (broken, replaced or
/// unloaded), and the six around it: its faces were put right as it
/// changed, but with its old blocked faces, which only go once its block
/// entity does, after that.
fn around_unblocked(
    remove: On<Remove<BlockedFaces>>,
    entities: Query<(&BlockEntity, &BlockedFaces)>,
    mut pending: ResMut<Unconnected>,
) {
    if let Ok((entity, faces)) = entities.get(remove.entity)
        && !faces.is_empty()
    {
        pending.around(entity.pos);
    }
}

/// Chunks spawned since [`in_loaded_chunks`] last ran, noted as they're
/// added: an `Added<Chunk>` query would look at every loaded chunk on
/// every frame.
#[derive(Resource, Debug, Default)]
struct LoadedChunks(Vec<Entity>);

fn note_loaded_chunk(add: On<Add<Chunk>>, mut loaded: ResMut<LoadedChunks>) {
    loaded.0.push(add.entity);
}

/// The connecting blocks in each chunk that's just loaded, and in the
/// layer of each loaded chunk next to it that touches it.
fn in_loaded_chunks(
    mut loaded: ResMut<LoadedChunks>,
    added: Query<&ChunkCoord>,
    chunks: Query<&Chunk>,
    map: Res<ChunkMap>,
    content: Res<Content>,
    mut pending: ResMut<Unconnected>,
) {
    if loaded.0.is_empty() {
        return;
    }
    let last = CHUNK_SIZE as i32 - 1;
    for entity in loaded.0.drain(..) {
        let (Ok(coord), Ok(chunk)) = (added.get(entity), chunks.get(entity)) else {
            continue;
        };
        let origin = coord.origin();
        for (index, &state) in chunk.voxels().iter().enumerate() {
            if content.connects(state) {
                pending.0.insert(origin + Chunk::position_of(index));
            }
        }
        for side in Direction::ALL {
            let next = ChunkCoord(coord.0 + side.normal());
            let Some(neighbour) = map.get(next).and_then(|e| chunks.get(e).ok()) else {
                continue;
            };
            for a in 0..=last {
                for b in 0..=last {
                    let local = face_voxel(side.opposite(), a, b, last);
                    if content.connects(neighbour.at(local)) {
                        pending.0.insert(next.origin() + local);
                    }
                }
            }
        }
    }
}

/// The voxel on a chunk's `side` at `a`, `b` across it.
pub(crate) fn face_voxel(side: Direction, a: i32, b: i32, last: i32) -> IVec3 {
    let n = side.normal();
    let edge = |v: i32| if v > 0 { last } else { 0 };
    match n {
        IVec3 { x: 1 | -1, .. } => IVec3::new(edge(n.x), a, b),
        IVec3 { y: 1 | -1, .. } => IVec3::new(a, edge(n.y), b),
        _ => IVec3::new(a, b, edge(n.z)),
    }
}

/// Sets each waiting voxel's sides from its neighbours, leaving out the
/// faces blocked on either side.
fn connect(
    content: Res<Content>,
    mut pending: ResMut<Unconnected>,
    blocked: Blocked,
    mut voxels: Voxels,
) {
    for pos in pending.0.drain() {
        let Some(state) = voxels.get(pos) else {
            continue;
        };
        if !content.connects(state) {
            continue;
        }
        let own = blocked.at(pos);
        let joined = content.connected_unless(
            state,
            |side| own.has(side) || blocked.at(pos + side.normal()).has(side.opposite()),
            |side| voxels.get(pos + side.normal()),
        );
        voxels.update(pos, joined);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::{
        content::{BlockState, base, block},
        testing::{chunk_app, insert_chunk, unload_chunk},
    };

    /// Connections on their own, over chunks spawned by hand.
    fn app() -> App {
        let mut app = chunk_app(base().clone(), Duration::from_millis(16));
        app.init_resource::<Unconnected>()
            .init_resource::<LoadedChunks>()
            .add_observer(note_loaded_chunk)
            .add_systems(
                Update,
                (around_edits, around_updates, in_loaded_chunks, connect).chain(),
            );
        app
    }

    fn set(app: &mut App, pos: IVec3, state: BlockState) {
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| assert!(voxels.set(pos, state)))
            .unwrap();
        app.update();
    }

    fn get(app: &mut App, pos: IVec3) -> BlockState {
        app.world_mut()
            .run_system_once(move |voxels: Voxels| voxels.get(pos).unwrap())
            .unwrap()
    }

    fn pipe() -> BlockState {
        base().default_state("defaria:pipe").unwrap()
    }

    /// The sides of the block at `pos` that are joined.
    fn joined(app: &mut App, pos: IVec3) -> Vec<Direction> {
        let state = get(app, pos);
        Direction::ALL
            .into_iter()
            .filter(|&side| base().joined(state, side))
            .collect()
    }

    fn updates(app: &mut App) -> Vec<PropertiesUpdated> {
        app.world_mut()
            .run_system_once(|mut updates: MessageReader<PropertiesUpdated>| {
                updates.read().copied().collect::<Vec<_>>()
            })
            .unwrap()
    }

    /// One loaded chunk of air.
    fn one_chunk() -> App {
        let mut app = app();
        insert_chunk(&mut app, IVec3::ZERO, Chunk::default());
        app.update();
        app
    }

    const E: Direction = Direction::East;
    const W: Direction = Direction::West;
    const N: Direction = Direction::North;
    const S: Direction = Direction::South;

    #[test]
    fn a_line_of_pipes_joins_each_to_the_next() {
        let mut app = one_chunk();
        for x in 2..6 {
            set(&mut app, IVec3::new(x, 4, 4), pipe());
        }
        assert_eq!(joined(&mut app, IVec3::new(2, 4, 4)), [E]);
        assert_eq!(joined(&mut app, IVec3::new(3, 4, 4)), [E, W]);
        assert_eq!(joined(&mut app, IVec3::new(4, 4, 4)), [E, W]);
        assert_eq!(joined(&mut app, IVec3::new(5, 4, 4)), [W]);

        // Breaking the middle one lets go of it on both sides.
        set(&mut app, IVec3::new(3, 4, 4), BlockState::AIR);
        assert_eq!(joined(&mut app, IVec3::new(2, 4, 4)), []);
        assert_eq!(joined(&mut app, IVec3::new(4, 4, 4)), [E]);
        assert_eq!(joined(&mut app, IVec3::new(5, 4, 4)), [W]);
    }

    #[test]
    fn a_corner_joins_both_ways() {
        let mut app = one_chunk();
        set(&mut app, IVec3::new(4, 4, 4), pipe());
        set(&mut app, IVec3::new(5, 4, 4), pipe());
        set(&mut app, IVec3::new(4, 4, 3), pipe());
        assert_eq!(joined(&mut app, IVec3::new(4, 4, 4)), [N, E]);
        assert_eq!(joined(&mut app, IVec3::new(5, 4, 4)), [W]);
        assert_eq!(joined(&mut app, IVec3::new(4, 4, 3)), [S]);
        // And up: a pipe on top joins the corner's up side.
        set(&mut app, IVec3::new(4, 5, 4), pipe());
        assert_eq!(joined(&mut app, IVec3::new(4, 4, 4)), [Direction::Up, N, E]);
        assert_eq!(joined(&mut app, IVec3::new(4, 5, 4)), [Direction::Down]);
    }

    #[test]
    fn different_rules_dont_join() {
        let mut app = one_chunk();
        let wire = base().default_state("defaria:wire").unwrap();
        set(&mut app, IVec3::new(4, 4, 4), pipe());
        set(&mut app, IVec3::new(5, 4, 4), wire);
        set(&mut app, IVec3::new(6, 4, 4), wire);
        set(&mut app, IVec3::new(3, 4, 4), block("stone"));
        assert_eq!(joined(&mut app, IVec3::new(4, 4, 4)), []);
        assert_eq!(joined(&mut app, IVec3::new(5, 4, 4)), [E]);
        assert_eq!(joined(&mut app, IVec3::new(6, 4, 4)), [W]);
    }

    /// The engine's values win over whatever state a block is set to, and
    /// updating them is no edit: no `VoxelChanged`, and the chunk's change
    /// detection is left to the remesh marking.
    #[test]
    fn updates_are_sent_as_properties_not_edits() {
        let mut app = one_chunk();
        set(&mut app, IVec3::new(4, 4, 4), pipe());
        let stale = base()
            .block_state("defaria:pipe", &[("north", "true"), ("up", "true")])
            .unwrap();
        set(&mut app, IVec3::new(5, 4, 4), stale);
        assert_eq!(joined(&mut app, IVec3::new(5, 4, 4)), [W]);
        let updates = updates(&mut app);
        // Both pipes were updated, once each.
        assert_eq!(updates.len(), 2, "{updates:?}");
        assert!(
            updates
                .iter()
                .any(|u| u.pos == IVec3::new(5, 4, 4) && u.before == stale)
        );
        // The edits are only what was set, never what the engine joined.
        let edits = app
            .world_mut()
            .run_system_once(|mut changes: MessageReader<VoxelChanged>| {
                changes.read().map(|c| c.after).collect::<Vec<_>>()
            })
            .unwrap();
        assert!(!edits.is_empty());
        assert!(
            edits.iter().all(|&after| after == stale || after == pipe()),
            "{edits:?}"
        );
    }

    /// An update that changes what a block offers its neighbours, such as
    /// a script recolouring a cable, lets go of them and joins anew; one
    /// to a block's sides alone is put right.
    #[test]
    fn updates_that_change_what_joins_reconnect_around_them() {
        let content = crate::content::test_mod(&[
            ("models/m.json5", "{ faces: [] }"),
            (
                "networks/power.json5",
                r#"{ name: "Power", solver: "energy" }"#,
            ),
            (
                "blocks/cable.json5",
                r#"{ model: "m", properties: { color: ["red", "blue"], connect: { connections: true } },
                     conduit: { network: "power", group: "{color}_cable", capacity: 0, throughput: 5 } }"#,
            ),
        ]);
        assert!(content.errors.is_empty(), "{:?}", content.errors);
        let cable = |color, sides: &[Direction]| {
            let mut values = vec![("color", color)];
            values.extend(sides.iter().map(|side| (side.name(), "true")));
            content.block_state("test:cable", &values).unwrap()
        };
        let mut app = app();
        app.insert_resource(content.clone());
        insert_chunk(&mut app, IVec3::ZERO, Chunk::default());
        app.update();
        let (a, b) = (IVec3::new(4, 4, 4), IVec3::new(5, 4, 4));
        set(&mut app, a, cable("red", &[]));
        set(&mut app, b, cable("red", &[]));
        assert_eq!(get(&mut app, a), cable("red", &[E]));
        assert_eq!(get(&mut app, b), cable("red", &[W]));

        let update = |app: &mut App, pos: IVec3, state: BlockState| {
            app.world_mut()
                .run_system_once(move |mut voxels: Voxels| assert!(voxels.update(pos, state)))
                .unwrap();
            app.update();
            app.update();
        };
        // Recoloured blue, still joined west: both let go.
        update(&mut app, b, cable("blue", &[W]));
        assert_eq!(get(&mut app, a), cable("red", &[]));
        assert_eq!(get(&mut app, b), cable("blue", &[]));
        // Back to red: they join again.
        update(&mut app, b, cable("red", &[]));
        assert_eq!(get(&mut app, a), cable("red", &[E]));
        assert_eq!(get(&mut app, b), cable("red", &[W]));
        // Sides set by hand are put right.
        update(&mut app, b, cable("red", &[W, N]));
        assert_eq!(get(&mut app, b), cable("red", &[W]));
    }

    #[test]
    fn a_pipe_at_a_chunk_border_joins_once_the_neighbour_loads() {
        let mut app = one_chunk();
        let edge = IVec3::new(CHUNK_SIZE as i32 - 1, 4, 4);
        set(&mut app, edge, pipe());
        set(&mut app, edge - IVec3::X, pipe());
        // Nothing is loaded to the east: that side stays as it is.
        assert_eq!(joined(&mut app, edge), [W]);

        // The chunk to the east loads with a pipe against it.
        let mut next = Chunk::default();
        next.set_at(IVec3::new(0, 4, 4), pipe());
        insert_chunk(&mut app, IVec3::X, next);
        app.update();
        assert_eq!(joined(&mut app, edge), [E, W]);
        assert_eq!(joined(&mut app, edge + IVec3::X), [W]);

        // Unloading it changes nothing: the side keeps its value.
        unload_chunk(&mut app, IVec3::X);
        assert_eq!(joined(&mut app, edge), [E, W]);
    }

    /// A chunk loading with stale connections (saved before its neighbour
    /// changed) is put right, along with the neighbour's side.
    #[test]
    fn a_loaded_chunks_own_pipes_are_brought_up_to_date() {
        let mut app = one_chunk();
        let stale = base()
            .block_state("defaria:pipe", &[("up", "true")])
            .unwrap();
        let mut next = Chunk::default();
        next.set_at(IVec3::new(1, 4, 4), stale);
        next.set_at(IVec3::new(2, 4, 4), stale);
        insert_chunk(&mut app, IVec3::X, next);
        app.update();
        let origin = IVec3::X * CHUNK_SIZE as i32;
        assert_eq!(joined(&mut app, origin + IVec3::new(1, 4, 4)), [E]);
        assert_eq!(joined(&mut app, origin + IVec3::new(2, 4, 4)), [W]);
    }

    #[test]
    fn face_voxels_are_on_the_face() {
        let last = CHUNK_SIZE as i32 - 1;
        for side in Direction::ALL {
            for (a, b) in [(0, 0), (3, 7), (last, last)] {
                let local = face_voxel(side, a, b, last);
                // One step out of the face leaves the chunk.
                let out = local + side.normal();
                assert!(
                    out.min_element() < 0 || out.max_element() > last,
                    "{side:?} {local}"
                );
                assert!(local.min_element() >= 0 && local.max_element() <= last);
            }
        }
    }
}
