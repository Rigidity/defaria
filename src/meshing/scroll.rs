//! Flowing fluid faces move their texture along the flow; still ones
//! don't. A flow map: every corner of a fluid face has a flow vector, and
//! the shader blends them across the face, so the flow is continuous from
//! one tile to the next.
//!
//! The mesher works out which way each fluid voxel flows ([`Flow`]) from
//! the amounts it holds, as flow itself does: down while it falls,
//! otherwise downhill toward neighbours of the same fluid holding enough
//! less that fluid moves between them. [`face_vector`] turns that into a
//! flow along each of its faces, and [`corner`] averages it over the
//! voxels sharing a face's corner, as `FluidTop` averages corner heights,
//! so two faces side by side agree along their shared edge. Fluid faces'
//! texture coordinates come from where they are ([`uv_at`]), wrapping
//! every block, so equal shifts line up from tile to tile.
//!
//! The vectors are in the mesh ([`crate::meshing::ATTRIBUTE_FLOW`]), so
//! flow costs no remeshing to animate, and the shader samples twice only
//! on faces with [`FLOWING`] set. A face's flow only changes when its
//! chunk is remeshed. A chunk is remeshed when a voxel's drawn height
//! changes, but fluid can go on levelling off by a few units without one,
//! and settle still showing the flow it had at its last remesh. So
//! [`ScrollPlugin`] remeshes a chunk once more when the flow in it has
//! been quiet for [`QUIET`].

use bevy::{platform::collections::HashMap, prelude::*};

use crate::world::{Chunk, ChunkCoord, ChunkMap, FluidChanged};

/// Remeshes chunks whose fluid has stopped flowing, so their faces stop
/// scrolling.
pub struct ScrollPlugin;

