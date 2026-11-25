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
    player::{GROUND_FRICTION, Player, PlayerMovement, TICK},
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
    let view = Mat4::from_translation(offset.as_vec3())
        * Mat4::from_rotation_z(roll as f32)
        * Mat4::from_rotation_x(nod as f32);
    Transform::from_matrix(view.inverse())
}

fn give_the_player_a_bob(add: On<Add<Player>>, mut commands: Commands) {
    commands
        .entity(add.entity)
        .insert_if_new(ViewBob::default());
}

/// After each physics step: adds the distance walked, and eases the sway
/// towards the player's speed on the ground, measured after a tick's
/// friction.
fn count_steps(time: Res<Time>, mut players: Query<(&Body, &Player, &mut ViewBob)>) {
    let ease = 1.0 - (1.0 - EASING).powf(time.delta_secs_f64() / TICK);
    for (body, player, mut bob) in &mut players {
        bob.previous_walked = bob.walked;
        bob.previous_amount = bob.amount;
        let moved = (body.position - body.previous).with_y(0.0).length();
        bob.walked += moved * STRIDE;
        let target = if body.on_ground && !player.flying {
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
    use crate::player::tests::{FRAME, app, floor, hold, run, spawn};

    /// Standing still, nothing sways.
    #[test]
    fn standing_still_doesnt_sway() {
        assert_eq!(sway(3.7, 0.0), Transform::IDENTITY);
    }

    /// The eyes bob up and down once a step, `amount` high, and sway to
    /// one side on one step and the other on the next.
    #[test]
    fn the_view_bobs_each_step_and_sways_side_to_side() {
        let amount = MAX_AMOUNT;
        let step = sway(0.0, amount).translation;
        let mid_left = sway(0.5, amount).translation;
        let next_step = sway(1.0, amount).translation;
        let mid_right = sway(1.5, amount).translation;
        // The world moves down by `amount` at each whole step, so the
        // camera is that much higher there, and level between steps.
        for at in [step, next_step] {
            assert!((at.y - amount as f32).abs() < 0.01, "{at}");
            assert!(at.x.abs() < 1e-4, "{at}");
        }
        for mid in [mid_left, mid_right] {
            assert!(mid.y.abs() < 0.01, "{mid}");
            assert!((mid.x.abs() - 0.05).abs() < 0.01, "{mid}");
        }
        assert!(mid_left.x * mid_right.x < 0.0, "{mid_left} {mid_right}");
    }

    /// Walking makes the view sway; stopping settles it; flying never
    /// sways.
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

        hold(&mut app, &[]);
        run(&mut app, 1.0);
        let bob = *app.world().get::<ViewBob>(player).unwrap();
        assert!(bob.amount < 1e-3, "{bob:?}");

        app.world_mut().get_mut::<Player>(player).unwrap().flying = true;
        hold(&mut app, &[KeyCode::KeyW]);
        run(&mut app, 1.0);
        let bob = *app.world().get::<ViewBob>(player).unwrap();
        assert!(bob.amount < 1e-3, "flying: {bob:?}");
    }
}
