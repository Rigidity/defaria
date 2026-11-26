//! Screens: UI that takes over the mouse and keyboard from the game, such
//! as the inventory.
//!
//! A screen is any UI root with the [`UiScreen`] component. The toolkit
//! doesn't know which screens exist: a feature opens its screen by spawning
//! its root with [`UiScreen`], and closes it by despawning the root.
//!
//! - While any screen is open the cursor is free and shown; when the last
//!   one closes it's locked again, back to playing.
//! - Escape closes every open screen.
//! - Gameplay input (looking, moving, breaking and placing) runs only while
//!   [`playing`]: no screen is open.

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

pub struct ScreenPlugin;

impl Plugin for ScreenPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<UiScreen>()
            .add_observer(free_cursor)
            .add_observer(lock_cursor)
            .add_systems(
                Update,
                close_on_escape
                    .run_if(not(playing))
                    .in_set(CloseOnEscapeSystems),
            );
    }
}

/// Where Escape closes the open screens. Gameplay's own use of Escape (the
/// fly camera freeing the cursor) runs before it, so it sees the screens
/// still open and leaves the cursor to them.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CloseOnEscapeSystems;

/// Marks the root of an open screen. Despawning the root closes it.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct UiScreen;

/// A run condition: no screen is open, so the game has the input. Also
/// true in apps without the UI at all, such as tests of a single system.
pub fn playing(screens: Query<(), With<UiScreen>>) -> bool {
    screens.is_empty()
}

fn close_on_escape(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    screens: Query<Entity, With<UiScreen>>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        for screen in &screens {
            // Another system may close it this frame too.
            commands.entity(screen).try_despawn();
        }
    }
}

fn free_cursor(
    _: On<Add<UiScreen>>,
    cursor: Option<Single<&mut CursorOptions, With<PrimaryWindow>>>,
) {
    if let Some(mut cursor) = cursor {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

/// Locks the cursor when the last screen closes. (The game still starts
/// with the cursor free, until the first click.)
fn lock_cursor(
    closed: On<Remove<UiScreen>>,
    screens: Query<Entity, With<UiScreen>>,
    cursor: Option<Single<&mut CursorOptions, With<PrimaryWindow>>>,
) {
    // The closing screen still has its component while this runs.
    let others = screens.iter().any(|screen| screen != closed.entity);
    if let Some(mut cursor) = cursor
        && !others
    {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, ScreenPlugin))
            .init_resource::<ButtonInput<KeyCode>>();
        app.world_mut()
            .spawn((Window::default(), CursorOptions::default(), PrimaryWindow));
        app.update();
        app
    }

    fn cursor(app: &mut App) -> CursorGrabMode {
        let mut cursors = app.world_mut().query::<&CursorOptions>();
        cursors.single(app.world()).unwrap().grab_mode
    }

    fn is_playing(app: &mut App) -> bool {
        app.world_mut().run_system_cached(playing).unwrap()
    }

    #[test]
    fn starting_leaves_the_cursor_free() {
        let mut app = app();
        assert_eq!(cursor(&mut app), CursorGrabMode::None);
        assert!(is_playing(&mut app));
    }

    #[test]
    fn opening_a_screen_frees_the_cursor_and_closing_the_last_locks_it() {
        let mut app = app();
        app.world_mut()
            .query::<&mut CursorOptions>()
            .single_mut(app.world_mut())
            .unwrap()
            .grab_mode = CursorGrabMode::Locked;
        let first = app.world_mut().spawn(UiScreen).id();
        assert_eq!(cursor(&mut app), CursorGrabMode::None);
        assert!(!is_playing(&mut app));
        let second = app.world_mut().spawn(UiScreen).id();

        // One still open: the cursor stays free.
        app.world_mut().despawn(first);
        assert_eq!(cursor(&mut app), CursorGrabMode::None);
        app.world_mut().despawn(second);
        assert_eq!(cursor(&mut app), CursorGrabMode::Locked);
        assert!(is_playing(&mut app));
    }

    #[test]
    fn escape_closes_every_screen() {
        let mut app = app();
        app.world_mut().spawn(UiScreen);
        app.world_mut().spawn(UiScreen);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        assert!(is_playing(&mut app));
        assert_eq!(cursor(&mut app), CursorGrabMode::Locked);
    }
}
