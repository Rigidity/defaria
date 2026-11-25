//! The player: a body that walks, jumps, sneaks, sprints and flies through
//! the world with Minecraft's movement.
//!
//! - **WASD** moves relative to where you face; looking up or down doesn't
//!   change the direction.
//! - **Space** jumps; held, it jumps again on landing. **Double-tap Space**
//!   to start or stop flying. Landing on the ground stops flying too.
//! - **Shift** sneaks: slower, and you don't walk off edges. Flying, Space
//!   rises and Shift sinks.
//! - **Ctrl** with W sprints, a little faster on foot and twice as fast
//!   flying. A sprinting jump gets a push forward.
//!
//! The camera is the player's eyes. The player is a [`Body`] 0.6 blocks
//! wide and 1.8 tall that climbs steps up to 0.6 (a slab) and collides
//! with blocks as `physics` describes, flying included. It moves in
//! [`FixedUpdate`]; the camera is placed between the last two steps every
//! frame, so it moves smoothly at any frame rate.
//!
//! The numbers are Minecraft's, per 20 Hz tick ([`Rule`]), turned into
//! rates that give the same top speeds and how quickly they're reached at
//! any step rate. Velocities change exactly as they would continuously
//! between steps, so the step rate barely matters.

use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};

use crate::{
    fly_camera::FlyCamera,
    physics::{Body, WorldShapes, guard_edges, move_box},
};

/// Width, height and depth.
pub const SIZE: DVec3 = DVec3::new(0.6, 1.8, 0.6);
/// How high the eyes (the camera) are above the feet.
pub const EYE_HEIGHT: f64 = 1.62;
/// The tallest step the player walks up without jumping: a slab, not a
/// block.
pub const STEP_HEIGHT: f64 = 0.6;
/// Two presses of Space within this many seconds start or stop flying.
pub const DOUBLE_TAP: f32 = 0.35;

/// Minecraft's tick, which [`Rule`]s are written per.
const TICK: f64 = 0.05;

/// One of Minecraft's movement rules, per tick: each tick the velocity
/// gains `accel` in the direction you're going, moves, then is multiplied
/// by `friction`.
#[derive(Debug, Clone, Copy)]
struct Rule {
    accel: f64,
    friction: f64,
}

impl Rule {
    /// How quickly velocity decays, per second.
    fn decay(self) -> f64 {
        -self.friction.ln() / TICK
    }

    /// The top speed, in blocks per second: what Minecraft moves per tick
    /// once its velocity has settled.
    fn top_speed(self) -> f64 {
        self.accel / (1.0 - self.friction) / TICK
    }

    /// The acceleration, in blocks per second per second, that settles at
    /// the same top speed with [`Rule::decay`].
    fn accel(self) -> f64 {
        self.top_speed() * self.decay()
    }
}

/// Minecraft scales movement input by this.
const INPUT: f64 = 0.98;
/// On the ground: block slipperiness 0.6 times air friction 0.91.
const WALK: Rule = Rule {
    accel: 0.1 * INPUT,
    friction: 0.6 * 0.91,
};
const AIR: Rule = Rule {
    accel: 0.02 * INPUT,
    friction: 0.91,
};
const FLY: Rule = Rule {
    accel: 0.05 * INPUT,
    friction: 0.91,
};
/// Up and down, flying: 3 times the fly speed, with heavy friction so it
/// stops quickly.
const FLY_VERTICAL: Rule = Rule {
    accel: 0.15,
    friction: 0.6,
};
/// Sprinting multiplies walking and air acceleration by this, and doubles
/// flying.
const SPRINT: f64 = 1.3;
/// Sneaking multiplies walking input by this.
const SNEAK: f64 = 0.3;
/// Falling: 0.08 a tick of gravity with friction 0.98 settles at 3.92
/// blocks a tick.
const TERMINAL_SPEED: f64 = 3.92 / TICK;
const FALL_FRICTION: f64 = 0.98;
/// The jump's speed straight up, in blocks per second: with
/// [`TERMINAL_SPEED`]'s gravity it peaks 1.2522 blocks up, as Minecraft's
/// 0.42 a tick does. Enough for a block, not a block and a half.
pub const JUMP_SPEED: f64 = 9.2474;
/// A sprinting jump's push forward: 0.2 a tick.
const SPRINT_JUMP_BOOST: f64 = 0.2 / TICK;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(give_the_camera_a_body)
            .add_systems(Update, toggle_flying)
            .add_systems(FixedUpdate, move_player)
            .add_systems(
                RunFixedMainLoop,
                place_the_camera.in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
            );
    }
}

