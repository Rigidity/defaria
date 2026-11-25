//! View bobbing: the view sways with each step while the player walks. It
//! sways side to side and bobs once a step, with a little roll and nod,
//! more the faster the player walks, and settles when the player stops,
//! jumps or flies.
//!
//! The shape and numbers follow Minecraft's view bobbing. It counts
//! distance walked (`walked`, 0.6 a block, so a step is 1/0.6 blocks), and
//! eases the sway's size towards the player's speed on the ground each
//! tick. Both are kept per physics step and drawn between steps like the
//! player's position.
//!
//! The sway moves the [`EyeView`], the camera that draws the view, and not
//! the [`FlyCamera`] it's a child of, so it never changes what the player
//! aims at or where the world loads.

use std::f64::consts::PI;

use bevy::{math::DVec3, prelude::*};

use crate::{
    fly_camera::EyeView,
    physics::Body,
    player::{GROUND_FRICTION, Player, PlayerMovement, TICK, per_tick},
};

/// How much `walked` grows per block walked.
const STRIDE: f64 = 0.6;
/// The most the sway grows to, in blocks a tick of speed.
const MAX_AMOUNT: f64 = 0.1;
/// How much of the way the sway's size moves to its target each tick.
const EASING: f64 = 0.4;

pub struct ViewBobPlugin;

impl Plugin for ViewBobPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(give_the_player_a_bob)
            .add_systems(FixedUpdate, count_steps.after(PlayerMovement))
            .add_systems(
                RunFixedMainLoop,
                sway_the_view.in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
            );
    }
}

/// How far the player has walked and how much the view sways, now and
/// before the last physics step.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct ViewBob {
    /// [`STRIDE`] for every block walked: a step every 1.
    pub walked: f64,
    pub previous_walked: f64,
    /// The sway's size, up to [`MAX_AMOUNT`].
    pub amount: f64,
    pub previous_amount: f64,
}

impl ViewBob {
    /// The sway, `fraction` of the way through the step after the last
    /// one: where the eye view goes relative to the eyes.
    pub fn sway(&self, fraction: f64) -> Transform {
        let walked = self.previous_walked + (self.walked - self.previous_walked) * fraction;
        let amount = self.previous_amount + (self.amount - self.previous_amount) * fraction;
        sway(walked, amount)
    }

    /// Stops the sway at once, for a player put somewhere new.
    pub fn settle(&mut self) {
        self.amount = 0.0;
        self.previous_amount = 0.0;
    }
}

/// The sway, `amount` big, `walked` along: a move and turn of what's
/// seen, in view space, so the camera moves and turns the other way.
pub fn sway(walked: f64, amount: f64) -> Transform {
    // The phase runs backwards. Wrapped, as `walked` only grows.
    let phase = -walked.rem_euclid(2.0) * PI;
    let offset = DVec3::new(
        phase.sin() * amount * 0.5,
        -(phase.cos() * amount).abs(),
        0.0,
    );
    let roll = (phase.sin() * amount * 3.0).to_radians();
    let nod = ((phase - 0.2).cos() * amount).abs() * 5.0_f64.to_radians();
    let view = Isometry3d::new(
        offset.as_vec3(),
        Quat::from_rotation_z(roll as f32) * Quat::from_rotation_x(nod as f32),
    );
    Transform::from_isometry(view.inverse())
}

fn give_the_player_a_bob(add: On<Add<Player>>, mut commands: Commands) {
    commands
        .entity(add.entity)
        .insert_if_new(ViewBob::default());
}

/// After each physics step: adds the distance walked, and eases the sway
/// towards the player's speed on the ground, measured after a tick's
/// friction. In the air, flying included, it eases to nothing.
fn count_steps(time: Res<Time>, mut players: Query<(&Body, &mut ViewBob)>) {
    let ease = per_tick(EASING, time.delta_secs_f64());
    for (body, mut bob) in &mut players {
        bob.previous_walked = bob.walked;
        bob.previous_amount = bob.amount;
        let moved = (body.position - body.previous).with_y(0.0).length();
        bob.walked += moved * STRIDE;
        let target = if body.on_ground {
            let speed = body.velocity.with_y(0.0).length() * TICK * GROUND_FRICTION;
            speed.min(MAX_AMOUNT)
        } else {
            0.0
        };
        bob.amount += (target - bob.amount) * ease;
    }
}