impl Plugin for ScrollPlugin {
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
/// [`crate::world::MARGIN`] of it, whose faces' flow it decides too), and
/// remeshes each once it has been quiet for [`QUIET`].
fn remesh_settled(
    mut changes: MessageReader<FluidChanged>,
    time: Res<Time>,
    map: Res<ChunkMap>,
    mut chunks: Query<&mut Chunk>,
    mut unsettled: ResMut<Unsettled>,
) {
    for change in changes.read().filter(|change| change.by_flow) {
        for coord in ChunkCoord::within(change.pos, crate::world::MARGIN) {
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

/// Set in a fluid face's layer: its texture coordinates come from where
/// it is ([`uv_at`]) and wrap every block.
pub const WRAPPED: u32 = 1 << 29;
/// Set in a fluid face's layer when any of its corners flows: only these
/// faces pay for the flow map's two samples.
pub const FLOWING: u32 = 1 << 28;
/// The bits of a face's layer that hold the texture's index.
pub const TEXTURE_BITS: u32 = (1 << 24) - 1;

// Every texture's index fits under the flags, and they under the back
// face's.
const _: () = assert!(crate::content::MAX_TILES as u32 <= TEXTURE_BITS + 1);
const _: () = assert!(WRAPPED < crate::meshing::BACK_FACE && FLOWING > TEXTURE_BITS);

/// Which way a fluid voxel flows.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Flow {
    /// It falls into the voxel below: its sides flow down.
    pub falling: bool,
    /// Downhill, across the ground (no `y`): which way it flows across.
    /// Zero on level fluid.
    pub downhill: Vec3,
}

/// The least flow that counts: less is level, or a flow that runs
/// straight through a face, with nothing along it.
const LEVEL: f32 = 1e-3;

/// Which way a voxel flowing `flow` moves along its face facing `normal`,
/// in the world: a unit vector, or zero where the face is still.
///
/// A top flows downhill. A side flows down while the fluid falls, or
/// across along the downhill part in its plane, and down where the fluid
/// flows out through it with nothing along it (the front of a spreading
/// sheet). A bottom is still.
pub fn face_vector(flow: Flow, normal: Vec3) -> Vec3 {
    let along = if normal.y > 0.5 {
        flow.downhill
    } else if normal.y < -0.5 {
        Vec3::ZERO
    } else if flow.falling {
        Vec3::NEG_Y
    } else {
        let across = flow.downhill - normal * flow.downhill.dot(normal);
        if across.length() > LEVEL {
            across
        } else if flow.downhill.dot(normal) > LEVEL {
            Vec3::NEG_Y
        } else {
            Vec3::ZERO
        }
    };
    if along.length() > LEVEL {
        along.normalize()
    } else {
        Vec3::ZERO
    }
}

/// The flow at a corner of a face: the average of the face vectors of the
/// voxels sharing the corner in the face's plane, as `FluidTop` averages
/// corner heights. `around(offset)` is the face vector of the voxel at
/// `offset` along the plane's axes ([`plane_axes`]) from the face's own,
/// or `None` if it isn't the same fluid; `toward` is which way the corner
/// is, each -1 or 1. The voxel diagonally across counts only if either
/// voxel beside both holds the fluid too, so fluid that only touches at
/// the corner doesn't pull on it. Still voxels count as zero, so flow
/// fades out toward still fluid.
pub fn corner(mut around: impl FnMut(IVec2) -> Option<Vec3>, toward: IVec2) -> Vec3 {
    let here = around(IVec2::ZERO);
    let beside = [
        around(IVec2::new(toward.x, 0)),
        around(IVec2::new(0, toward.y)),
    ];
    let diagonal = if beside.iter().any(Option::is_some) {
        around(toward)
    } else {
        None
    };
    let (mut sum, mut count) = (Vec3::ZERO, 0);
    for vector in [here, beside[0], beside[1], diagonal].into_iter().flatten() {
        sum += vector;
        count += 1;
    }
    if count == 0 {
        Vec3::ZERO
    } else {
        sum / count as f32
    }
}

/// The two axes along the plane of a face facing `normal`, the way its
/// texture coordinates run ([`uv_at`]): X and Z on tops and bottoms, and
/// across then up on sides.
pub fn plane_axes(normal: Vec3) -> (IVec3, IVec3) {
    if normal.y.abs() > 0.5 {
        (IVec3::X, IVec3::Z)
    } else {
        (across(normal), IVec3::Y)
    }
}

/// The horizontal axis along a side facing `normal`, a quarter turn from
/// it.
fn across(normal: Vec3) -> IVec3 {
    IVec3::new(-normal.z.round() as i32, 0, normal.x.round() as i32)
}

/// Which corner of a face a vertex of the block's model (each coordinate 0
/// or 1) is: -1 or 1 along each of [`plane_axes`].
pub fn corner_toward(normal: Vec3, vertex: Vec3) -> IVec2 {
    let (a, b) = plane_axes(normal);
    let side = |axis: IVec3| {
        if (vertex - Vec3::splat(0.5)).dot(axis.as_vec3()) > 0.0 {
            1
        } else {
            -1
        }
    };
    IVec2::new(side(a), side(b))
}

/// A fluid face's texture coordinates at `pos`: where it is along its
/// plane, wrapping every block, so the texture lines up from one tile to
/// the next. Tops and bottoms run along X and Z; sides across, and down
/// (an image's rows go down).
pub fn uv_at(normal: Vec3, pos: Vec3) -> Vec2 {
    to_uv(normal, pos)
}

/// A vector along a fluid face, in its texture coordinates ([`uv_at`]).
pub fn to_uv(normal: Vec3, along: Vec3) -> Vec2 {
    if normal.y.abs() > 0.5 {
        along.xz()
    } else {
        Vec2::new(along.dot(across(normal).as_vec3()), -along.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-4
    }

    #[test]
    fn still_fluid_has_no_flow() {
        for normal in [Vec3::Y, Vec3::X, Vec3::NEG_Z, Vec3::NEG_Y] {
            assert_eq!(face_vector(Flow::default(), normal), Vec3::ZERO);
        }
    }

    #[test]
    fn a_top_flows_downhill() {
        let flow = Flow {
            falling: false,
            downhill: Vec3::new(0.2, 0.0, 0.2),
        };
        assert!(close(
            face_vector(flow, Vec3::Y),
            Vec3::new(1.0, 0.0, 1.0).normalize()
        ));
        let falling = Flow {
            falling: true,
            downhill: Vec3::ZERO,
        };
        assert_eq!(face_vector(falling, Vec3::Y), Vec3::ZERO);
        // A bottom is still.
        assert_eq!(face_vector(flow, Vec3::NEG_Y), Vec3::ZERO);
    }

    #[test]
    fn sides_flow_down_while_falling_and_across_along_the_flow() {
        let flow = |falling, downhill| Flow { falling, downhill };
        assert_eq!(face_vector(flow(true, Vec3::Z), Vec3::X), Vec3::NEG_Y);
        assert_eq!(face_vector(flow(false, Vec3::Z), Vec3::X), Vec3::Z);
        // Out through the face: it pours down it. Away from it: still.
        assert_eq!(face_vector(flow(false, Vec3::X), Vec3::X), Vec3::NEG_Y);
        assert_eq!(face_vector(flow(false, Vec3::NEG_X), Vec3::X), Vec3::ZERO);
    }

    /// The voxels of the same fluid around a corner, by offset: a grid of
    /// face vectors, `None` where it isn't the fluid.
    fn grid(cells: &[(IVec2, Vec3)]) -> impl Fn(IVec2) -> Option<Vec3> + '_ {
        move |at| cells.iter().find(|(p, _)| *p == at).map(|&(_, v)| v)
    }

    /// Two faces side by side get the same flow at both corners they
    /// share, from either side, as each averages the same voxels.
    #[test]
    fn neighbours_agree_on_shared_corners() {
        // A 3×3 patch flowing different ways, every voxel the same fluid.
        let vectors = |x: i32, z: i32| Vec3::new(x as f32, 0.0, (z * z) as f32 - 0.5).normalize();
        let world: Vec<(IVec2, Vec3)> = (-1..=2)
            .flat_map(|x| (-1..=1).map(move |z| (IVec2::new(x, z), vectors(x, z))))
            .collect();
        let from = |voxel: IVec2| {
            let world = world.clone();
            move |offset: IVec2| {
                world
                    .iter()
                    .find(|(p, _)| *p == voxel + offset)
                    .map(|&(_, v)| v)
            }
        };
        // Voxel (0, 0)'s +X corners are voxel (1, 0)'s -X corners.
        for z in [-1, 1] {
            let left = corner(from(IVec2::ZERO), IVec2::new(1, z));
            let right = corner(from(IVec2::X), IVec2::new(-1, z));
            assert!(close(left, right), "{left} vs {right}");
        }
    }

    #[test]
    fn still_water_has_zero_corners_and_fades_flow() {
        let still = [
            (IVec2::ZERO, Vec3::ZERO),
            (IVec2::X, Vec3::ZERO),
            (IVec2::Y, Vec3::ZERO),
            (IVec2::ONE, Vec3::ZERO),
        ];
        assert_eq!(corner(grid(&still), IVec2::ONE), Vec3::ZERO);
        // One of four flowing: a quarter of its flow at the corner.
        let mut one = still;
        one[3].1 = Vec3::X;
        assert!(close(corner(grid(&one), IVec2::ONE), Vec3::X / 4.0));
    }

    /// The voxel diagonally across a corner counts only when it's joined
    /// to this one through a voxel beside both.
    #[test]
    fn a_corner_touch_doesnt_count() {
        let touching = [(IVec2::ZERO, Vec3::ZERO), (IVec2::ONE, Vec3::X)];
        assert_eq!(corner(grid(&touching), IVec2::ONE), Vec3::ZERO);
        let joined = [
            (IVec2::ZERO, Vec3::ZERO),
            (IVec2::X, Vec3::ZERO),
            (IVec2::ONE, Vec3::X),
        ];
        assert!(close(corner(grid(&joined), IVec2::ONE), Vec3::X / 3.0));
        // Other blocks don't count at all.
        assert!(close(
            corner(grid(&[(IVec2::ZERO, Vec3::Z)]), IVec2::ONE),
            Vec3::Z
        ));
    }

    /// Texture coordinates come from the position, so a face's and its
    /// neighbour's meet at the same values on their shared edge, and a
    /// flow along a face maps the same way.
    #[test]
    fn fluid_uvs_follow_the_world() {
        assert_eq!(
            uv_at(Vec3::Y, Vec3::new(3.0, 5.0, 7.0)),
            Vec2::new(3.0, 7.0)
        );
        // A side: across it, and down it.
        for normal in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
            let (a, b) = plane_axes(normal);
            assert_eq!(a.as_vec3().dot(normal), 0.0, "{normal}");
            assert_eq!(b, IVec3::Y);
            let pos = Vec3::new(2.0, 4.0, 6.0);
            let step = uv_at(normal, pos + a.as_vec3()) - uv_at(normal, pos);
            assert_eq!(step, Vec2::X, "{normal}");
            assert_eq!(to_uv(normal, Vec3::NEG_Y), Vec2::Y, "{normal}");
        }
        // The corner a model vertex is.
        assert_eq!(
            corner_toward(Vec3::Y, Vec3::new(1.0, 1.0, 0.0)),
            IVec2::new(1, -1)
        );
        assert_eq!(
            corner_toward(Vec3::X, Vec3::new(1.0, 0.0, 1.0)),
            IVec2::new(1, -1)
        );
    }

    /// A chunk flow changed fluid in is remeshed once, after its fluid has
    /// been quiet for [`QUIET`], and not while flow keeps changing it; so
    /// is the chunk across the edge from a changed voxel near it, whose
    /// faces' flow the voxel's amount decides too.
    #[test]
    fn settled_chunks_are_remeshed_once() {
        use crate::world::CHUNK_SIZE;
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;

        /// How many times each chunk was marked changed, by coordinate.
        #[derive(Resource, Default)]
        struct Remeshed(HashMap<IVec3, usize>);

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, ScrollPlugin))
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