/// The player's movement state. Its [`Body`] is on the same entity, the
/// [`FlyCamera`].
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component)]
pub struct Player {
    pub flying: bool,
    pub sneaking: bool,
    pub sprinting: bool,
}

impl Player {
    /// The player's body with its eyes at `eye`.
    pub fn body(eye: Vec3) -> Body {
        Body::new(eye.as_dvec3() - DVec3::Y * EYE_HEIGHT, SIZE, STEP_HEIGHT)
    }
}

/// Makes the camera the player, with its eyes where the camera is.
fn give_the_camera_a_body(
    add: On<Add<FlyCamera>>,
    transforms: Query<&Transform>,
    mut commands: Commands,
) {
    let eye = transforms
        .get(add.entity)
        .map_or(Vec3::ZERO, |transform| transform.translation);
    commands
        .entity(add.entity)
        .insert_if_new((Player::default(), Player::body(eye)));
}

/// Double-tapping Space starts or stops flying.
fn toggle_flying(
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut last_tap: Local<Option<f32>>,
    mut players: Query<(&mut Player, &mut Body)>,
) {
    if !keys.just_pressed(KeyCode::Space) {
        return;
    }
    let now = time.elapsed_secs();
    if last_tap.is_some_and(|last| now - last <= DOUBLE_TAP) {
        *last_tap = None;
        for (mut player, mut body) in &mut players {
            player.flying = !player.flying;
            body.velocity.y = 0.0;
        }
    } else {
        *last_tap = Some(now);
    }
}

/// How a velocity component changes over `dt` with constant `accel` and
/// exponential `decay`: the new velocity, and how far it went.
fn drift(velocity: f64, accel: f64, decay: f64, dt: f64) -> (f64, f64) {
    let settled = accel / decay;
    let fade = (-decay * dt).exp();
    let new = settled + (velocity - settled) * fade;
    let distance = settled * dt + (velocity - settled) * (1.0 - fade) / decay;
    (new, distance)
}

