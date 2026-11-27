//! Blocks that need support: plants stand on what their block's `support`
//! lists (see `docs/modding.md`), and break when it goes.
//!
//! - Placing one is refused where the block under it doesn't support it
//!   (in `block_edit`), so nothing is used up.
//! - After every edit ([`VoxelChanged`], from the player, scripts, fluids,
//!   a scenario's `fill`), the edited voxel and the one above it are
//!   checked, and above every block whose state was updated in place
//!   ([`PropertiesUpdated`]: a script's `set_state`, a machine turning
//!   on): a block there that needs support and doesn't have it is
//!   broken, and so is anything resting on it in turn. That's the same
//!   frame for edits made in `Update`, and across chunk borders, as long
//!   as both chunks are loaded. A block whose support is in a chunk that
//!   isn't loaded is left alone.
//! - Broken this way, it gives nothing, except that a player mining the
//!   block under it in survival gets its item too (in `mining`, through
//!   [`unsupported`]).
//!
//! The terrain only grows plants where they're supported, so loading
//! chunks checks nothing.

use bevy::{platform::collections::HashSet, prelude::*};

use crate::{
    block_edit::BlockEditSystems,
    connections::ConnectionSystems,
    content::{BlockState, Content},
    mining::MiningSystems,
    world::{PropertiesUpdated, RemeshChunks, VoxelChanged, Voxels},
};

pub struct SupportPlugin;

impl Plugin for SupportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Unchecked>().add_systems(
            Update,
            (note_edits, break_unsupported)
                .chain()
                .in_set(SupportSystems)
                .after(BlockEditSystems)
                .after(MiningSystems)
                .before(ConnectionSystems)
                .before(RemeshChunks),
        );
    }
}

/// Where blocks that lost their support break: after this frame's edits,
/// before connections are updated and chunks remeshed, so the break shows
/// on the same frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SupportSystems;

/// Voxels that may hold a block that lost its support.
#[derive(Resource, Debug, Default)]
struct Unchecked(HashSet<IVec3>);

/// Each edited voxel (a plant placed where it can't stand) and the one
/// above it (standing on what changed), and the one above each voxel whose
/// state changed in place (grass turned to sand by a script's `set`).
fn note_edits(
    mut changes: MessageReader<VoxelChanged>,
    mut updates: MessageReader<PropertiesUpdated>,
    mut unchecked: ResMut<Unchecked>,
) {
    for change in changes.read() {
        unchecked.0.insert(change.pos);
        unchecked.0.insert(change.pos + IVec3::Y);
    }
    for update in updates.read() {
        unchecked.0.insert(update.pos + IVec3::Y);
    }
}

/// The block at `pos`, if it needs support and the block under it doesn't
/// give it. `None` while either isn't loaded.
pub fn unsupported(content: &Content, voxels: &Voxels, pos: IVec3) -> Option<BlockState> {
    let block = voxels.get(pos)?;
    if !content.needs_support(block) {
        return None;
    }
    let below = voxels.get(pos - IVec3::Y)?;
    (!content.supports(block, below)).then_some(block)
}

