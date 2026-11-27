//! The player: a body that walks, jumps, sneaks, sprints and flies through
//! the world.
//!
//! - **WASD** moves relative to where you face; looking up or down doesn't
//!   change the direction.
//! - **Space** jumps; held, it jumps again on landing. **Double-tap Space**
//!   to start or stop flying. Landing on the ground stops flying too.
//! - **Shift** sneaks: slower, and you don't walk off edges. Flying, Space
//!   rises and Shift sinks.
//! - **F** with W sprints, a little faster on foot and twice as fast
//!   flying. A sprinting jump gets a push forward. Sprinting and flying
//!   widen the view (see `fov`).
//! - **In a fluid** (water, lava) you move slower, by the fluid's
//!   `move_speed`, the deeper you're in. Chest deep you swim: you sink
//!   slowly, and **Space** swims up, so holding it keeps your head above
//!   the surface, and swimming up against a wall climbs out over it. A
//!   fall into deep water slows to a sink.
//!
//! The player is an entity of its own, [`LocalPlayer`] for the one this
//! game is played as: a [`Body`] 0.6 blocks wide and 1.8 tall that climbs
//! steps up to 0.6 (a slab) and collides with blocks as `physics`
//! describes, flying included. Its [`Transform`] is at its feet, turned to
//! face its [`Look`]'s yaw; its eyes ([`Eyes`]) are [`Player::eye_height`]
//! above, looking along the [`Look`]. Gameplay (the crosshair, mining,
//! using blocks) works from the eyes, never the camera: the camera
//! (`fly_camera`) is only a child of the local player that shows what its
//! eyes see.
//!
//! What a player does comes from its [`Controls`]: the keys fill the local
//! player's, so any other body with a [`Player`] (a mob, another player)
//! moves the same way from its own. It moves in [`FixedUpdate`]; the
//! [`Transform`] is placed between the last two steps every frame, so it
//! moves smoothly at any frame rate.
//!
//! Moving is a top speed and how quickly it's reached ([`Rule`]).
//! Velocities change exactly as they would continuously between steps, so
//! the step rate barely matters.
//!
//! Walking, sneaking, sprinting, gravity, the jump and the eye heights are
//! tuned to Minecraft's: its values are written per 50 ms [`TICK`] and
//! converted here, so the constants below say where they came from.

use std::f32::consts::FRAC_PI_2;

use bevy::{
    ecs::query::QueryData,
    math::{DQuat, DVec3},
    prelude::*,
    transform::TransformSystems,
};

use crate::{
    fluid::FluidView,
    physics::{Body, Shapes, WorldShapes, guard_edges},
};

/// Where the local player's eyes start: just above the hills, so there's
/// ground right below. The scenarios' coordinates are picked around it.
pub const START_POSITION: Vec3 = Vec3::new(64.0, 56.0, 92.0);
/// The point the local player looks at when it starts.
pub const START_TARGET: Vec3 = Vec3::new(64.0, 28.0, 56.0);
/// Keep pitch just inside ±90° so the eyes never look exactly straight up
/// or down, where yaw becomes ambiguous.
pub const MAX_PITCH: f32 = FRAC_PI_2 - 0.01;

/// Width, height and depth.
pub const SIZE: DVec3 = DVec3::new(0.6, 1.8, 0.6);
/// How high the eyes (the camera) are above the feet.
pub const EYE_HEIGHT: f64 = 1.62;
/// How high they are while sneaking.
pub const SNEAK_EYE_HEIGHT: f64 = 1.27;
/// How much of the way the eyes move to their new height each tick, when
/// the player starts or stops sneaking.
const EYE_EASING: f64 = 0.5;
/// The tallest step the player walks up without jumping: a slab, not a
/// block.
pub const STEP_HEIGHT: f64 = 0.6;
/// Two presses of Space within this many seconds start or stop flying.
pub const DOUBLE_TAP: f32 = 0.35;

/// The tick the reference values are written per.
pub const TICK: f64 = 0.05;
/// Friction on the ground, a tick: a block's slipperiness, 0.6, times 0.91.
pub const GROUND_FRICTION: f64 = 0.6 * 0.91;
/// The walking push, a tick: 0.1, times 0.98 for the input.
const WALK_PUSH: f64 = 0.1 * 0.98;

/// How the player moves in one situation: the speed it settles at with a
/// key held, and how quickly it gets there or stops. Velocity closes
/// `1 − 1/e` (63%) of the gap to its target every `response` seconds, so
/// it's 95% of the way after three.
#[derive(Debug, Clone, Copy)]
struct Rule {
    /// Blocks per second.
    speed: f64,
    /// Seconds.
    response: f64,
}

/// How much of the way to a target a value moves in `dt` seconds, when it
/// moves `fraction` of the way each tick.
pub fn per_tick(fraction: f64, dt: f64) -> f64 {
    1.0 - (1.0 - fraction).powf(dt / TICK)
}

/// How quickly velocity decays, per second, with `friction` a tick.
fn decay(friction: f64) -> f64 {
    -friction.ln() / TICK
}

impl Rule {
    /// How quickly velocity decays, per second.
    fn decay(self) -> f64 {
        1.0 / self.response
    }

    /// The acceleration, in blocks per second per second, that settles at
    /// [`Rule::speed`].
    fn accel(self) -> f64 {
        self.speed / self.response
    }
}