/// One physics step of the player: input to velocity, then through the
/// world.
fn move_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    shapes: WorldShapes,
    mut players: Query<(&FlyCamera, &mut Player, &mut Body)>,
) {
    let dt = time.delta_secs_f64();
    let held = |key: KeyCode| keys.pressed(key);
    let axis = |positive: KeyCode, negative: KeyCode| {
        held(positive) as i8 as f64 - held(negative) as i8 as f64
    };
    let forward_input = axis(KeyCode::KeyW, KeyCode::KeyS);
    let right_input = axis(KeyCode::KeyD, KeyCode::KeyA);
    let jump = held(KeyCode::Space);
    let shift = held(KeyCode::ShiftLeft) || held(KeyCode::ShiftRight);
    let ctrl = held(KeyCode::ControlLeft) || held(KeyCode::ControlRight);

    for (camera, mut player, mut body) in &mut players {
        body.previous = body.position;
        if dt == 0.0 {
            continue;
        }
        // In the unloaded world, wait for it to load.
        if !shapes.is_loaded(body.position.floor().as_ivec3()) {
            body.velocity = DVec3::ZERO;
            continue;
        }
        player.sneaking = shift && !player.flying;
        player.sprinting = ctrl && forward_input > 0.0 && !player.sneaking;

        // Horizontal movement follows yaw only.
        let yaw = DQuat::from_rotation_y(camera.yaw as f64);
        let forward = yaw * DVec3::NEG_Z;
        let right = yaw * DVec3::X;
        let wish = (forward * forward_input + right * right_input).normalize_or_zero();

        let (rule, mut boost) = if player.flying {
            (FLY, if player.sprinting { 2.0 } else { 1.0 })
        } else if body.on_ground {
            (WALK, 1.0)
        } else {
            (AIR, 1.0)
        };
        if player.sneaking {
            boost *= SNEAK;
        }
        if player.sprinting && !player.flying {
            boost *= SPRINT;
        }
        let mut velocity = body.velocity;
        if !player.flying && body.on_ground && jump {
            velocity.y = JUMP_SPEED;
            if player.sprinting {
                velocity += forward * SPRINT_JUMP_BOOST;
            }
        }
        let mut motion = DVec3::ZERO;
        let accel = wish * rule.accel() * boost;
        for axis in [0, 2] {
            (velocity[axis], motion[axis]) = drift(velocity[axis], accel[axis], rule.decay(), dt);
        }
        (velocity.y, motion.y) = if player.flying {
            let rise = jump as i8 as f64 - shift as i8 as f64;
            let rule = FLY_VERTICAL;
            drift(velocity.y, rule.accel() * rise, rule.decay(), dt)
        } else {
            let decay = -FALL_FRICTION.ln() / TICK;
            drift(velocity.y, -TERMINAL_SPEED * decay, decay, dt)
        };

        let aabb = body.aabb();
        if player.sneaking && body.on_ground {
            let guarded = guard_edges(aabb, motion, body.step_height, &shapes);
            for axis in [0, 2] {
                if guarded[axis] != motion[axis] {
                    velocity[axis] = 0.0;
                }
            }
            motion = guarded;
        }
        let moved = move_box(aabb, motion, body.step_height, body.on_ground, &shapes);
        body.position += moved.offset;
        body.velocity = DVec3::select(moved.blocked, DVec3::ZERO, velocity);
        body.on_ground = moved.landed;
        if player.flying && body.on_ground {
            player.flying = false;
        }
    }
}

