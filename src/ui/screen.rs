//! Screens: UI that takes over the mouse and keyboard from the game, such
//! as the inventory. At most one is open, as the [`UiScreen`] state.
//!
//! - Opening a screen frees and shows the cursor; closing it locks the
//!   cursor again, back to playing.
//! - Escape closes whichever screen is open.
//! - Gameplay input (looking, moving, breaking and placing) runs only while
//!   [`playing`].
//!
//! A screen spawns its UI on `OnEnter` with `DespawnOnExit`, so closing it
//! cleans up after it.

use bevy::{
    prelude::*,
    state::state::StateTransitionEvent,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

pub struct ScreenPlugin;

impl Plugin for ScreenPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<UiScreen>()
            .register_type::<UiScreen>()
            .add_systems(
                Update,
                (
                    close_on_escape.run_if(not(playing)),
                    grab_cursor_for_screens,
                ),
            );
    }
}

/// The screen that's open, if any.
#[derive(States, Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UiScreen {
    /// No screen: playing.
    #[default]
    Closed,
    Inventory,
}

/// A run condition: no screen is open, so the game has the input. True in
/// apps without screens at all, such as tests of a single system.
pub fn playing(screen: Option<Res<State<UiScreen>>>) -> bool {
    screen.is_none_or(|screen| *screen.get() == UiScreen::Closed)
}

fn close_on_escape(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<UiScreen>>) {
    if keys.just_pressed(KeyCode::Escape) {
        next.set(UiScreen::Closed);
    }
}

/// Frees the cursor when a screen opens and locks it when the last one
/// closes. Not on entering `Closed` at startup: the game starts with the
/// cursor free until the first click.
fn grab_cursor_for_screens(
    mut transitions: MessageReader<StateTransitionEvent<UiScreen>>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
) {
    for transition in transitions.read() {
        match (transition.exited, transition.entered) {
            (Some(UiScreen::Closed), Some(screen)) if screen != UiScreen::Closed => {
                cursor.grab_mode = CursorGrabMode::None;
                cursor.visible = true;
            }
            (Some(screen), Some(UiScreen::Closed)) if screen != UiScreen::Closed => {
                cursor.grab_mode = CursorGrabMode::Locked;
                cursor.visible = false;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::state::app::StatesPlugin;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin, ScreenPlugin))
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

    fn set_screen(app: &mut App, screen: UiScreen) {
        app.world_mut()
            .resource_mut::<NextState<UiScreen>>()
            .set(screen);
        app.update();
        app.update();
    }

    #[test]
    fn starting_leaves_the_cursor_free() {
        let mut app = app();
        assert_eq!(cursor(&mut app), CursorGrabMode::None);
        assert_eq!(
            *app.world().resource::<State<UiScreen>>().get(),
            UiScreen::Closed
        );
    }

    #[test]
    fn opening_a_screen_frees_the_cursor_and_closing_locks_it() {
        let mut app = app();
        app.world_mut()
            .query::<&mut CursorOptions>()
            .single_mut(app.world_mut())
            .unwrap()
            .grab_mode = CursorGrabMode::Locked;
        set_screen(&mut app, UiScreen::Inventory);
        assert_eq!(cursor(&mut app), CursorGrabMode::None);
        set_screen(&mut app, UiScreen::Closed);
        assert_eq!(cursor(&mut app), CursorGrabMode::Locked);
    }

    #[test]
    fn escape_closes_the_screen() {
        let mut app = app();
        set_screen(&mut app, UiScreen::Inventory);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        app.update();
        assert_eq!(
            *app.world().resource::<State<UiScreen>>().get(),
            UiScreen::Closed
        );
    }
}
