//! Keys kept held through a brief loss of the window's focus.
//!
//! When the window loses keyboard focus, Bevy lets go of every key, and
//! when it gets focus back it can't tell which are still held: the OS
//! sends no key-down for a key that went down before. So something that
//! takes focus for a moment (a notification, another app activating
//! itself) stopped the player until its keys were pressed again. If focus
//! comes back within [`FOCUS_GRACE`], the keys held when it went are held
//! again, unless they went up or down since. A key let go of in that
//! moment stays held until it's pressed and let go again; see
//! "Keys come back after a brief loss of focus" in `docs/decisions.md`.

use std::time::Duration;

use bevy::{
    input::{
        ButtonState, InputSystems,
        keyboard::{KeyboardFocusLost, KeyboardInput},
    },
    platform::collections::HashSet,
    prelude::*,
    window::WindowFocused,
};

/// How long focus may be gone for the keys held when it went to be held
/// again when it's back.
pub const FOCUS_GRACE: Duration = Duration::from_millis(500);

pub struct FocusInputPlugin;

impl Plugin for FocusInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HeldThroughFocus>().add_systems(
            PreUpdate,
            keep_keys_through_focus
                .after(InputSystems)
                .run_if(resource_exists::<Messages<WindowFocused>>)
                .run_if(resource_exists::<Messages<KeyboardFocusLost>>)
                .run_if(resource_exists::<Messages<KeyboardInput>>),
        );
    }
}

/// The keys held at the end of the last frame, and those held when focus
/// went, while it's gone.
#[derive(Resource, Debug, Default)]
struct HeldThroughFocus {
    held: HashSet<KeyCode>,
    lost: Option<Lost>,
}

#[derive(Debug)]
struct Lost {
    /// When focus went, in real time.
    at: Duration,
    /// The keys held then that haven't gone up or down since.
    keys: HashSet<KeyCode>,
}

/// Notes the keys held when focus goes, and holds them again if it's back
/// in time. They're held without being just pressed, so nothing that acts
/// on a press (the inventory, switching the game mode) does.
fn keep_keys_through_focus(
    time: Res<Time<Real>>,
    mut state: ResMut<HeldThroughFocus>,
    mut focus: MessageReader<WindowFocused>,
    mut focus_lost: MessageReader<KeyboardFocusLost>,
    mut inputs: MessageReader<KeyboardInput>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
) {
    let state = &mut *state;
    let now = time.elapsed();
    // Bevy's window backend lets go of the held keys at the end of the
    // frame focus goes, so the next frame's key events start with one up
    // for each, before any real ones (focus may be back by then). The first
    // up of each key held is Bevy's; anything after is real.
    let mut made_up = HashSet::new();
    if focus_lost.read().count() > 0 {
        made_up.clone_from(&state.held);
        state.lost = Some(Lost {
            at: now,
            keys: state.held.clone(),
        });
    }
    for input in inputs.read() {
        if input.state == ButtonState::Released && made_up.remove(&input.key_code) {
            continue;
        }
        // A key that went up or down since is where its events left it.
        if let Some(lost) = &mut state.lost {
            lost.keys.remove(&input.key_code);
        }
    }
    if focus.read().any(|event| event.focused)
        && let Some(lost) = state.lost.take()
        && now - lost.at <= FOCUS_GRACE
    {
        for key in lost.keys {
            if !keys.pressed(key) {
                keys.press(key);
                keys.clear_just_pressed(key);
            }
        }
    }
    state.held.clear();
    state.held.extend(keys.get_pressed().copied());
}

#[cfg(test)]
mod tests {
    use bevy::{
        input::{InputPlugin, keyboard::Key},
        time::TimeUpdateStrategy,
    };

    use super::*;