/// On the ground: the push and friction settle at 4.317 blocks a second,
/// and friction closes 1 − 0.546 of the gap each tick, which is a response
/// of −[`TICK`] / ln 0.546 (checked in a test).
const WALK: Rule = Rule {
    speed: WALK_PUSH / (1.0 - GROUND_FRICTION) / TICK,
    response: 0.082_626_012_904_068_79,
};
/// In the air, steering is looser, but the top speed is the same, so a
/// running jump keeps its speed.
const AIR: Rule = Rule {
    speed: WALK.speed,
    response: 0.25,
};
const FLY: Rule = Rule {
    speed: 12.0,
    response: 0.06,
};
/// Up and down, flying.
const FLY_VERTICAL: Rule = Rule {
    speed: 10.0,
    response: 0.06,
};
/// Sprinting multiplies walking and air speed by this, and doubles flying.
const SPRINT: f64 = 1.3;
/// Sneaking multiplies walking speed by this.
const SNEAK: f64 = 0.3;
/// Falling: 0.08 a tick of gravity with friction 0.98 settles at 3.92
/// blocks a tick.
const TERMINAL_SPEED: f64 = 3.92 / TICK;
const FALL_FRICTION: f64 = 0.98;
/// The jump's speed straight up, in blocks per second: with
/// [`TERMINAL_SPEED`]'s gravity it peaks 1.2522 blocks up, as 0.42 a tick
/// does. Enough for a block, not a block and a half.
pub const JUMP_SPEED: f64 = 9.2474;
/// A sprinting jump's push forward: 0.2 a tick.
const SPRINT_JUMP_BOOST: f64 = 0.2 / TICK;
/// How much of the body must be under a fluid to swim, rather than walk or
/// fall: about chest deep.
pub const SWIM_DEPTH: f64 = 0.6;
/// How much must be under to keep swimming up, with Space held: less, so
/// you rise until your feet are near enough the surface to step out onto
/// a shore level with it.
pub const SWIM_UP_DEPTH: f64 = 0.25;
/// Swimming up, in blocks per second, in a fluid with a `move_speed` of 1;
/// slower fluids scale it down.
const SWIM_UP: f64 = 4.0;
/// Sinking while swimming, likewise.
const SINK: f64 = 2.0;
/// How quickly swimming reaches its speed up or down, in seconds.
const SWIM_RESPONSE: f64 = 0.25;
/// Swimming up against a wall pushes up this fast, in blocks per second,
/// while any of the body is in the fluid: a jump's speed, enough to climb
/// out onto a side a block above the surface.
const CLIMB_OUT_SPEED: f64 = JUMP_SPEED;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<LocalPlayer>()
            .register_type::<Look>()
            .register_type::<Controls>()
            .add_systems(Update, toggle_flying.run_if(crate::ui::playing))
            .add_systems(
                FixedUpdate,
                (read_the_keys, move_player, ease_eye_height)
                    .chain()
                    .in_set(PlayerMovement),
            )
            .add_systems(
                RunFixedMainLoop,
                place_the_player.in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
            )
            // Whatever turned the look this frame (the mouse, a script's
            // `camera`), the body faces it when it's drawn. Nothing else
            // turns the body.
            .add_systems(
                PostUpdate,
                face_the_look.before(TransformSystems::Propagate),
            );
    }
}

/// Spawns the local player at [`START_POSITION`], looking at
/// [`START_TARGET`]. Separate from [`PlayerPlugin`], so tests of players
/// spawn their own.
pub struct LocalPlayerPlugin;

impl Plugin for LocalPlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_the_local_player);
    }
}

fn spawn_the_local_player(mut commands: Commands) {
    let eyes = Transform::from_translation(START_POSITION).looking_at(START_TARGET, Vec3::Y);
    let feet = feet_below(START_POSITION.as_dvec3(), EYE_HEIGHT);
    commands.spawn((
        Name::new("Player"),
        local_player(feet, Look::from_rotation(eyes.rotation)),
    ));
}

/// The local player, standing with its feet at `feet` and looking along
/// `look`.
pub fn local_player(feet: DVec3, look: Look) -> impl Bundle {
    (
        LocalPlayer,
        Player::default(),
        look,
        Body::new(feet, SIZE, STEP_HEIGHT),
        Transform::from_translation(feet.as_vec3()).with_rotation(look.yaw_rotation()),
    )
}

/// Where the feet are for eyes at `eyes`, `eye_height` above them.
pub fn feet_below(eyes: DVec3, eye_height: f64) -> DVec3 {
    eyes - DVec3::Y * eye_height
}

/// Puts a body's eyes at `eyes` at once, `eye_height` above its feet: the
/// body jumps there (it isn't drawn sliding), and so does its
/// [`Transform`]. Which way it looks is its [`Look`]'s, which turns the
/// body when it's drawn.
pub fn put_eyes(eyes: DVec3, eye_height: f64, body: &mut Body, transform: &mut Transform) {
    let feet = feet_below(eyes, eye_height);
    body.teleport(feet);
    transform.translation = feet.as_vec3();
}

/// Which way a body looks: its yaw turns the whole body (and its
/// [`Transform`]), its pitch only the head.
#[derive(Component, Reflect, Debug, Clone, Copy, Default, PartialEq)]
#[reflect(Component)]
pub struct Look {
    /// Rotation around the world Y axis, in radians.
    pub yaw: f32,
    /// Rotation up (+) or down (-), in radians, within ±[`MAX_PITCH`].
    pub pitch: f32,
}

impl Look {
    /// The look that faces the way `rotation` does, its roll left out.
    pub fn from_rotation(rotation: Quat) -> Self {
        let (yaw, pitch, _) = rotation.to_euler(EulerRot::YXZ);
        Self {
            yaw,
            pitch: pitch.clamp(-MAX_PITCH, MAX_PITCH),
        }
    }