/// Sways each player's eye view, between the last two physics steps.
fn sway_the_view(
    time: Res<Time<Fixed>>,
    players: Query<(&ViewBob, &Children)>,
    mut views: Query<&mut Transform, With<EyeView>>,
) {
    let fraction = time.overstep_fraction_f64();
    for (bob, children) in &players {
        for &child in children {
            if let Ok(mut view) = views.get_mut(child) {
                *view = bob.sway(fraction);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        physics::STEP,
        player::tests::{FRAME, app, floor, hold, run, spawn},
    };

    /// Standing still, nothing sways.
    #[test]
    fn standing_still_doesnt_sway() {
        assert_eq!(sway(3.7, 0.0), Transform::IDENTITY);
    }

    /// The eyes bob up `amount` at each whole step and are level between
    /// steps, where they sway half that to one side on one step and the
    /// other on the next.
    #[test]
    fn the_view_bobs_each_step_and_sways_side_to_side() {
        let amount = MAX_AMOUNT;
        let step = sway(0.0, amount).translation;
        let mid_left = sway(0.5, amount).translation;
        let next_step = sway(1.0, amount).translation;
        let mid_right = sway(1.5, amount).translation;
        for at in [step, next_step] {
            assert!((at.y - amount as f32).abs() < 1e-4, "{at}");
            assert!(at.x.abs() < 1e-4, "{at}");
        }
        for mid in [mid_left, mid_right] {
            assert!(mid.y.abs() < 1e-3, "{mid}");
            assert!((mid.x.abs() - 0.05).abs() < 1e-3, "{mid}");
        }
        assert!(mid_left.x * mid_right.x < 0.0, "{mid_left} {mid_right}");
    }

    /// Between steps the view rolls 0.3° one way, then the other; at each
    /// step it nods down 0.5° (cos 0.2 of it).
    #[test]
    fn the_view_rolls_side_to_side_and_nods_down() {
        let amount = MAX_AMOUNT;
        let right = |walked: f64| sway(walked, amount).rotation * Vec3::X;
        let forward = |walked: f64| sway(walked, amount).rotation * Vec3::NEG_Z;
        let roll = 0.3_f32.to_radians().sin();
        let (left, right_side) = (right(0.5).y, right(1.5).y);
        assert!((left.abs() - roll).abs() < 1e-5, "{left}");
        assert!((right_side.abs() - roll).abs() < 1e-5, "{right_side}");
        assert!(left * right_side < 0.0, "{left} {right_side}");
        // It rolls with the sideways sway: both come from the same swing,
        // so the camera's sideways shift and its tilt share a sign.
        for walked in [0.5, 1.5] {
            let shift = sway(walked, amount).translation.x;
            assert!(shift * right(walked).y > 0.0, "{walked}");
        }
        let nod = (0.5 * 0.2_f32.cos()).to_radians().sin();
        for walked in [0.0, 1.0, 2.0] {
            let down = forward(walked).y;
            assert!((down + nod).abs() < 1e-5, "{walked}: {down}");
        }
        // The nod never tips the view up.
        for tenth in 0..20 {
            let walked = f64::from(tenth) / 10.0;
            assert!(forward(walked).y <= 1e-7, "{walked}");
        }
    }

    /// Between physics steps, the sway is drawn part of the way from the
    /// last step's to the new one's.
    #[test]
    fn the_sway_is_drawn_between_steps() {
        let bob = ViewBob {
            walked: 0.4,
            previous_walked: 0.2,
            amount: 0.1,
            previous_amount: 0.05,
        };
        assert_eq!(bob.sway(0.5), sway(0.3, 0.075));

        let mut app = app(IVec3::new(8, 8, 8), STEP / 2, floor(4));
        app.add_plugins(ViewBobPlugin);
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        let view = app.world_mut().spawn((EyeView, ChildOf(player))).id();
        hold(&mut app, &[KeyCode::KeyW]);
        run(&mut app, 0.5);
        let mut halfway = 0;
        for _ in 0..4 {
            run(&mut app, STEP.as_secs_f64() / 2.0);
            let bob = *app.world().get::<ViewBob>(player).unwrap();
            let drawn = *app.world().get::<Transform>(view).unwrap();
            if drawn.translation.distance(bob.sway(0.5).translation) < 1e-6 {
                halfway += 1;
            }
            assert_ne!(bob.walked, bob.previous_walked);
        }
        // Every other frame has no step, so it's drawn half way.
        assert_eq!(halfway, 2);
    }

    /// Walking makes the view sway without moving the eyes; stopping
    /// settles it; flying never sways.
    #[test]
    fn walking_sways_and_stopping_settles() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        app.add_plugins(ViewBobPlugin);
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        let view = app.world_mut().spawn((EyeView, ChildOf(player))).id();
        run(&mut app, 0.2);
        let sway = |app: &App| *app.world().get::<Transform>(view).unwrap();
        assert_eq!(sway(&app), Transform::IDENTITY);

        hold(&mut app, &[KeyCode::KeyW]);
        run(&mut app, 1.0);
        let bob = *app.world().get::<ViewBob>(player).unwrap();
        assert!((bob.amount - MAX_AMOUNT).abs() < 1e-3, "{bob:?}");
        // About 4.3 blocks a second, after getting up to speed.
        assert!((1.5..2.6).contains(&bob.walked), "{bob:?}");
        assert_ne!(sway(&app), Transform::IDENTITY);
        // The eyes, which the player aims from, don't sway.
        let eyes = *app.world().get::<Transform>(player).unwrap();
        let body = app.world().get::<Body>(player).unwrap();
        let expected = (body.previous + DVec3::Y * crate::player::EYE_HEIGHT).as_vec3();
        assert!(eyes.translation.distance(expected) < 1e-5, "{eyes:?}");
        assert_eq!(eyes.rotation, Quat::IDENTITY);

        hold(&mut app, &[]);
        run(&mut app, 1.0);
        let bob = *app.world().get::<ViewBob>(player).unwrap();
        assert!(bob.amount < 1e-3, "{bob:?}");

        app.world_mut().get_mut::<Player>(player).unwrap().flying = true;
        hold(&mut app, &[KeyCode::Space]);
        run(&mut app, 0.2);
        hold(&mut app, &[KeyCode::KeyW]);
        run(&mut app, 1.0);
        let bob = *app.world().get::<ViewBob>(player).unwrap();
        assert!(app.world().get::<Player>(player).unwrap().flying);
        assert!(bob.amount < 1e-3, "flying: {bob:?}");
    }

    /// Sneaking walks slower, so it sways less: its speed after a tick's
    /// friction.
    #[test]
    fn sneaking_sways_less() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        app.add_plugins(ViewBobPlugin);
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        hold(&mut app, &[KeyCode::KeyW, KeyCode::ShiftLeft]);
        run(&mut app, 1.5);
        let bob = *app.world().get::<ViewBob>(player).unwrap();
        let expected = 4.317 * 0.3 * TICK * GROUND_FRICTION;
        assert!((bob.amount - expected).abs() < 1e-3, "{bob:?}");
    }

    /// Leaving the ground, the sway fades: 0.4 of the way to nothing each
    /// tick.
    #[test]
    fn jumping_fades_the_sway_a_tick_at_a_time() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        app.add_plugins(ViewBobPlugin);
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        hold(&mut app, &[KeyCode::KeyW]);
        run(&mut app, 1.0);
        let before = app.world().get::<ViewBob>(player).unwrap().amount;
        assert!((before - MAX_AMOUNT).abs() < 1e-4, "{before}");
        hold(&mut app, &[KeyCode::KeyW, KeyCode::Space]);
        run(&mut app, TICK);
        assert!(!app.world().get::<Body>(player).unwrap().on_ground);
        let after = app.world().get::<ViewBob>(player).unwrap().amount;
        assert!((after - before * (1.0 - EASING)).abs() < 1e-6, "{after}");
    }
}
