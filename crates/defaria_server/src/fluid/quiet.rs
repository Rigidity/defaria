//! Fluid that has stopped flowing is drawn as still: its faces' texture
//! scrolls along the flow (see `meshing::scroll`, in the client), and the
//! flow on a face only changes when its chunk is redrawn. A chunk is
//! redrawn when a voxel's drawn height changes, but fluid can go on
//! levelling off by a few units without one, and settle still showing the
//! flow it had at its last redraw. So a chunk is marked changed once more
//! when the flow in it has been quiet for [`QUIET`], which redraws it (and
//! sends it to clients again).

use bevy::{platform::collections::HashMap, prelude::*};

use defaria_core::world::{Chunk, ChunkCoord, ChunkMap, FluidChanged};

/// Remeshes chunks whose fluid has stopped flowing, so their faces stop
/// scrolling.
pub struct QuietPlugin;

impl Plugin for QuietPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Unsettled>()
            .add_systems(Update, remesh_settled);
    }
}

/// How long fluid in a chunk goes without flowing before the chunk is
/// remeshed: a few steps of the slowest base fluid (lava, 2 a second), so
/// a chunk still flowing isn't remeshed again and again.
pub const QUIET: f32 = 1.5;

/// Chunks fluid has flowed in since they were last remeshed for it, and
/// how long ago it last did.
#[derive(Resource, Debug, Default)]
pub struct Unsettled(HashMap<ChunkCoord, f32>);

/// Notes the chunks flow changes fluid in (and those within
/// [`defaria_core::world::MARGIN`] of it, whose faces' flow it decides too), and
/// remeshes each once it has been quiet for [`QUIET`].
fn remesh_settled(
    mut changes: MessageReader<FluidChanged>,
    time: Res<Time>,
    map: Res<ChunkMap>,
    mut chunks: Query<&mut Chunk>,
    mut unsettled: ResMut<Unsettled>,
) {
    for change in changes.read().filter(|change| change.by_flow) {
        for coord in ChunkCoord::within(change.pos, defaria_core::world::MARGIN) {
            unsettled.0.insert(coord, 0.0);
        }
    }
    let delta = time.delta_secs();
    unsettled.0.retain(|&coord, quiet| {
        *quiet += delta;
        if *quiet < QUIET {
            return true;
        }
        if let Some(mut chunk) = map
            .get(coord)
            .and_then(|entity| chunks.get_mut(entity).ok())
        {
            chunk.set_changed();
        }
        false
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chunk flow changed fluid in is remeshed once, after its fluid has
    /// been quiet for [`QUIET`], and not while flow keeps changing it; so
    /// is the chunk across the edge from a changed voxel near it, whose
    /// faces' flow the voxel's amount decides too.
    #[test]
    fn settled_chunks_are_remeshed_once() {
        use bevy::time::TimeUpdateStrategy;
        use defaria_core::world::CHUNK_SIZE;
        use std::time::Duration;

        /// How many times each chunk was marked changed, by coordinate.
        #[derive(Resource, Default)]
        struct Remeshed(HashMap<IVec3, usize>);

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, QuietPlugin))
            .add_message::<FluidChanged>()
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .init_resource::<Remeshed>()
            .add_systems(
                Last,
                |chunks: Query<&ChunkCoord, Changed<Chunk>>, mut remeshed: ResMut<Remeshed>| {
                    for coord in &chunks {
                        *remeshed.0.entry(coord.0).or_default() += 1;
                    }
                },
            );
        let mut map = ChunkMap::default();
        for coord in [IVec3::ZERO, IVec3::X, IVec3::NEG_X] {
            let coord = ChunkCoord(coord);
            map.insert(coord, app.world_mut().spawn((coord, Chunk::default())).id());
        }
        app.world_mut().insert_resource(map);
        app.update();
        let remeshed = |app: &App, x: i32| {
            let counts = &app.world().resource::<Remeshed>().0;
            counts.get(&IVec3::new(x, 0, 0)).copied().unwrap_or(0)
        };
        app.world_mut().resource_mut::<Remeshed>().0.clear();
        // Two in from the +X edge of chunk 0: it decides the flow at the
        // corners of chunk 1's faces along that edge.
        let edge = IVec3::new(CHUNK_SIZE as i32 - 2, 5, 5);
        let flow = |app: &mut App, by_flow| {
            app.world_mut().write_message(FluidChanged {
                pos: edge,
                by_flow,
                before: None,
                after: None,
            });
        };
        // Flowing every tenth of a second for two seconds: never quiet.
        for _ in 0..20 {
            flow(&mut app, true);
            app.update();
        }
        assert_eq!(remeshed(&app, 0), 0);
        // Quiet, but not for long enough yet.
        for _ in 0..(QUIET * 10.0) as usize - 2 {
            app.update();
        }
        assert_eq!(remeshed(&app, 0), 0);
        // Then remeshed once: the chunk and the one across its edge, not
        // the one on its far side.
        for _ in 0..30 {
            app.update();
        }
        assert_eq!(
            [remeshed(&app, 0), remeshed(&app, 1), remeshed(&app, -1)],
            [1, 1, 0]
        );
        // Something else setting fluid (a player, a pump) isn't counted:
        // it starts flow if the fluid can move, and flow's changes are.
        flow(&mut app, false);
        for _ in 0..30 {
            app.update();
        }
        assert_eq!(remeshed(&app, 0), 1);
        // A chunk that unloads while it waits is let go.
        flow(&mut app, true);
        app.update();
        let gone = app
            .world_mut()
            .resource_mut::<ChunkMap>()
            .remove(ChunkCoord(IVec3::X))
            .unwrap();
        app.world_mut().despawn(gone);
        for _ in 0..30 {
            app.update();
        }
        assert_eq!(remeshed(&app, 0), 2);
        assert!(app.world().resource::<Unsettled>().0.is_empty());
    }
}