    /// The eyes' rotation, from yaw and pitch. No roll, ever.
    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }

    /// The body's rotation: the yaw alone.
    pub fn yaw_rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw)
    }

    /// The head's rotation on the body: the pitch alone.
    pub fn pitch_rotation(&self) -> Quat {
        Quat::from_rotation_x(self.pitch)
    }

    /// Turns by `yaw` and `pitch` radians, keeping the pitch within
    /// ±[`MAX_PITCH`].
    pub fn turn(&mut self, yaw: f32, pitch: f32) {
        self.yaw += yaw;
        self.pitch = (self.pitch + pitch).clamp(-MAX_PITCH, MAX_PITCH);
    }
}

/// What a player is trying to do, for its next physics step: for the local
/// player, the keys held.
#[derive(Component, Reflect, Debug, Clone, Copy, Default, PartialEq)]
#[reflect(Component)]
pub struct Controls {
    /// Forward (+1) or back (−1), relative to where it faces.
    pub forward: f64,
    /// Right (+1) or left (−1).
    pub right: f64,
    /// Jumping, or rising when flying, or swimming up.
    pub jump: bool,
    /// Sneaking, or sinking when flying.
    pub sneak: bool,
    pub sprint: bool,
    /// Start or stop flying, on the next physics step, which clears it.
    pub toggle_fly: bool,
}

/// Where a player's eyes are and which way they look: what it aims from.
#[derive(QueryData)]
pub struct Eyes {
    transform: &'static Transform,
    player: &'static Player,
    look: &'static Look,
}

impl EyesItem<'_, '_> {
    /// The eyes, in the world: at the eye height drawn this frame above the
    /// feet, looking along the [`Look`].
    pub fn transform(&self) -> Transform {
        eyes(self.transform, self.player, self.look)
    }
}

/// The eyes of a player whose feet are at `transform`.
pub fn eyes(transform: &Transform, player: &Player, look: &Look) -> Transform {
    Transform::from_translation(transform.translation + Vec3::Y * player.drawn_eye_height as f32)
        .with_rotation(look.rotation())
}

/// The player's physics step, in [`FixedUpdate`]. Whatever follows how the
/// player moved runs after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlayerMovement;

/// The player this game is played as, as opposed to other players and
/// mobs. Features that act for whoever is playing (the hotbar, the
/// inventory screen, placing blocks) find the player with this. The
/// camera (`fly_camera`) is its child.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
#[require(crate::use_action::UseAction)]
pub struct LocalPlayer;

/// The player's movement state. Its [`Body`] is on the same entity.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
#[require(Look, Controls, Transform, Visibility)]
pub struct Player {
    pub flying: bool,
    pub sneaking: bool,
    pub sprinting: bool,
    /// How high the eyes are above the feet: [`EYE_HEIGHT`], or
    /// [`SNEAK_EYE_HEIGHT`] sneaking, easing between the two.
    pub eye_height: f64,
    /// [`Player::eye_height`] before the last physics step, for drawing it
    /// smoothly between steps.
    pub previous_eye_height: f64,
    /// The eye height drawn this frame, between the last two steps'.
    pub drawn_eye_height: f64,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            flying: false,
            sneaking: false,
            sprinting: false,
            eye_height: EYE_HEIGHT,
            previous_eye_height: EYE_HEIGHT,
            drawn_eye_height: EYE_HEIGHT,
        }
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

/// How a velocity component changes over `dt` with constant `accel` and
/// exponential `decay`: the new velocity, and how far it went.
fn drift(velocity: f64, accel: f64, decay: f64, dt: f64) -> (f64, f64) {
    let settled = accel / decay;
    let fade = (-decay * dt).exp();
    let new = settled + (velocity - settled) * fade;
    let distance = settled * dt + (velocity - settled) * (1.0 - fade) / decay;
    (new, distance)
}