/// Puts the camera at the player's eyes, between the last two physics
/// steps, so it moves smoothly whatever the frame rate.
fn place_the_camera(
    time: Res<Time<Fixed>>,
    mut players: Query<(&Body, &mut Transform), With<Player>>,
) {
    let fraction = time.overstep_fraction_f64();
    for (body, mut transform) in &mut players {
        let eye = body.interpolated(fraction) + DVec3::Y * EYE_HEIGHT;
        transform.translation = eye.as_vec3();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::time::TimeUpdateStrategy;

    use super::*;
    use crate::{
        content::{BlockState, base, block},
        physics::{PhysicsPlugin, STEP},
        terrain::{ChunkCoord, ChunkMap},
        voxel::{CHUNK_VOLUME, Chunk},
    };

    /// The player and its systems in a world of the chunks around
    /// `around` (3×3×3 of them), filled by `fill`, with frames of `frame`.
    fn app(around: IVec3, frame: Duration, fill: impl Fn(IVec3) -> BlockState) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PhysicsPlugin, PlayerPlugin))
            .insert_resource(TimeUpdateStrategy::ManualDuration(frame))
            .insert_resource(base().clone())
            .init_resource::<ChunkMap>()
            .init_resource::<ButtonInput<KeyCode>>();
        let (center, _) = ChunkCoord::split(around);
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    let coord = ChunkCoord(center.0 + IVec3::new(x, y, z));
                    let mut chunk = Chunk::default();
                    for index in 0..CHUNK_VOLUME {
                        let local = Chunk::position_of(index);
                        chunk.set_at(local, fill(coord.origin() + local));
                    }
                    let entity = app.world_mut().spawn((coord, chunk)).id();
                    app.world_mut()
                        .resource_mut::<ChunkMap>()
                        .insert(coord, entity);
                }
            }
        }
        app
    }

    /// A flat stone floor whose top is at `y = top`.
    fn floor(top: i32) -> impl Fn(IVec3) -> BlockState {
        move |pos| {
            if pos.y < top {
                block("stone")
            } else {
                BlockState::AIR
            }
        }
    }

    const FRAME: Duration = STEP;

    /// Spawns the player with its feet at `feet`, facing north (−Z).
    fn spawn(app: &mut App, feet: DVec3) -> Entity {
        let eye = (feet + DVec3::Y * EYE_HEIGHT).as_vec3();
        let entity = app
            .world_mut()
            .spawn((
                FlyCamera::default(),
                Transform::from_translation(eye),
                Body::new(feet, SIZE, STEP_HEIGHT),
            ))
            .id();
        app.update();
        entity
    }

    fn body(app: &App, player: Entity) -> Body {
        app.world().get::<Body>(player).unwrap().clone()
    }

    fn player(app: &App, entity: Entity) -> Player {
        app.world().get::<Player>(entity).unwrap().clone()
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
        let frame = match app.world().resource::<TimeUpdateStrategy>() {
            TimeUpdateStrategy::ManualDuration(frame) => frame.as_secs_f64(),
            _ => unreachable!(),
        };
        for _ in 0..(seconds / frame).round() as usize {
            app.update();
            // Pressed keys stay pressed, but only the first frame's are
            // just pressed.
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .clear();
        }
    }

    #[test]
    fn the_player_falls_and_lands_on_the_ground() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        let player = spawn(&mut app, DVec3::new(8.5, 12.0, 8.5));
        run(&mut app, 2.0);
        let body = body(&app, player);
        assert!(body.on_ground);
        assert!((body.position.y - 4.0).abs() < 1e-9, "{}", body.position);
        assert_eq!(body.velocity, DVec3::ZERO);
        // The camera is at the eyes.
        let eye = app.world().get::<Transform>(player).unwrap().translation;
        assert!((eye.y - (4.0 + EYE_HEIGHT as f32)).abs() < 1e-4, "{eye}");
    }

    /// Walking settles at Minecraft's 4.317 blocks a second, sprinting at
    /// 5.612 and sneaking at 1.295.
    #[test]
    fn walking_sprinting_and_sneaking_speeds() {
        for (keys, speed) in [
            (&[KeyCode::KeyW][..], 4.317),
            (&[KeyCode::KeyW, KeyCode::ControlLeft], 5.612),
            (&[KeyCode::KeyW, KeyCode::ShiftLeft], 4.317 * SNEAK),
        ] {
            let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
            let player = spawn(&mut app, DVec3::new(8.5, 4.0, 15.0));
            run(&mut app, 0.1);
            hold(&mut app, keys);
            run(&mut app, 1.0);
            let start = body(&app, player).position;
            run(&mut app, 0.5);
            let moved = body(&app, player).position - start;
            assert!((moved.z / 0.5 + speed).abs() < 0.01, "{keys:?}: {moved}");
            assert!(moved.x.abs() < 1e-9 && moved.y.abs() < 1e-9, "{moved}");
        }
    }

    /// A jump peaks at Minecraft's 1.25 blocks: onto a block, not two.
    #[test]
    fn jumps_are_a_block_and_a_quarter_high() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        run(&mut app, 0.1);
        hold(&mut app, &[KeyCode::Space]);
        let mut peak: f64 = 0.0;
        for _ in 0..30 {
            run(&mut app, STEP.as_secs_f64());
            peak = peak.max(body(&app, player).position.y - 4.0);
        }
        assert!((1.22..=1.2523).contains(&peak), "{peak}");
    }

    /// Walking climbs a slab, then a block on from the slab (half a block
    /// up again), and stops at a wall a block higher; with jump held, it
    /// hops up that.
    #[test]
    fn walking_climbs_slabs_and_jumping_climbs_blocks() {
        // Heading north (−Z) from z = 8.5 on a floor with its top at 4.
        let world = |pos: IVec3| match (pos.y, pos.z) {
            (y, _) if y < 4 => block("stone"),
            (4, 5..=6) => block("stone_slab"),
            (4, ..=4) | (5, ..=3) => block("stone"),
            _ => BlockState::AIR,
        };
        let mut app = app(IVec3::new(8, 8, 8), FRAME, world);
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        run(&mut app, 0.1);
        hold(&mut app, &[KeyCode::KeyW]);
        run(&mut app, 2.0);
        let stopped = body(&app, player);
        assert!(
            (stopped.position.y - 5.0).abs() < 1e-9,
            "{}",
            stopped.position
        );
        assert!(
            (stopped.position.z - 4.3).abs() < 1e-9,
            "{}",
            stopped.position
        );
        hold(&mut app, &[KeyCode::KeyW, KeyCode::Space]);
        run(&mut app, 1.0);
        let up = body(&app, player);
        assert!(up.position.z < 4.0, "{}", up.position);
        assert!(up.position.y >= 6.0 - 1e-9, "{}", up.position);
    }

    /// Sneaking, the player stops at the edge with the body still over it.
    #[test]
    fn sneaking_doesnt_walk_off_edges() {
        // A platform ending at z = 6.
        let world = |pos: IVec3| {
            if pos.y < 4 && pos.z >= 6 || pos.y < 0 {
                block("stone")
            } else {
                BlockState::AIR
            }
        };
        let mut app = app(IVec3::new(8, 8, 8), FRAME, world);
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        run(&mut app, 0.1);
        hold(&mut app, &[KeyCode::KeyW, KeyCode::ShiftLeft]);
        run(&mut app, 4.0);
        let sneaking = body(&app, player);
        assert!(sneaking.on_ground);
        assert_eq!(sneaking.position.y, 4.0);
        assert!(sneaking.position.z + 0.3 > 6.0, "{}", sneaking.position);
        // Without sneaking, off it goes.
        hold(&mut app, &[KeyCode::KeyW]);
        run(&mut app, 1.0);
        assert!(body(&app, player).position.y < 4.0);
    }

    /// Double-tapping Space flies; Space and Shift go up and down; landing
    /// stops flying.
    #[test]
    fn double_tap_flies_and_landing_stops_it() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        run(&mut app, 0.1);
        for _ in 0..2 {
            hold(&mut app, &[KeyCode::Space]);
            run(&mut app, 0.05);
            hold(&mut app, &[]);
            run(&mut app, 0.05);
        }
        assert!(self::player(&app, player).flying);
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

    /// The fixed step makes the path the same at any frame rate, including
    /// long frames from lag.
    #[test]
    fn frame_rate_doesnt_change_the_path() {
        let path = |frame: Duration| {
            let mut app = app(IVec3::new(8, 8, 8), frame, floor(4));
            let player = spawn(&mut app, DVec3::new(8.5, 9.0, 8.5));
            hold(&mut app, &[KeyCode::KeyW, KeyCode::KeyD, KeyCode::Space]);
            run(&mut app, 1.2);
            body(&app, player).position
        };
        let reference = path(STEP);
        for frame in [STEP / 2, STEP * 3, Duration::from_millis(200)] {
            let other = path(frame);
            assert!(
                other.abs_diff_eq(reference, 1e-9),
                "{frame:?}: {other} vs {reference}"
            );
        }
    }

    /// Falling for a long way, and landing on a single layer, far from the
    /// origin.
    #[test]
    fn long_falls_land_far_from_the_origin() {
        let x = 30_000_000;
        let world = move |pos: IVec3| {
            if pos.y == -12 {
                block("stone")
            } else {
                BlockState::AIR
            }
        };
        let mut app = app(IVec3::new(x, 0, x), Duration::from_millis(100), world);
        let feet = DVec3::new(x as f64 + 0.5, 20.0, x as f64 + 0.5);
        let player = spawn(&mut app, feet);
        // A fast fall, and a sideways run into nothing.
        app.world_mut().get_mut::<Body>(player).unwrap().velocity = DVec3::new(0.0, -70.0, 0.0);
        run(&mut app, 3.0);
        let landed = body(&app, player);
        assert!(landed.on_ground, "{}", landed.position);
        assert_eq!(landed.position.y, -11.0);
        assert_eq!(landed.position.x, feet.x);
    }

    /// In the unloaded world, the player waits.
    #[test]
    fn the_player_waits_where_the_world_isnt_loaded() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        let feet = DVec3::new(500.5, 9.0, 8.5);
        let player = spawn(&mut app, feet);
        hold(&mut app, &[KeyCode::KeyW]);
        run(&mut app, 1.0);
        assert_eq!(body(&app, player).position, feet);
    }
}
