//! The mouse mining for the local player: holding left click while
//! playing with the cursor locked sets its [`Mining::holding`]. The click
//! that locks the cursor mines nothing, even if it's held.

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use crate::{
    crosshair::BlockOutlineSystems,
    fly_camera::FlyCameraSystems,
    ui::{UiScreen, playing},
};
use defaria_core::{
    mining::{Mining, MiningSystems},
    player::LocalPlayer,
};

pub struct MiningInputPlugin;

impl Plugin for MiningInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Armed>()
            // After the camera turns, so the block mined is the one under
            // the crosshair on the frame being drawn. Before the outline,
            // so it's drawn around the next block, not one that just broke.
            .configure_sets(
                Update,
                MiningSystems
                    .after(FlyCameraSystems)
                    .before(BlockOutlineSystems),
            )
            .add_systems(
                Update,
                (
                    // Before the fly camera, so the click that locks the
                    // cursor finds it still unlocked and arms nothing.
                    arm.before(FlyCameraSystems),
                    hold.after(FlyCameraSystems).before(MiningSystems),
                ),
            );
    }
}

/// Whether the held left button counts: it was pressed while playing with
/// the cursor locked.
#[derive(Resource, Debug, Default)]
struct Armed(bool);

/// Whether the player can mine now: the cursor is locked and no screen
/// is open.
fn can_mine(cursor: &CursorOptions, screens: Query<(), With<UiScreen>>) -> bool {
    cursor.grab_mode != CursorGrabMode::None && playing(screens)
}

/// Arms mining on a left press made while the player can mine, before the
/// fly camera can lock the cursor for it.
fn arm(
    mouse: Res<ButtonInput<MouseButton>>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    screens: Query<(), With<UiScreen>>,
    mut armed: ResMut<Armed>,
) {
    if mouse.just_pressed(MouseButton::Left) {
        armed.0 = can_mine(&cursor, screens);
    }
}

/// The local player holds the button while the armed left button is held
/// and it can still mine. Once it doesn't, the button stays disarmed until
/// pressed again.
fn hold(
    mouse: Res<ButtonInput<MouseButton>>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    screens: Query<(), With<UiScreen>>,
    mut armed: ResMut<Armed>,
    mut mining: Single<&mut Mining, With<LocalPlayer>>,
) {
    let holding = armed.0 && mouse.pressed(MouseButton::Left) && can_mine(&cursor, screens);
    if !holding {
        armed.0 = false;
    }
    if mining.holding != holding {
        mining.holding = holding;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use defaria_core::{
        content::{base, block},
        player::tests::local_player_at,
        testing::{chunk_app, insert_chunk},
        world::{Chunk, Voxels},
    };
    use defaria_server::mining::MiningPlugin;

    /// Stone at (5, 1, 5), a locked cursor, and the local player looking
    /// at the stone from three blocks away.
    fn app() -> App {
        let mut app = chunk_app(base().clone(), Duration::from_millis(50));
        app.add_plugins((MiningPlugin, MiningInputPlugin))
            .init_resource::<ButtonInput<MouseButton>>();
        app.world_mut().spawn((
            Window::default(),
            CursorOptions {
                grab_mode: CursorGrabMode::Locked,
                ..default()
            },
            PrimaryWindow,
        ));
        let mut chunk = Chunk::default();
        chunk.set(5, 1, 5, block("stone"));
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        app.world_mut().spawn(local_player_at(
            Transform::from_xyz(2.5, 1.5, 5.5).looking_to(Vec3::X, Vec3::Y),
        ));
        app.update();
        app
    }

    /// Runs a frame, with the left button just pressed on it if `press`.
    fn frame(app: &mut App, press: bool) {
        let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        if press {
            mouse.press(MouseButton::Left);
        }
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
    }

    fn mining(app: &mut App) -> Mining {
        let mut miners = app.world_mut().query::<&Mining>();
        *miners.single(app.world()).unwrap()
    }

    fn stone(app: &mut App) -> bool {
        app.world_mut()
            .run_system_once(|voxels: Voxels| voxels.get(IVec3::new(5, 1, 5)))
            .unwrap()
            == Some(block("stone"))
    }

    fn set_cursor(app: &mut App, grab_mode: CursorGrabMode) {
        let mut cursors = app.world_mut().query::<&mut CursorOptions>();
        cursors.single_mut(app.world_mut()).unwrap().grab_mode = grab_mode;
    }

    /// Holding the button mines; releasing it stops.
    #[test]
    fn holding_left_click_mines() {
        let mut app = app();
        frame(&mut app, true);
        frame(&mut app, false);
        assert!(mining(&mut app).holding);
        assert!(mining(&mut app).progress > 0.0);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Left);
        frame(&mut app, false);
        assert_eq!(mining(&mut app), Mining::default());
    }

    /// A press made while the cursor is free (the click that locks it)
    /// mines nothing, however long it's held.
    #[test]
    fn the_locking_click_mines_nothing() {
        let mut app = app();
        set_cursor(&mut app, CursorGrabMode::None);
        frame(&mut app, true);
        set_cursor(&mut app, CursorGrabMode::Locked);
        for _ in 0..60 {
            frame(&mut app, false);
        }
        assert_eq!(mining(&mut app), Mining::default());
        assert!(stone(&mut app));
    }

    /// Opening a screen mid-mine starts over, and closing it doesn't
    /// start again until the button is pressed again.
    #[test]
    fn opening_a_screen_starts_over() {
        let mut app = app();
        frame(&mut app, true);
        frame(&mut app, false);
        assert!(mining(&mut app).progress > 0.0);
        let screen = app.world_mut().spawn(UiScreen).id();
        frame(&mut app, false);
        assert_eq!(mining(&mut app), Mining::default());
        app.world_mut().despawn(screen);
        frame(&mut app, false);
        assert!(!mining(&mut app).holding);
    }
}