/// The keys held, as the local player's [`Controls`].
fn read_the_keys(
    keys: Res<ButtonInput<KeyCode>>,
    screens: Query<(), With<crate::ui::UiScreen>>,
    mut players: Query<&mut Controls, With<LocalPlayer>>,
) {
    // With a screen open, keys are for it: the player stops pressing them
    // but still falls and slides.
    let playing = crate::ui::playing(screens);
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

/// One physics step of each player: its controls to velocity, then
/// through the world.
fn move_player(
    time: Res<Time>,
    shapes: WorldShapes,
    fluids: FluidView,
    mut players: Query<(&Look, &mut Controls, &mut Player, &mut Body)>,
) {
    let dt = time.delta_secs_f64();
    for (look, mut controls, mut player, mut body) in &mut players {
        if dt == 0.0 {
            continue;
        }
        if controls.toggle_fly {
            controls.toggle_fly = false;
            player.flying = !player.flying;
            body.velocity.y = 0.0;
        }
        let Controls {
            forward: forward_input,
            right: right_input,
            jump,
            sneak: shift,
            sprint,
            toggle_fly: _,
        } = *controls;
        // In the unloaded world, wait for it to load.
        if shapes.shape(body.position.floor().as_ivec3()).is_none() {
            body.velocity = DVec3::ZERO;
            continue;
        }
        player.sneaking = shift && !player.flying;
        player.sprinting = sprint && forward_input > 0.0 && !player.sneaking;

        // Horizontal movement follows yaw only.
        let yaw = DQuat::from_rotation_y(look.yaw as f64);
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
        // In a fluid: slower the deeper, down to its `move_speed` waist
        // deep; swimming once it's chest deep.
        let (min, max) = (body.aabb().min, body.aabb().max);
        let immersion = (!player.flying)
            .then(|| fluids.immersion(body.position.x, body.position.z, min.y, max.y))
            .flatten()
            .and_then(|(kind, depth)| Some((fluids.fluid(kind)?.move_speed as f64, depth)));
        let mut swim = None;
        if let Some((move_speed, depth)) = immersion {
            boost *= 1.0 - (1.0 - move_speed) * (depth * 2.0).min(1.0);
            if depth >= if jump { SWIM_UP_DEPTH } else { SWIM_DEPTH } {
                swim = Some(move_speed);
            }
        }
        let mut velocity = body.velocity;
        if !player.flying && body.on_ground && jump && swim.is_none() {
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
        } else if let Some(move_speed) = swim {
            let target = if jump { SWIM_UP } else { -SINK } * move_speed;
            drift(velocity.y, target / SWIM_RESPONSE, 1.0 / SWIM_RESPONSE, dt)
        } else {
            let decay = decay(FALL_FRICTION);
            drift(velocity.y, -TERMINAL_SPEED * decay, decay, dt)
        };

        // Not when jumping: a sneaking jump off an edge carries on over it.
        if player.sneaking && body.on_ground && motion.y <= 0.0 {
            let guarded = guard_edges(body.aabb(), motion, body.step_height, &shapes);
            for axis in [0, 2] {
                if guarded[axis] != motion[axis] {
                    velocity[axis] = 0.0;
                }
            }
            motion = guarded;
        }
        let before = body.position;
        body.advance(motion, velocity, &shapes);
        // Swimming up into a wall climbs out over it.
        let moved = body.position - before;
        let blocked = (moved.xz() - motion.xz()).length() > 1e-6;
        if immersion.is_some_and(|(_, depth)| depth > 0.0) && jump && blocked {
            body.velocity.y = body.velocity.y.max(CLIMB_OUT_SPEED);
        }
        if player.flying && body.on_ground {
            player.flying = false;
        }
    }
}

/// Moves the eyes part of the way to where they should be: lower when
/// sneaking. [`EYE_EASING`] is per tick, so it's spread over the steps.
fn ease_eye_height(time: Res<Time>, mut players: Query<&mut Player>) {
    let ease = per_tick(EYE_EASING, time.delta_secs_f64());
    for mut player in &mut players {
        let target = if player.sneaking {
            SNEAK_EYE_HEIGHT
        } else {
            EYE_HEIGHT
        };
        player.previous_eye_height = player.eye_height;
        player.eye_height += (target - player.eye_height) * ease;
    }
}

/// Puts each player's [`Transform`] at its feet, and its eyes at their
/// height, between the last two physics steps, so they move smoothly
/// whatever the frame rate.
fn place_the_player(
    time: Res<Time<Fixed>>,
    mut players: Query<(&Body, &mut Player, &mut Transform)>,
) {
    let fraction = time.overstep_fraction_f64();
    for (body, mut player, mut transform) in &mut players {
        player.drawn_eye_height = player.previous_eye_height
            + (player.eye_height - player.previous_eye_height) * fraction;
        transform.translation = body.interpolated(fraction).as_vec3();
    }
}

/// Turns each body to face its look's yaw.
fn face_the_look(mut bodies: Query<(&Look, &mut Transform), Changed<Look>>) {
    for (look, mut transform) in &mut bodies {
        transform.rotation = look.yaw_rotation();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::Duration;

    use bevy::time::TimeUpdateStrategy;

    use super::*;
    use crate::{
        content::{BlockState, base, block},
        physics::{PhysicsPlugin, STEP},
        testing::insert_chunk,
        world::{CHUNK_VOLUME, Chunk, ChunkCoord, ChunkMap},
    };

    /// The player and its systems in a world of the chunks around
    /// `around` (3×3×3 of them), filled by `fill`, with frames of `frame`.
    pub(crate) fn app(around: IVec3, frame: Duration, fill: impl Fn(IVec3) -> BlockState) -> App {
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
                        chunk.put(local, fill(coord.origin() + local));
                    }
                    insert_chunk(&mut app, coord.0, chunk);
                }
            }
        }
        app
    }

    /// A flat stone floor whose top is at `y = top`.
    pub(crate) fn floor(top: i32) -> impl Fn(IVec3) -> BlockState {
        move |pos| {
            if pos.y < top {
                block("stone")
            } else {
                BlockState::AIR
            }
        }
    }

    pub(crate) const FRAME: Duration = STEP;

    /// Spawns the local player with its feet at `feet`, facing north (−Z).
    pub(crate) fn spawn(app: &mut App, feet: DVec3) -> Entity {
        let entity = app
            .world_mut()
            .spawn(local_player(feet, Look::default()))
            .id();
        app.update();
        entity
    }

    /// The local player with its eyes at `eyes`, looking the way they face.
    pub(crate) fn local_player_at(eyes: Transform) -> impl Bundle {
        local_player(
            feet_below(eyes.translation.as_dvec3(), EYE_HEIGHT),
            Look::from_rotation(eyes.rotation),
        )
    }

    /// Puts a player's eyes at `eyes` at once, keeping its eye height, and
    /// turns it to look the way they face.
    pub(crate) fn put_eyes(world: &mut World, player: Entity, eyes: Transform) {
        let mut entity = world.entity_mut(player);
        entity.insert(Look::from_rotation(eyes.rotation));
        let height = entity.get::<Player>().unwrap().drawn_eye_height;
        let eyes = eyes.translation.as_dvec3();
        let mut transform = *entity.get::<Transform>().unwrap();
        match entity.get_mut::<Body>() {
            Some(mut body) => super::put_eyes(eyes, height, &mut body, &mut transform),
            // Some tests' players are only eyes, with no body in the way.
            None => transform.translation = feet_below(eyes, height).as_vec3(),
        }
        entity.insert(transform);
    }

    /// The local player.
    pub(crate) fn local(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<LocalPlayer>>()
            .single(app.world())
            .unwrap()
    }

    /// Where the local player's eyes are, and which way they look.
    pub(crate) fn local_eyes(app: &mut App) -> Transform {
        let player = local(app);
        let world = app.world();
        eyes(
            world.get::<Transform>(player).unwrap(),
            world.get::<Player>(player).unwrap(),
            world.get::<Look>(player).unwrap(),
        )
    }

    /// Puts the local player's eyes at `eyes`, looking the way they face.
    pub(crate) fn put_local_eyes(app: &mut App, eyes: Transform) {
        let player = local(app);
        put_eyes(app.world_mut(), player, eyes);
    }

    /// Where a player's eyes are drawn this frame.
    pub(crate) fn eye(app: &App, player: Entity) -> Vec3 {
        let world = app.world();
        eyes(
            world.get::<Transform>(player).unwrap(),
            world.get::<Player>(player).unwrap(),
            world.get::<Look>(player).unwrap(),
        )
        .translation
    }

    fn body(app: &App, player: Entity) -> Body {
        app.world().get::<Body>(player).unwrap().clone()
    }

    fn player(app: &App, entity: Entity) -> Player {
        app.world().get::<Player>(entity).unwrap().clone()
    }

    pub(crate) fn hold(app: &mut App, keys: &[KeyCode]) {
        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        input.release_all();
        input.clear();
        for &key in keys {
            input.press(key);
        }
    }

    pub(crate) fn run(app: &mut App, seconds: f64) {
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
        // The body's transform is at its feet, and the eyes above them.
        let feet = app.world().get::<Transform>(player).unwrap().translation;
        assert!((feet.y - 4.0).abs() < 1e-4, "{feet}");
        let eye = eye(&app, player);
        assert!((eye.y - (4.0 + EYE_HEIGHT as f32)).abs() < 1e-4, "{eye}");
    }

    /// Walking, sprinting and sneaking settle at their speeds.
    #[test]
    fn walking_sprinting_and_sneaking_speeds() {
        for (keys, speed) in [
            (&[KeyCode::KeyW][..], WALK.speed),
            (&[KeyCode::KeyW, KeyCode::KeyF], WALK.speed * SPRINT),
            (&[KeyCode::KeyW, KeyCode::ShiftLeft], WALK.speed * SNEAK),
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

    /// Flying settles at its speed, and twice that sprinting. Ctrl with S isn't sprinting.
    #[test]
    fn flying_and_backwards_speeds() {
        for (flying, keys, speed) in [
            (true, &[KeyCode::KeyW][..], -FLY.speed),
            (true, &[KeyCode::KeyW, KeyCode::KeyF], -FLY.speed * 2.0),
            (false, &[KeyCode::KeyS, KeyCode::KeyF], WALK.speed),
        ] {
            let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
            let player = spawn(
                &mut app,
                DVec3::new(8.5, if flying { 10.0 } else { 4.0 }, 8.5),
            );
            app.world_mut().get_mut::<Player>(player).unwrap().flying = flying;
            run(&mut app, 0.1);
            hold(&mut app, keys);
            run(&mut app, 0.4);
            // Back to where it started now and then, so it doesn't run out
            // of loaded world: speeds don't depend on where.
            for _ in 0..6 {
                app.world_mut().get_mut::<Body>(player).unwrap().position.z = 8.5;
                run(&mut app, 0.5);
            }
            app.world_mut().get_mut::<Body>(player).unwrap().position.z = 8.5;
            let start = self::body(&app, player).position;
            run(&mut app, 0.25);
            let moved = self::body(&app, player).position - start;
            assert!(
                (moved.z / 0.25 - speed).abs() < 0.02 * speed.abs(),
                "{keys:?}: {moved}"
            );
            assert!(!self::player(&app, player).sprinting || flying);
        }
    }

    /// Walking settles at the push and friction's top speed, with the
    /// response that friction gives.
    #[test]
    fn walking_matches_its_push_and_friction() {
        assert!((WALK.speed - 4.317).abs() < 1e-3, "{}", WALK.speed);
        let response = -TICK / GROUND_FRICTION.ln();
        assert!((WALK.response - response).abs() < 1e-12, "{response}");
    }

    /// Sneaking lowers the eyes, easing down over a few ticks rather than
    /// at once, and standing up raises them again.
    #[test]
    fn sneaking_lowers_the_eyes() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        run(&mut app, 0.1);
        let eye = |app: &App| eye(app, player).y;
        assert!((eye(&app) - (4.0 + EYE_HEIGHT as f32)).abs() < 1e-4);
        hold(&mut app, &[KeyCode::ShiftLeft]);
        run(&mut app, TICK);
        let partway = eye(&app);
        assert!(
            partway < 4.0 + EYE_HEIGHT as f32 && partway > 4.0 + SNEAK_EYE_HEIGHT as f32 + 0.1,
            "{partway}"
        );
        run(&mut app, 0.5);
        assert!((eye(&app) - (4.0 + SNEAK_EYE_HEIGHT as f32)).abs() < 1e-3);
        hold(&mut app, &[]);
        run(&mut app, 0.5);
        assert!((eye(&app) - (4.0 + EYE_HEIGHT as f32)).abs() < 1e-3);
    }

    /// Letting go stops the player quickly: within two fifths of a block
    /// walking and a block flying.
    #[test]
    fn letting_go_stops_quickly() {
        for (flying, limit) in [(false, 0.4), (true, 1.0)] {
            let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
            let player = spawn(
                &mut app,
                DVec3::new(8.5, if flying { 10.0 } else { 4.0 }, 15.0),
            );
            app.world_mut().get_mut::<Player>(player).unwrap().flying = flying;
            run(&mut app, 0.1);
            hold(&mut app, &[KeyCode::KeyW]);
            run(&mut app, 0.6);
            hold(&mut app, &[]);
            let from = body(&app, player).position;
            run(&mut app, 0.5);
            let after = body(&app, player);
            let slid = from.z - after.position.z;
            assert!(slid < limit, "flying {flying}: slid {slid}");
            assert!(
                after.velocity.length() < 0.5,
                "flying {flying}: {}",
                after.velocity
            );
        }
    }

    /// In the air, the player steers far less than on the ground.
    #[test]
    fn air_control_is_weak() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        run(&mut app, 0.1);
        hold(&mut app, &[KeyCode::Space]);
        run(&mut app, STEP.as_secs_f64());
        hold(&mut app, &[KeyCode::KeyW]);
        run(&mut app, 0.25);
        let body = body(&app, player);
        assert!(!body.on_ground);
        // Air: 4.317 × (1 − e^(−0.25 / 0.25)) ≈ 2.73; the ground would be
        // 4.317 × (1 − e^(−0.25 / 0.0826)) ≈ 4.10.
        assert!((2.5..3.0).contains(&-body.velocity.z), "{}", body.velocity);
    }

    /// Sprint-jumping gets a push forward, so it's faster than sprinting.
    #[test]
    fn sprint_jumping_beats_sprinting() {
        let distance = |keys: &[KeyCode]| {
            let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
            let player = spawn(&mut app, DVec3::new(8.5, 4.0, 23.0));
            run(&mut app, 0.1);
            hold(&mut app, keys);
            run(&mut app, 1.5);
            8.5 - body(&app, player).position.z
        };
        let sprint = distance(&[KeyCode::KeyW, KeyCode::KeyF]);
        let jump = distance(&[KeyCode::KeyW, KeyCode::KeyF, KeyCode::Space]);
        assert!(jump > sprint + 0.5, "{jump} vs {sprint}");
    }

    /// Jumping under a low ceiling bumps the head and comes straight back
    /// down, rather than sticking to it.
    #[test]
    fn ceilings_stop_jumps() {
        // Floor top at 4, ceiling from 6: 0.2 above the head.
        let world = |pos: IVec3| match pos.y {
            ..4 | 6 => block("stone"),
            _ => BlockState::AIR,
        };
        let mut app = app(IVec3::new(8, 8, 8), FRAME, world);
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        run(&mut app, 0.1);
        hold(&mut app, &[KeyCode::Space]);
        run(&mut app, STEP.as_secs_f64());
        hold(&mut app, &[]);
        let mut peak: f64 = 0.0;
        for _ in 0..12 {
            run(&mut app, STEP.as_secs_f64());
            let body = body(&app, player);
            peak = peak.max(body.position.y);
            if body.position.y > 4.2 - 1e-9 {
                assert!(body.velocity.y <= 0.0, "{}", body.velocity);
            }
        }
        assert!((peak - 4.2).abs() < 1e-9, "{peak}");
        assert!(body(&app, player).on_ground, "back down within 0.2 s");
    }

    /// A jump peaks at 1.25 blocks: onto a block, not two.
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
        // Held at the edge, it doesn't build up speed to fly off with.
        assert_eq!(sneaking.velocity.z, 0.0);
        assert_eq!(sneaking.position.y, 4.0);
        assert!(sneaking.position.z + 0.3 > 6.0, "{}", sneaking.position);
        // A sneaking jump carries on over the edge.
        hold(
            &mut app,
            &[KeyCode::KeyW, KeyCode::ShiftLeft, KeyCode::Space],
        );
        run(&mut app, 0.3);
        // All of the body past the edge at z = 6.
        let past = body(&app, player).position;
        assert!(past.z + 0.3 < 6.0, "{past}");
    }

    /// Without sneaking, the player walks off.
    #[test]
    fn walking_goes_off_edges() {
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
        hold(&mut app, &[KeyCode::KeyW]);
        run(&mut app, 1.5);
        assert!(body(&app, player).position.y < 4.0);
    }

    /// Double-tapping Space flies; Space and Shift go up and down; landing
    /// stops flying.
    #[test]
    fn double_tap_flies_and_landing_stops_it() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
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
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
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

    /// Between physics steps, the eyes are drawn part of the way from the
    /// last position to the new one.
    #[test]
    fn the_eyes_are_drawn_between_steps() {
        let mut app = app(IVec3::new(8, 8, 8), STEP / 2, floor(4));
        let player = spawn(&mut app, DVec3::new(8.5, 10.0, 8.5));
        app.world_mut().get_mut::<Player>(player).unwrap().flying = true;
        hold(&mut app, &[KeyCode::KeyW]);
        run(&mut app, 0.5);
        let mut fractions = Vec::new();
        for _ in 0..4 {
            run(&mut app, STEP.as_secs_f64() / 2.0);
            let body = body(&app, player);
            let eye = eye(&app, player);
            let (from, to) = (body.previous.z, body.position.z);
            assert!(to < from, "moving north");
            fractions.push((eye.z as f64 - from) / (to - from));
        }
        // Every other frame has no step, so it's drawn half way.
        assert!(
            fractions.iter().any(|f| (f - 0.5).abs() < 0.01),
            "{fractions:?}"
        );
        assert!(
            fractions.iter().all(|f| (-0.01..=1.01).contains(f)),
            "{fractions:?}"
        );
    }

    /// Between physics steps, the eyes are drawn part of the way from the
    /// last step's height to the new one's as they lower for sneaking.
    #[test]
    fn the_eye_height_is_drawn_between_steps() {
        let mut app = app(IVec3::new(8, 8, 8), STEP / 2, floor(4));
        let entity = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        run(&mut app, 0.1);
        hold(&mut app, &[KeyCode::ShiftLeft]);
        run(&mut app, STEP.as_secs_f64());
        let mut fractions = Vec::new();
        for _ in 0..4 {
            run(&mut app, STEP.as_secs_f64() / 2.0);
            let player = player(&app, entity);
            let (from, to) = (player.previous_eye_height, player.eye_height);
            assert!(to < from, "lowering");
            let eye = eye(&app, entity);
            fractions.push((eye.y as f64 - 4.0 - from) / (to - from));
        }
        // Every other frame has no step, so it's drawn half way.
        assert!(
            fractions.iter().filter(|f| (*f - 0.5).abs() < 0.01).count() == 2,
            "{fractions:?}"
        );
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
        // A fast fall, drifting sideways.
        app.world_mut().get_mut::<Body>(player).unwrap().velocity = DVec3::new(3.0, -70.0, 0.0);
        run(&mut app, 3.0);
        let landed = body(&app, player);
        assert!(landed.on_ground, "{}", landed.position);
        assert_eq!(landed.position.y, -11.0);
        assert!(landed.position.x > feet.x + 0.1, "{}", landed.position);
        assert_eq!(landed.position.z, feet.z);
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

    /// Stone below `floor`, then water up to (not including) `top`.
    fn pool(floor: i32, top: i32) -> impl Fn(IVec3) -> BlockState {
        move |pos| {
            if pos.y < floor {
                block("stone")
            } else if pos.y < top {
                block("water")
            } else {
                BlockState::AIR
            }
        }
    }

    /// Water half as deep as the player slows walking to its
    /// `move_speed`: half.
    #[test]
    fn wading_is_slower() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, pool(4, 5));
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 15.0));
        run(&mut app, 0.1);
        hold(&mut app, &[KeyCode::KeyW]);
        run(&mut app, 1.0);
        let start = body(&app, player).position;
        run(&mut app, 0.5);
        let moved = body(&app, player).position - start;
        assert!((moved.z / 0.5 + WALK.speed * 0.5).abs() < 0.01, "{moved}");
    }

    /// In deep water the player sinks slowly, and holding Space swims up to
    /// the surface and stays there, feet near enough it to step out.
    #[test]
    fn swimming_sinks_slowly_and_space_swims_up() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, pool(0, 12));
        let player = spawn(&mut app, DVec3::new(8.5, 8.0, 8.5));
        run(&mut app, 2.0);
        let sinking = body(&app, player).velocity.y;
        assert!((sinking + SINK * 0.5).abs() < 0.05, "sinks at {sinking}");

        hold(&mut app, &[KeyCode::Space]);
        run(&mut app, 6.0);
        let mut lowest = f64::MAX;
        let mut highest = f64::MIN;
        for _ in 0..120 {
            run(&mut app, FRAME.as_secs_f64());
            let feet = body(&app, player).position.y;
            lowest = lowest.min(feet);
            highest = highest.max(feet);
        }
        // The surface is at 12. Floating there, the feet stay under it, but
        // within a step of it.
        assert!(highest < 12.0, "{highest}");
        assert!(lowest > 12.0 - STEP_HEIGHT - 0.05, "{lowest}");
    }

    /// Swimming up against the side of a pool, its top a block above the
    /// water, climbs out onto it.
    #[test]
    fn swimming_into_a_wall_climbs_out() {
        // Water below 12 for z under 12, and stone up to 13 beyond.
        let mut app = app(IVec3::new(8, 8, 8), FRAME, |pos: IVec3| {
            if pos.y < 0 || (pos.z >= 12 && pos.y < 13) {
                block("stone")
            } else if pos.y < 12 {
                block("water")
            } else {
                BlockState::AIR
            }
        });
        let player = spawn(&mut app, DVec3::new(8.5, 8.0, 8.5));
        // Facing +Z.
        app.world_mut().get_mut::<Look>(player).unwrap().yaw = std::f32::consts::PI;
        hold(&mut app, &[KeyCode::Space, KeyCode::KeyW]);
        run(&mut app, 6.0);
        let feet = body(&app, player).position;
        assert!(feet.z > 12.3 && feet.y >= 13.0, "{feet}");
    }

    /// A long fall into deep water slows to a sink rather than hitting the
    /// bottom at full speed.
    #[test]
    fn deep_water_breaks_a_fall() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, pool(-8, 12));
        let player = spawn(&mut app, DVec3::new(8.5, 22.0, 8.5));
        run(&mut app, 2.0);
        let body = body(&app, player);
        assert!(body.position.y < 12.0, "in the water: {}", body.position.y);
        assert!(body.velocity.y > -3.0, "slowed: {}", body.velocity.y);
    }

    /// Another body, with no camera and not the local player, walks with
    /// the same physics from its own controls, and the keys move only the
    /// local player.
    #[test]
    fn a_second_body_walks_without_a_camera() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        let local = spawn(&mut app, DVec3::new(8.5, 4.0, 15.0));
        let other = app
            .world_mut()
            .spawn((
                Player::default(),
                Body::new(DVec3::new(4.5, 4.0, 15.0), SIZE, STEP_HEIGHT),
                Controls {
                    forward: 1.0,
                    ..default()
                },
            ))
            .id();
        assert!(app.world().get::<LocalPlayer>(other).is_none());
        run(&mut app, 1.1);
        let start = body(&app, other).position;
        run(&mut app, 0.5);
        let moved = body(&app, other).position - start;
        assert!((moved.z / 0.5 + WALK.speed).abs() < 0.01, "{moved}");
        assert_eq!(body(&app, local).position.z, 15.0, "no keys held");
        // Its transform follows its body, as the local player's does.
        let feet = app.world().get::<Transform>(other).unwrap().translation;
        assert!(feet.distance(body(&app, other).previous.as_vec3()) < 1e-4);

        // Keys held move the local player, and leave the other's controls
        // alone: turned to face east, it walks east.
        app.world_mut().get_mut::<Look>(other).unwrap().yaw = -std::f32::consts::FRAC_PI_2;
        hold(&mut app, &[KeyCode::KeyD]);
        run(&mut app, 1.0);
        assert!(body(&app, local).position.x > 8.5 + 2.0);
        let other_now = body(&app, other).velocity;
        assert!((other_now.x - WALK.speed).abs() < 0.01, "{other_now}");
        assert_eq!(
            *app.world().get::<Controls>(other).unwrap(),
            Controls {
                forward: 1.0,
                ..default()
            }
        );
    }

    /// A body that isn't the local player starts and stops flying when its
    /// controls ask, once for each request.
    #[test]
    fn controls_toggle_flying() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        let other = app
            .world_mut()
            .spawn((
                Player::default(),
                Body::new(DVec3::new(4.5, 4.0, 8.5), SIZE, STEP_HEIGHT),
                Controls {
                    toggle_fly: true,
                    jump: true,
                    ..default()
                },
            ))
            .id();
        run(&mut app, 0.5);
        assert!(self::player(&app, other).flying);
        assert!(!app.world().get::<Controls>(other).unwrap().toggle_fly);
        // Flying with jump held rises; it didn't jump and land.
        assert!(
            body(&app, other).position.y > 5.0,
            "{}",
            body(&app, other).position
        );
        app.world_mut()
            .get_mut::<Controls>(other)
            .unwrap()
            .toggle_fly = true;
        run(&mut app, STEP.as_secs_f64());
        assert!(!self::player(&app, other).flying);
    }

    /// The local player's eyes look along its [`Look`], and only the yaw
    /// turns its body.
    #[test]
    fn the_body_turns_with_the_yaw_and_the_eyes_with_the_pitch() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        let look = Look {
            yaw: 0.7,
            pitch: -0.4,
        };
        *app.world_mut().get_mut::<Look>(player).unwrap() = look;
        app.update();
        let transform = *app.world().get::<Transform>(player).unwrap();
        assert!(transform.rotation.angle_between(Quat::from_rotation_y(0.7)) < 1e-5);
        let world = app.world();
        let eyes = eyes(
            &transform,
            world.get::<Player>(player).unwrap(),
            world.get::<Look>(player).unwrap(),
        );
        let forward = Quat::from_euler(EulerRot::YXZ, 0.7, -0.4, 0.0) * Vec3::NEG_Z;
        assert!(eyes.forward().distance(forward) < 1e-5, "{eyes:?}");
        assert!(
            (eyes.translation - transform.translation).distance(Vec3::Y * EYE_HEIGHT as f32) < 1e-5
        );
    }

    /// Turning never pitches past [`MAX_PITCH`], and a look from a rotation
    /// faces the same way.
    #[test]
    fn looks_clamp_their_pitch_and_round_trip() {
        let mut look = Look::default();
        look.turn(0.2, 10.0);
        assert_eq!(look.pitch, MAX_PITCH);
        look.turn(0.0, -20.0);
        assert_eq!(look.pitch, -MAX_PITCH);
        let rotation = Transform::IDENTITY
            .looking_to(Vec3::new(1.0, -0.5, 0.3), Vec3::Y)
            .rotation;
        let back = Look::from_rotation(rotation).rotation();
        assert!((back * Vec3::NEG_Z).distance(rotation * Vec3::NEG_Z) < 1e-5);
    }

    /// Putting the eyes somewhere moves the body under them at once.
    #[test]
    fn putting_the_eyes_moves_the_body() {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        let to = Transform::from_xyz(10.0, 9.0, 7.0).looking_to(Vec3::X, Vec3::Y);
        put_eyes(app.world_mut(), player, to);
        let body = body(&app, player);
        let feet = DVec3::new(10.0, 9.0 - EYE_HEIGHT, 7.0);
        assert!(body.position.distance(feet) < 1e-5, "{}", body.position);
        assert!(eye(&app, player).distance(to.translation) < 1e-5);
        let look = *app.world().get::<Look>(player).unwrap();
        assert!((look.rotation() * Vec3::NEG_Z).distance(Vec3::X) < 1e-5);
    }
}
