//! Keeps connection properties up to date in the world: a pipe joins the
//! pipes next to it as it's placed, and lets go when one is broken.
//!
//! A block whose `connections` property becomes six sides (see
//! `content::connect`) has each side set from the block next to it,
//! through [`Content::connected`]. That happens for every voxel near one
//! that changed ([`VoxelChanged`], which `fill` in scenarios sends too) and
//! for every connecting block in a chunk that's just loaded, and along the
//! faces of the loaded chunks next to it, so a pipe placed against an
//! unloaded chunk catches up when it loads. A side facing a chunk that
//! isn't loaded keeps its value, so unloading changes nothing.
//!
//! The update goes through [`Voxels::update`]: it isn't an edit, so it
//! wakes no fluid, and each chunk it touches is remeshed once.

use bevy::{platform::collections::HashSet, prelude::*};

use crate::{
    block_edit::BlockEditSystems,
    content::{Content, Direction},
    mining::MiningSystems,
    terrain::{ChunkCoord, ChunkMap, RemeshChunks, VoxelChanged, Voxels},
    voxel::{CHUNK_SIZE, Chunk},
};

pub struct ConnectionsPlugin;

impl Plugin for ConnectionsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Unconnected>().add_systems(
            Update,
            (around_edits, in_loaded_chunks, connect)
                .chain()
                .in_set(ConnectionSystems)
                .after(BlockEditSystems)
                .after(MiningSystems)
                .before(RemeshChunks),
        );
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

/// Each voxel that changed, and the six around it, whose neighbour it is.
fn around_edits(mut changes: MessageReader<VoxelChanged>, mut pending: ResMut<Unconnected>) {
    for change in changes.read() {
        pending.0.insert(change.pos);
        for side in Direction::ALL {
            pending.0.insert(change.pos + side.normal());
        }
    }
}

/// The connecting blocks in each chunk that's just loaded, and in the
/// layer of each loaded chunk next to it that touches it.
fn in_loaded_chunks(
    added: Query<(&ChunkCoord, &Chunk), Added<Chunk>>,
    chunks: Query<&Chunk>,
    map: Res<ChunkMap>,
    content: Res<Content>,
    mut pending: ResMut<Unconnected>,
) {
    let last = CHUNK_SIZE as i32 - 1;
    for (coord, chunk) in &added {
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

/// Sets each waiting voxel's sides from its neighbours.
fn connect(content: Res<Content>, mut pending: ResMut<Unconnected>, mut voxels: Voxels) {
    for pos in pending.0.drain() {
        let Some(state) = voxels.get(pos) else {
            continue;
        };
        if !content.connects(state) {
            continue;
        }
        let joined = content.connected(state, |side| voxels.get(pos + side.normal()));
        voxels.update(pos, joined);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::{ecs::system::RunSystemOnce, time::TimeUpdateStrategy};

    use super::*;
    use crate::{
        content::{BlockState, base, block},
        terrain::{FluidChanged, PropertiesUpdated, VoxelChanged},
    };

    /// Connections on their own, over chunks spawned by hand.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(base().clone())
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                16,
            )))
            .init_resource::<ChunkMap>()
            .add_message::<VoxelChanged>()
            .add_message::<FluidChanged>()
            .add_message::<PropertiesUpdated>()
            .init_resource::<Unconnected>()
            .add_systems(Update, (around_edits, in_loaded_chunks, connect).chain());
        app
    }

    fn spawn(app: &mut App, coord: IVec3, chunk: Chunk) {
        let world = app.world_mut();
        let entity = world.spawn((ChunkCoord(coord), chunk)).id();
        world
            .resource_mut::<ChunkMap>()
            .insert(ChunkCoord(coord), entity);
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
        spawn(&mut app, IVec3::ZERO, Chunk::default());
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
        spawn(&mut app, IVec3::X, next);
        app.update();
        assert_eq!(joined(&mut app, edge), [E, W]);
        assert_eq!(joined(&mut app, edge + IVec3::X), [W]);

        // Unloading it changes nothing: the side keeps its value.
        let entity = app
            .world_mut()
            .resource_mut::<ChunkMap>()
            .remove(ChunkCoord(IVec3::X))
            .unwrap();
        app.world_mut().despawn(entity);
        app.update();
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
        spawn(&mut app, IVec3::X, next);
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