/// Breaks every noted block that lost its support, and what stood on it.
fn break_unsupported(mut unchecked: ResMut<Unchecked>, content: Res<Content>, mut voxels: Voxels) {
    if unchecked.0.is_empty() {
        return;
    }
    // In a fixed order, so runs repeat.
    let mut queue: Vec<IVec3> = unchecked.0.drain().collect();
    queue.sort_unstable_by_key(|pos| std::cmp::Reverse(pos.to_array()));
    while let Some(pos) = queue.pop() {
        if unsupported(&content, &voxels, pos).is_some() {
            voxels.set(pos, BlockState::AIR);
            queue.push(pos + IVec3::Y);
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::{
        content::{base, block, grass, sand, test_mod},
        world::{CHUNK_SIZE, Chunk, ChunkCoord, ChunkMap, FluidChanged, PropertiesUpdated},
    };

    /// The chunk at the origin and the one above it, empty, with the
    /// support systems.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, SupportPlugin))
            .insert_resource(base().clone())
            .init_resource::<ChunkMap>()
            .add_message::<VoxelChanged>()
            .add_message::<PropertiesUpdated>()
            .add_message::<FluidChanged>();
        for coord in [IVec3::ZERO, IVec3::Y] {
            let entity = app
                .world_mut()
                .spawn((ChunkCoord(coord), Chunk::default()))
                .id();
            app.world_mut()
                .resource_mut::<ChunkMap>()
                .insert(ChunkCoord(coord), entity);
        }
        app
    }

    /// Edits voxels as a player or a script would, then runs a frame.
    fn edit(app: &mut App, edits: &[(IVec3, BlockState)]) {
        let edits = edits.to_vec();
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| {
                for &(pos, state) in &edits {
                    voxels.set(pos, state);
                }
            })
            .unwrap();
        app.update();
    }

    fn at(app: &mut App, pos: IVec3) -> BlockState {
        app.world_mut()
            .run_system_once(move |voxels: Voxels| voxels.get(pos).unwrap())
            .unwrap()
    }

    #[test]
    fn plants_break_when_their_support_goes() {
        let mut app = app();
        let ground = IVec3::new(3, 4, 3);
        let tuft = ground + IVec3::Y;
        edit(&mut app, &[(ground, grass()), (tuft, block("tuft"))]);
        assert_eq!(at(&mut app, tuft), block("tuft"));
        // Grass to bare dirt still holds it.
        let dirt =
            crate::content::block_with("soil", &[("soil", "defaria:dirt"), ("turf", "none")]);
        edit(&mut app, &[(ground, dirt)]);
        assert_eq!(at(&mut app, tuft), block("tuft"));
        // Sand doesn't.
        edit(&mut app, &[(ground, sand())]);
        assert_eq!(at(&mut app, tuft), BlockState::AIR);
    }

    /// A support changed in place (a script's `set_state`, or `set` to
    /// another state of the same block) is checked too.
    #[test]
    fn support_updated_in_place_is_checked() {
        let mut app = app();
        let ground = IVec3::new(3, 4, 3);
        let tuft = ground + IVec3::Y;
        edit(&mut app, &[(ground, grass()), (tuft, block("tuft"))]);
        app.world_mut()
            .run_system_once(move |mut voxels: Voxels| voxels.update(ground, sand()))
            .unwrap();
        app.update();
        assert_eq!(at(&mut app, ground), sand());
        assert_eq!(at(&mut app, tuft), BlockState::AIR);
    }

    /// What stands on a plant that breaks breaks with it, on the same frame.
    #[test]
    fn plants_on_plants_break_together() {
        let content = test_mod(&[
            (
                "blocks/ground.json5",
                r#"{ model: "quad", textures: { all: "a" } }"#,
            ),
            (
                "blocks/stalk.json5",
                r#"{ model: "quad", textures: { all: "a" },
                     support: [{ block: "ground" }, { block: "stalk" }] }"#,
            ),
        ]);
        assert!(content.errors.is_empty(), "{:?}", content.errors);
        let ground = content.default_state("test:ground").unwrap();
        let stalk = content.default_state("test:stalk").unwrap();
        let mut app = app();
        app.insert_resource(content);
        let pos = IVec3::new(3, 2, 3);
        let stack: Vec<_> = (1..5).map(|y| (pos + IVec3::Y * y, stalk)).collect();
        edit(&mut app, &[&[(pos, ground)][..], &stack[..]].concat());
        assert_eq!(at(&mut app, pos + IVec3::Y * 4), stalk);
        edit(&mut app, &[(pos, BlockState::AIR)]);
        for (at_pos, _) in stack {
            assert_eq!(at(&mut app, at_pos), BlockState::AIR, "{at_pos}");
        }
    }

    #[test]
    fn broken_ground_takes_the_plant_on_the_same_frame() {
        let mut app = app();
        let ground = IVec3::new(3, 4, 3);
        edit(
            &mut app,
            &[(ground, grass()), (ground + IVec3::Y, block("daisy"))],
        );
        edit(&mut app, &[(ground, BlockState::AIR)]);
        assert_eq!(at(&mut app, ground + IVec3::Y), BlockState::AIR);
    }

    #[test]
    fn plants_break_across_chunk_borders() {
        let mut app = app();
        // The top of the lower chunk, and the bottom of the one above.
        let ground = IVec3::new(7, CHUNK_SIZE as i32 - 1, 7);
        let plant = ground + IVec3::Y;
        edit(&mut app, &[(ground, grass()), (plant, block("fern"))]);
        assert_eq!(at(&mut app, plant), block("fern"));
        edit(&mut app, &[(ground, BlockState::AIR)]);
        assert_eq!(at(&mut app, plant), BlockState::AIR);
    }

    #[test]
    fn a_plant_set_where_it_cant_stand_breaks() {
        let mut app = app();
        let stone = IVec3::new(2, 2, 2);
        let sand_pos = IVec3::new(5, 2, 2);
        edit(
            &mut app,
            &[
                (stone, block("stone")),
                (stone + IVec3::Y, block("tuft")),
                (stone + IVec3::new(1, 1, 0), block("mushroom")),
                (stone + IVec3::new(1, 0, 0), block("stone")),
                (sand_pos, sand()),
                (sand_pos + IVec3::Y, block("dead_bush")),
                (IVec3::new(9, 9, 9), block("daisy")), // in mid-air
            ],
        );
        assert_eq!(at(&mut app, stone + IVec3::Y), BlockState::AIR);
        assert_eq!(at(&mut app, stone + IVec3::new(1, 1, 0)), block("mushroom"));
        assert_eq!(at(&mut app, sand_pos + IVec3::Y), block("dead_bush"));
        assert_eq!(at(&mut app, IVec3::new(9, 9, 9)), BlockState::AIR);
    }

    /// A plant whose support is in a chunk that isn't loaded stays put.
    #[test]
    fn plants_over_unloaded_chunks_are_left_alone() {
        let mut app = app();
        let plant = IVec3::new(4, 0, 4); // the chunk below isn't loaded
        edit(&mut app, &[(plant, block("tulip"))]);
        assert_eq!(at(&mut app, plant), block("tulip"));
    }

    /// Blocks without a `support` stand anywhere.
    #[test]
    fn other_blocks_need_no_support() {
        let mut app = app();
        let pos = IVec3::new(4, 8, 4);
        edit(
            &mut app,
            &[(pos, block("stone")), (pos + IVec3::Y, block("log"))],
        );
        assert_eq!(at(&mut app, pos), block("stone"));
        assert_eq!(at(&mut app, pos + IVec3::Y), block("log"));
    }
}
