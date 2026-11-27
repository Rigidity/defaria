//! Light in the game.

use bevy::prelude::*;
use defaria_core::{
    content::Content,
    light::{ChunkLight, ChunkStore, Light, LightQueue, LightTable, sky::Sky},
    world::{CHUNK_SIZE, CHUNK_VOLUME, Chunk, ChunkCoord},
};

/// Breaking a block under a pool of water, so it drains into a chamber
/// below and its level drops, leaves the light around it as lighting
/// the drained world afresh gives it: none stale where the water was.
#[test]
fn a_pool_draining_into_a_hole_relights() {
    let mut app = crate::headless::test_game(
        crate::headless::Script::parse(
            "camera 6.5 70 24 0 -30\n\
             settle\n\
             fill -2 52 -2 14 66 14 air\n\
             fill -1 53 -1 13 56 13 stone\n\
             fill -1 57 -1 13 59 -1 stone\n\
             fill -1 57 13 13 59 13 stone\n\
             fill -1 57 -1 -1 59 13 stone\n\
             fill 13 57 -1 13 59 13 stone\n\
             fill 0 53 0 12 55 12 air\n\
             fill 0 57 0 12 59 12 water\n\
             wait 300\n",
        )
        .unwrap(),
    );
    crate::headless::settle(&mut app);
    // Long enough for the water to settle and the light to follow.
    let settled = |app: &mut App| {
        for _ in 0..1500 {
            app.update();
        }
        assert!(
            app.world().resource::<LightQueue>().is_empty(),
            "light caught up"
        );
    };
    crate::headless::run_more(&mut app, "wait 1\n");
    settled(&mut app);
    let before = stale_light(&mut app);
    assert!(
        before.is_empty(),
        "before: {} stale: {:?}",
        before.len(),
        &before[..before.len().min(6)]
    );
    // Break the floor under the middle of the pool: it drains into the
    // dark chamber below, and its level drops.
    crate::headless::run_more(&mut app, "fill 6 56 6 6 56 6 air\nwait 600\n");
    settled(&mut app);
    let after = stale_light(&mut app);
    assert!(
        after.is_empty(),
        "after: {} stale: {:?}",
        after.len(),
        &after[..after.len().min(6)]
    );
}

/// Every loaded chunk lit afresh from its blocks and fluid, as if just
/// loaded together, with the world's sky; and the voxels whose light
/// in the world differs from it, in chunks whose neighbours are all
/// loaded (the edge of the loaded world is lit by what's beyond it).
fn stale_light(app: &mut App) -> Vec<(IVec3, Light, Light)> {
    struct Fresh(bevy::platform::collections::HashMap<IVec3, (Chunk, ChunkLight)>);
    impl ChunkStore for Fresh {
        fn with_chunk(&mut self, coord: IVec3, f: impl FnOnce(&Chunk, &mut ChunkLight)) -> bool {
            match self.0.get_mut(&coord) {
                Some((chunk, light)) => {
                    f(chunk, light);
                    true
                }
                None => false,
            }
        }
        fn is_loaded(&self, coord: IVec3) -> bool {
            self.0.contains_key(&coord)
        }
    }
    let world = app.world_mut();
    let mut chunks = world.query::<(&ChunkCoord, &Chunk, &ChunkLight)>();
    let loaded: Vec<(IVec3, Chunk, ChunkLight)> = chunks
        .iter(world)
        .map(|(coord, chunk, light)| (coord.0, chunk.clone(), light.clone()))
        .collect();
    let table = LightTable::new(world.resource::<Content>());
    let sky = world.resource::<Sky>().0.clone();
    let mut fresh = Fresh(default());
    let mut queue = LightQueue::default();
    for (coord, chunk, _) in &loaded {
        queue.chunk_loaded(*coord, chunk, &table, &*sky, |c| {
            fresh.0.get(&c).map(|(chunk, light)| (chunk, light))
        });
        fresh
            .0
            .insert(*coord, (chunk.clone(), ChunkLight::default()));
    }
    queue.run(&mut fresh, &table, &*sky);
    let mut stale = Vec::new();
    for (coord, _, light) in &loaded {
        let inner = ChunkCoord(*coord)
            .neighbours()
            .all(|n| fresh.0.contains_key(&n.0));
        if !inner {
            continue;
        }
        let expected = &fresh.0[coord].1;
        for index in 0..CHUNK_VOLUME {
            let local = Chunk::position_of(index);
            if light.at(local) != expected.at(local) {
                stale.push((
                    *coord * CHUNK_SIZE as i32 + local,
                    light.at(local),
                    expected.at(local),
                ));
            }
        }
    }
    stale.sort_by_key(|(pos, _, _)| pos.to_array());
    stale
}
