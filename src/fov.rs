//! The field of view widens while the player sprints or flies, as in
//! Minecraft: [`SPRINT_WIDENING`] sprinting (Minecraft's 30% faster
//! movement, half of it), times [`FLYING_WIDENING`] flying. The widening
//! eases [`EASING`] of the way to its target each tick, so it swells and
//! settles over a few tenths of a second. It's kept per physics step and
//! drawn between steps, like the player's position.
//!
//! It scales the [`EyeView`]'s projection from [`FOV`], so it changes what
//! the player sees but not what they aim at.

use bevy::prelude::*;

use crate::{
    fly_camera::{EyeView, FOV},
    player::{Player, PlayerMovement, per_tick},
};

/// How much wider the view is while sprinting.
pub const SPRINT_WIDENING: f64 = 1.15;
/// How much wider the view is while flying.
pub const FLYING_WIDENING: f64 = 1.1;
/// How much of the way the widening moves to its target each tick.
const EASING: f64 = 0.5;

pub struct FovPlugin;

impl Plugin for FovPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(give_the_player_a_widening)
            .add_systems(FixedUpdate, ease_widening.after(PlayerMovement))
            .add_systems(
                RunFixedMainLoop,
                widen_the_view.in_set(RunFixedMainLoopSystems::AfterFixedMainLoop),
            );
    }
}

/// How much wider than [`FOV`] the view is, now and before the last
/// physics step.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct Widening {
    pub scale: f64,
    pub previous_scale: f64,
}

impl Default for Widening {
    fn default() -> Self {
        Self {
            scale: 1.0,
            previous_scale: 1.0,
        }
    }
}

impl Widening {
    /// The field of view in radians, `fraction` of the way through the
    /// step after the last one.
    pub fn fov(&self, fraction: f64) -> f32 {
        let scale = self.previous_scale + (self.scale - self.previous_scale) * fraction;
        FOV.to_radians() * scale as f32
    }
}

/// The widening the player's movement calls for.
pub fn target(player: &Player) -> f64 {
    let mut target = 1.0;
    if player.flying {
        target *= FLYING_WIDENING;
    }
    if player.sprinting {
        target *= SPRINT_WIDENING;
    }
    target
}

fn give_the_player_a_widening(add: On<Add<Player>>, mut commands: Commands) {
    commands
        .entity(add.entity)
        .insert_if_new(Widening::default());
}

/// After each physics step: eases the widening towards the player's.
fn ease_widening(time: Res<Time>, mut players: Query<(&Player, &mut Widening)>) {
    let ease = per_tick(EASING, time.delta_secs_f64());
    for (player, mut widening) in &mut players {
        widening.previous_scale = widening.scale;
        widening.scale += (target(player) - widening.scale) * ease;
    }
}

/// Sets each eye view's field of view from the player it's the eyes of
/// (its nearest ancestor with a [`Widening`]), between the last two
/// physics steps.
fn widen_the_view(
    time: Res<Time<Fixed>>,
    players: Query<&Widening>,
    parents: Query<&ChildOf>,
    mut views: Query<(Entity, &mut Projection), With<EyeView>>,
) {
    let fraction = time.overstep_fraction_f64();
    for (view, mut projection) in &mut views {
        let Some(widening) = parents
            .iter_ancestors(view)
            .find_map(|ancestor| players.get(ancestor).ok())
        else {
            continue;
        };
        if let Projection::Perspective(perspective) = &mut *projection {
            let fov = widening.fov(fraction);
            // Only on a change, so the camera isn't marked changed
            // every frame.
            if perspective.fov != fov {
                perspective.fov = fov;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::tests::{FRAME, app, floor, hold, run, spawn};
    use bevy::math::DVec3;

    fn fov(app: &App, view: Entity) -> f32 {
        match app.world().get::<Projection>(view).unwrap() {
            Projection::Perspective(perspective) => perspective.fov.to_degrees(),
            _ => unreachable!(),
        }
    }

    fn setup() -> (App, Entity, Entity) {
        let mut app = app(IVec3::new(8, 8, 8), FRAME, floor(4));
        app.add_plugins(FovPlugin);
        let player = spawn(&mut app, DVec3::new(8.5, 4.0, 8.5));
        let view = app
            .world_mut()
            .spawn((
                EyeView,
                Projection::Perspective(PerspectiveProjection {
                    fov: FOV.to_radians(),
                    ..default()
                }),
                ChildOf(player),
            ))
            .id();
        (app, player, view)
    }

    /// Walking keeps the view as it is; sprinting widens it, easing in
    /// over a few tenths of a second, and stopping eases it back.
    #[test]
    fn sprinting_widens_the_view() {
        let (mut app, _, view) = setup();
        hold(&mut app, &[KeyCode::KeyW]);
        run(&mut app, 1.0);
        assert!((fov(&app, view) - FOV).abs() < 1e-4, "{}", fov(&app, view));

        hold(&mut app, &[KeyCode::KeyW, KeyCode::KeyF]);
        run(&mut app, 0.05);
        let partway = fov(&app, view);
        assert!(FOV < partway && partway < FOV * 1.15, "{partway}");
        run(&mut app, 1.0);
        let wide = FOV * SPRINT_WIDENING as f32;
        assert!((fov(&app, view) - wide).abs() < 1e-2, "{}", fov(&app, view));

        hold(&mut app, &[]);
        run(&mut app, 1.0);
        assert!((fov(&app, view) - FOV).abs() < 1e-2, "{}", fov(&app, view));
    }

    /// Flying widens the view a little, and sprinting while flying more.
    #[test]
    fn flying_widens_the_view() {
        let (mut app, player, view) = setup();
        app.world_mut().get_mut::<Player>(player).unwrap().flying = true;
        run(&mut app, 1.0);
        let flying = FOV * FLYING_WIDENING as f32;
        assert!(
            (fov(&app, view) - flying).abs() < 1e-2,
            "{}",
            fov(&app, view)
        );

        hold(&mut app, &[KeyCode::KeyW, KeyCode::KeyF]);
        run(&mut app, 1.0);
        assert!(app.world().get::<Player>(player).unwrap().flying);
        let both = FOV * (FLYING_WIDENING * SPRINT_WIDENING) as f32;
        assert!((fov(&app, view) - both).abs() < 1e-2, "{}", fov(&app, view));
    }

    #[test]
    fn targets() {
        let mut player = Player::default();
        assert_eq!(target(&player), 1.0);
        player.sprinting = true;
        assert_eq!(target(&player), SPRINT_WIDENING);
        player.flying = true;
        assert_eq!(target(&player), SPRINT_WIDENING * FLYING_WIDENING);
    }
}
