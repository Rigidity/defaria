//! Breaking and placing with the mouse, in the game.

use bevy::prelude::*;
use defaria_core::{
    content::{BlockState, base, block, grass},
    mining::Mining,
    world::{Chunk, ChunkCoord},
};
use defaria_server::worldgen::{Terrain, TerrainBlocks};

use crate::headless::{Script, settle, test_game};

/// Frames, at the headless runner's 60 per second, that holding left
/// click on grass takes to break it.
fn grass_frames() -> u32 {
    let seconds = defaria_core::mining::break_time(base().state(grass()).hardness);
    (seconds * 60.0).ceil() as u32
}

/// Runs `script` (after a `camera` line three blocks above the grass
/// at (40, 40), looking straight down) through the real game and input
/// pipeline, and returns the game, the grass's position and what was
/// under it before.
fn run_over_grass(script: &str) -> (App, IVec3, BlockState) {
    let (x, z) = (40, 40);
    let height = Terrain::new(default(), TerrainBlocks::from_content(base())).height(x, z);
    let script = Script::parse(&format!(
        "camera {} {} {} 0 -90\n{script}",
        x as f32 + 0.5,
        height as f32 + 3.5,
        z as f32 + 0.5,
    ))
    .unwrap();
    let mut app = test_game(script);
    settle(&mut app);

    let top = IVec3::new(x, height - 1, z);
    let below = top - IVec3::Y;
    assert_eq!(world_voxel(&mut app, top), grass());
    let below_before = world_voxel(&mut app, below);
    assert!(!below_before.is_air());

    let mut frames = 1;
    while app.should_exit().is_none() {
        app.update();
        frames += 1;
        // The script starts once the fog has opened up: a few seconds.
        assert!(frames < 600, "script never finished");
    }
    (app, top, below_before)
}

/// Through the real game: the click that locks the cursor mines
/// nothing, even held for longer than the grass takes to break.
#[test]
fn holding_the_locking_click_mines_nothing() {
    let hold = grass_frames() * 2;
    let (mut app, top, _) = run_over_grass(&format!(
        "wait 2\n\
         hold MouseLeft {hold}  # lock the cursor, and keep holding\n\
         wait 2\n"
    ));
    assert_eq!(world_voxel(&mut app, top), grass());
    let mut miners = app.world_mut().query::<&Mining>();
    assert_eq!(miners.single(app.world()).unwrap().target, None);
}

/// Through the real game: the click that locks the cursor edits
/// nothing, holding the next one breaks the block under the crosshair,
/// and a right click puts the active block (stone) back.
#[test]
fn locking_click_edits_nothing_then_clicks_break_and_place() {
    // Long enough to break the grass, but not the block under it too.
    let hold = grass_frames() + 2;
    let (mut app, top, below_before) = run_over_grass(&format!(
        "wait 2\n\
         hold MouseLeft 1  # lock the cursor\n\
         wait 1\n\
         hold MouseLeft {hold} # break\n\
         wait 1\n\
         hold MouseRight 1  # place\n\
         wait 2\n"
    ));
    // Grass broken once, not twice, and stone placed in its place.
    assert_eq!(world_voxel(&mut app, top), block("stone"));
    assert_eq!(world_voxel(&mut app, top - IVec3::Y), below_before);
}

fn world_voxel(app: &mut App, pos: IVec3) -> BlockState {
    let (coord, local) = ChunkCoord::split(pos);
    let mut chunks = app.world_mut().query::<(&ChunkCoord, &Chunk)>();
    chunks
        .iter(app.world())
        .find(|(c, _)| **c == coord)
        .map_or(BlockState::AIR, |(_, chunk)| chunk.at(local))
}
