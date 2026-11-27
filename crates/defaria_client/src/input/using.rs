//! Right click as the local player's [`UseAction`] (see `use_action`).
//! Only while playing, with the cursor locked: the click that locks it
//! uses nothing.

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use crate::{fly_camera::FlyCameraSystems, hotbar_ui::HotbarInput, ui::playing};
use defaria_core::{
    block_edit::crosshair_target,
    content::Content,
    player::{Eyes, LocalPlayer},
    use_action::{Use, UseAction, UseStep, UseSystems},
    world::Voxels,
};

pub struct UseInputPlugin;

impl Plugin for UseInputPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            Update,
            // Before the fly camera, so the click that locks the cursor
            // finds it still unlocked. Without sync points, so a screen a
            // step opens applies at the end of the frame, as one a key
            // opens does (the fly camera's own chain applies it before it
            // turns the view). After the hotbar, so a slot picked this frame
            // is the one used.
            UseSystems
                .after(HotbarInput)
                .before_ignore_deferred(FlyCameraSystems),
        )
        .add_systems(Update, read_use.run_if(playing).in_set(UseStep::Input));
    }
}

/// Right click as the local player's use this frame, and no use without
/// one.
fn read_use(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    // Optional, so a frame with no window still clears the last use.
    cursor: Option<Single<&CursorOptions, With<PrimaryWindow>>>,
    content: Res<Content>,
    voxels: Voxels,
    mut players: Query<(Eyes, &mut UseAction), With<LocalPlayer>>,
) {
    let locked = cursor.is_some_and(|cursor| cursor.grab_mode != CursorGrabMode::None);
    let clicked = locked && mouse.just_pressed(MouseButton::Right);
    for (eyes, mut action) in &mut players {
        let using = clicked.then(|| {
            let eyes = eyes.transform();
            Use {
                eyes,
                target: crosshair_target(&eyes, &content, &voxels),
                sneak: keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]),
            }
        });
        action.set_if_neq(UseAction(using));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use defaria_core::use_action::UseActionPlugin;
    use defaria_core::{
        content::{base, block},
        player::tests::local_player_at,
        testing::{chunk_app, insert_chunk},
        world::Chunk,
    };

    /// What each step saw, in the order they ran.
    #[derive(Resource, Default)]
    struct Seen(Vec<(UseStep, Option<Use>)>);

    /// A step that records the use it saw, and claims it if it's `claims`.
    fn step(at: UseStep, claims: bool) -> impl FnMut(ResMut<Seen>, Query<&mut UseAction>) {
        move |mut seen, mut actions| {
            for mut action in &mut actions {
                seen.0.push((at, action.pending()));
                if claims && action.pending().is_some() {
                    action.claim();
                }
            }
        }
    }

    const STEPS: [UseStep; 6] = [
        UseStep::Wrench,
        UseStep::Container,
        UseStep::Block,
        UseStep::Screen,
        UseStep::Item,
        UseStep::Place,
    ];

    /// A locked cursor, a stone block in front of the local player and a
    /// recording step in each [`UseStep`], `claimer` claiming.
    fn app(claimer: Option<UseStep>) -> App {
        let mut app = chunk_app(base().clone(), Duration::from_millis(16));
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<Seen>()
            .add_plugins((UseActionPlugin, UseInputPlugin));
        for at in STEPS {
            app.add_systems(Update, step(at, claimer == Some(at)).in_set(at));
        }
        app.world_mut().spawn((
            Window::default(),
            CursorOptions {
                grab_mode: CursorGrabMode::Locked,
                ..default()
            },
            PrimaryWindow,
        ));
        let mut chunk = Chunk::default();
        chunk.set(8, 4, 5, block("stone"));
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        app.world_mut().spawn(local_player_at(
            Transform::from_xyz(8.5, 4.5, 8.5).looking_to(Vec3::NEG_Z, Vec3::Y),
        ));
        app
    }

    /// Steps a frame with right click pressed (and Shift held, if `shift`),
    /// or with nothing if not `click`, and returns what the steps saw.
    fn frame(app: &mut App, click: bool, shift: bool) -> Vec<(UseStep, Option<Use>)> {
        if shift {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::ShiftLeft);
        }
        if click {
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .press(MouseButton::Right);
        }
        app.update();
        let mut mouse = app.world_mut().resource_mut::<ButtonInput<MouseButton>>();
        mouse.release_all();
        mouse.clear();
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release_all();
        keys.clear();
        std::mem::take(&mut app.world_mut().resource_mut::<Seen>().0)
    }

    /// The steps run in order of precedence, each with the use, until one
    /// claims it; none after it runs.
    #[test]
    fn steps_see_the_use_until_one_claims_it() {
        for claimer in STEPS {
            let mut app = app(Some(claimer));
            let seen: Vec<_> = frame(&mut app, true, false)
                .into_iter()
                .map(|(at, using)| (at, using.is_some()))
                .collect();
            let claimed = STEPS.iter().position(|&s| s == claimer).unwrap();
            let expected: Vec<_> = STEPS[..=claimed].iter().map(|&s| (s, true)).collect();
            assert_eq!(seen, expected, "{claimer:?} claiming");
        }
    }

    /// The use has the crosshair's block and Shift, and lasts only the
    /// frame of the click.
    #[test]
    fn a_use_has_the_target_and_shift() {
        let mut app = app(None);
        let using = frame(&mut app, true, false)[5].1.unwrap();
        let target = using.target.unwrap();
        assert_eq!(
            (target.block, target.normal, target.shape_box),
            (IVec3::new(8, 4, 5), IVec3::Z, 0)
        );
        // On the face it entered, the south one.
        assert!((target.at.z - 1.0).abs() < 1e-4, "{}", target.at);
        assert!(!using.sneak);
        assert!(frame(&mut app, true, true)[5].1.unwrap().sneak);
        assert!(frame(&mut app, false, false).is_empty(), "no steps run");
    }

    /// The click that locks the cursor uses nothing.
    #[test]
    fn no_use_while_the_cursor_is_free() {
        let mut app = app(None);
        app.world_mut()
            .query::<&mut CursorOptions>()
            .single_mut(app.world_mut())
            .unwrap()
            .grab_mode = CursorGrabMode::None;
        assert!(frame(&mut app, true, false).is_empty());
    }
}