    const FRAME: Duration = Duration::from_millis(16);

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, InputPlugin, FocusInputPlugin))
            .add_message::<WindowFocused>()
            .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME));
        app.update();
        app
    }

    fn key(app: &mut App, key_code: KeyCode, state: ButtonState) {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: Key::Character("w".into()),
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    fn focused(app: &mut App, focused: bool) {
        app.world_mut().write_message(WindowFocused {
            window: Entity::PLACEHOLDER,
            focused,
        });
    }

    /// Focus going, as Bevy's window backend says it: focus lost, then the
    /// next frame every held key let go and the keyboard's focus lost.
    fn lose_focus(app: &mut App, held: &[KeyCode]) {
        focused(app, false);
        app.update();
        for &held in held {
            key(app, held, ButtonState::Released);
        }
        app.world_mut().write_message(KeyboardFocusLost);
        app.update();
    }

    fn frames(app: &mut App, count: usize) {
        for _ in 0..count {
            app.update();
        }
    }

    fn keys(app: &App) -> &ButtonInput<KeyCode> {
        app.world().resource::<ButtonInput<KeyCode>>()
    }

    /// A moment without focus: the keys held are held again, but not
    /// pressed again.
    #[test]
    fn keys_held_through_a_blip_are_held_again() {
        let mut app = app();
        key(&mut app, KeyCode::KeyW, ButtonState::Pressed);
        key(&mut app, KeyCode::KeyE, ButtonState::Pressed);
        frames(&mut app, 3);
        lose_focus(&mut app, &[KeyCode::KeyW, KeyCode::KeyE]);
        assert!(!keys(&app).pressed(KeyCode::KeyW));
        frames(&mut app, 1);
        focused(&mut app, true);
        app.update();
        assert!(keys(&app).pressed(KeyCode::KeyW));
        assert!(keys(&app).pressed(KeyCode::KeyE));
        assert!(!keys(&app).just_pressed(KeyCode::KeyE));
        // Let go of after, it's let go of.
        key(&mut app, KeyCode::KeyW, ButtonState::Released);
        app.update();
        assert!(!keys(&app).pressed(KeyCode::KeyW));
    }

    /// Focus gone longer than the grace (switching apps): nothing comes
    /// back.
    #[test]
    fn keys_stay_up_after_a_long_loss() {
        let mut app = app();
        key(&mut app, KeyCode::KeyW, ButtonState::Pressed);
        frames(&mut app, 2);
        lose_focus(&mut app, &[KeyCode::KeyW]);
        frames(
            &mut app,
            (FOCUS_GRACE.as_millis() / FRAME.as_millis()) as usize + 2,
        );
        focused(&mut app, true);
        app.update();
        assert!(!keys(&app).pressed(KeyCode::KeyW));
    }

    /// A key that goes up and down again while focus is back but before
    /// the window says so isn't changed: its own events say where it is.
    #[test]
    fn a_key_seen_since_is_left_alone() {
        let mut app = app();
        key(&mut app, KeyCode::KeyW, ButtonState::Pressed);
        key(&mut app, KeyCode::KeyA, ButtonState::Pressed);
        frames(&mut app, 2);
        lose_focus(&mut app, &[KeyCode::KeyW, KeyCode::KeyA]);
        key(&mut app, KeyCode::KeyA, ButtonState::Pressed);
        app.update();
        key(&mut app, KeyCode::KeyA, ButtonState::Released);
        focused(&mut app, true);
        app.update();
        assert!(keys(&app).pressed(KeyCode::KeyW));
        assert!(!keys(&app).pressed(KeyCode::KeyA));
    }

    /// Focus back by the frame Bevy lets go of the keys (the shortest
    /// blip that loses them): they're held again.
    #[test]
    fn keys_come_back_when_focus_returns_with_their_release() {
        let mut app = app();
        key(&mut app, KeyCode::KeyW, ButtonState::Pressed);
        frames(&mut app, 2);
        focused(&mut app, false);
        app.update();
        key(&mut app, KeyCode::KeyW, ButtonState::Released);
        app.world_mut().write_message(KeyboardFocusLost);
        focused(&mut app, true);
        app.update();
        assert!(keys(&app).pressed(KeyCode::KeyW));
    }

    /// A key really let go of as focus comes back, in the frame Bevy lets
    /// go of it too, stays up: only the first up is Bevy's.
    #[test]
    fn a_real_release_beside_bevys_is_kept() {
        let mut app = app();
        key(&mut app, KeyCode::KeyW, ButtonState::Pressed);
        key(&mut app, KeyCode::KeyA, ButtonState::Pressed);
        frames(&mut app, 2);
        focused(&mut app, false);
        app.update();
        key(&mut app, KeyCode::KeyW, ButtonState::Released);
        key(&mut app, KeyCode::KeyA, ButtonState::Released);
        app.world_mut().write_message(KeyboardFocusLost);
        focused(&mut app, true);
        key(&mut app, KeyCode::KeyW, ButtonState::Released);
        app.update();
        assert!(!keys(&app).pressed(KeyCode::KeyW));
        assert!(keys(&app).pressed(KeyCode::KeyA));
    }

    /// Keys not held when focus went aren't held after.
    #[test]
    fn only_the_keys_held_come_back() {
        let mut app = app();
        key(&mut app, KeyCode::KeyW, ButtonState::Pressed);
        app.update();
        key(&mut app, KeyCode::KeyW, ButtonState::Released);
        app.update();
        lose_focus(&mut app, &[]);
        focused(&mut app, true);
        app.update();
        assert!(keys(&app).get_pressed().next().is_none());
    }
}
