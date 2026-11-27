//! The cracks on a block being mined, in the game.

use bevy::prelude::*;
use defaria_client::cracks::Cracks;

/// Through the real game and input pipeline: holding left click on a
/// block cracks it, and letting go takes the cracks away.
#[test]
fn holding_left_click_cracks_the_block_in_the_game() {
    use crate::headless::{Script, run_more, settle, test_game};

    // A stone in the air, the camera two blocks over it looking down.
    let script = Script::parse("camera 40.5 123 40.5 0 -90\nwait 2").unwrap();
    let mut app = test_game(script);
    settle(&mut app);
    run_more(&mut app, "fill 40 120 40 40 120 40 stone\nwait 2");
    run_more(&mut app, "hold MouseLeft 1  # lock the cursor\nwait 1");
    // About half way through breaking the stone.
    run_more(&mut app, "press MouseLeft\nwait 30");
    let (stage, at, visibility, points, _) = overlay(&mut app);
    assert_eq!(at, Vec3::new(40.0, 120.0, 40.0));
    assert_eq!(visibility, Visibility::Inherited);
    assert!(stage.is_some_and(|stage| stage > 0), "stage {stage:?}");
    assert_eq!(points.len(), 6 * 4);

    run_more(&mut app, "release MouseLeft\nwait 2");
    let (stage, _, visibility, _, _) = overlay(&mut app);
    assert_eq!((stage, visibility), (None, Visibility::Hidden));
}

/// The overlay's stage, place, visibility, mesh positions, and whether it
/// shows its stage's material.
fn overlay(app: &mut App) -> (Option<usize>, Vec3, Visibility, Vec<Vec3>, bool) {
    let mut query = app.world_mut().query::<(
        &Cracks,
        &Transform,
        &Visibility,
        Option<&Mesh3d>,
        &MeshMaterial3d<StandardMaterial>,
    )>();
    let (cracks, transform, visibility, mesh, material) = query.single(app.world()).unwrap();
    let points = mesh.map_or(Vec::new(), |mesh| {
        let mesh = app.world().resource::<Assets<Mesh>>().get(&mesh.0).unwrap();
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            .unwrap()
            .as_float3()
            .unwrap()
            .iter()
            .map(|&p| Vec3::from(p))
            .collect()
    });
    (
        cracks.stage,
        transform.translation,
        *visibility,
        points,
        cracks
            .stage
            .is_some_and(|stage| material.0 == cracks.stages[stage]),
    )
}
