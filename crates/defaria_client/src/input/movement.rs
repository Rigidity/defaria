//! The keys moving the local player: WASD, Space to jump (twice quickly to
//! start or stop flying), Shift to sneak, F to sprint. They set its
//! [`Controls`] (see `player`), which move it the same way any player's
//! do.

use bevy::prelude::*;

use crate::ui::{UiScreen, playing};
use defaria_core::player::{Controls, DOUBLE_TAP, LocalPlayer, ReadControls};

pub struct MovementInputPlugin;

impl Plugin for MovementInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, toggle_flying.run_if(playing))
            .add_systems(FixedUpdate, read_the_keys.in_set(ReadControls));
    }
}

/// Double-tapping Space asks for the local player to start or stop flying
/// ([`Controls::toggle_fly`]).
fn toggle_flying(
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut last_tap: Local<Option<f32>>,
    mut players: Query<&mut Controls, With<LocalPlayer>>,
) {
    if !keys.just_pressed(KeyCode::Space) {
        return;
    }
    let now = time.elapsed_secs();
    if last_tap.is_some_and(|last| now - last <= DOUBLE_TAP) {
        *last_tap = None;
        for mut controls in &mut players {
            controls.toggle_fly = true;
        }
    } else {
        *last_tap = Some(now);
    }
}

/// The keys held, as the local player's [`Controls`].
fn read_the_keys(
    keys: Res<ButtonInput<KeyCode>>,
    screens: Query<(), With<UiScreen>>,
    mut players: Query<&mut Controls, With<LocalPlayer>>,
) {
    // With a screen open, keys are for it: the player stops pressing them
    // but still falls and slides.
    let playing = playing(screens);
    let held = |key: KeyCode| playing && keys.pressed(key);
    let axis = |positive: KeyCode, negative: KeyCode| {
        held(positive) as i8 as f64 - held(negative) as i8 as f64
    };
    for mut player in &mut players {
        // A double tap's request stands until the step that takes it.
        let controls = Controls {
            forward: axis(KeyCode::KeyW, KeyCode::KeyS),
            right: axis(KeyCode::KeyD, KeyCode::KeyA),
            jump: held(KeyCode::Space),
            sneak: held(KeyCode::ShiftLeft) || held(KeyCode::ShiftRight),
            sprint: held(KeyCode::KeyF),
            toggle_fly: player.toggle_fly,
        };
        player.set_if_neq(controls);
    }
}

#[cfg(test)]
mod tests {
    use bevy::math::DVec3;

    use super::*;
    use defaria_core::player::tests::{FRAME, HeldKeys, app, floor, spawn};
    use defaria_core::{physics::Body, player::Player};

    /// [`player::tests::app`](defaria_core::player::tests::app) reading the
    /// keyboard as play does.
    fn keyboard_app() -> App {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        app.add_plugins(MovementInputPlugin)
            .init_resource::<ButtonInput<KeyCode>>()
            .world_mut()
            .remove_resource::<HeldKeys>();
        app
    }

    fn hold(app: &mut App, keys: &[KeyCode]) {
        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        input.release_all();
        input.clear();
        for &key in keys {
            input.press(key);
        }
    }

    fn run(app: &mut App, seconds: f64) {
        for _ in 0..(seconds / FRAME.as_secs_f64()).round() as usize {
            app.update();
            // Pressed keys stay pressed, but only the first frame's are
            // just pressed.
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .clear();
        }
    }

    fn body(app: &App, player: Entity) -> Body {
        app.world().get::<Body>(player).unwrap().clone()
    }

    fn player(app: &App, entity: Entity) -> Player {
        app.world().get::<Player>(entity).unwrap().clone()
    }

    /// Double-tapping Space flies; Space and Shift go up and down; landing
    /// stops flying.
    #[test]
    fn double_tap_flies_and_landing_stops_it() {
        let mut app = keyboard_app();
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        run(&mut app, 0.1);
        let mut tapped = 0.0;
        for _ in 0..2 {
            tapped = body(&app, player).position.y;
            hold(&mut app, &[KeyCode::Space]);
            run(&mut app, 0.05);
            hold(&mut app, &[]);
            run(&mut app, 0.05);
        }
        assert!(self::player(&app, player).flying);
        // The first tap's jump stops when flying starts: from the second
        // tap, it only rises for the three steps Space is held (about 0.4),
        // not on with the jump's speed too (about a block).
        run(&mut app, 1.0);
        let rise = body(&app, player).position.y - tapped;
        assert!((0.2..0.6).contains(&rise), "{rise}");
        hold(&mut app, &[KeyCode::Space]);
        run(&mut app, 1.0);
        let up = body(&app, player);
        assert!(up.position.y > 9.0, "{}", up.position);
        // Hovering: no gravity.
        hold(&mut app, &[]);
        run(&mut app, 1.0);
        let hover = body(&app, player).position.y;
        run(&mut app, 1.0);
        assert!((body(&app, player).position.y - hover).abs() < 1e-3);
        hold(&mut app, &[KeyCode::ShiftLeft]);
        run(&mut app, 3.0);
        assert!(!self::player(&app, player).flying);
        assert_eq!(body(&app, player).position.y, 4.0);
    }

    /// Two taps further apart than [`DOUBLE_TAP`] are two jumps, not a flight.
    #[test]
    fn slow_taps_dont_fly() {
        let mut app = keyboard_app();
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        run(&mut app, 0.1);
        for _ in 0..2 {
            hold(&mut app, &[KeyCode::Space]);
            run(&mut app, 0.05);
            hold(&mut app, &[]);
            run(&mut app, DOUBLE_TAP as f64 + 0.1);
        }
        assert!(!self::player(&app, player).flying);
    }
}
