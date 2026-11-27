//! The pause menu: Escape while playing, with no screen open, opens it
//! (a screen, so the cursor's free); Escape again or **Resume** closes it,
//! and **Main menu** leaves the world (the server saves it) for the main
//! menu.
//!
//! While it's open the player is [`Paused`], which the server is told: a
//! server in the game's own process pauses the game while every player
//! is (see `defaria_server::server::PauseWithPlayers`).

use bevy::{
    picking::{events::PointerClick, pointer::PointerButton},
    prelude::*,
};
use defaria_core::net::ClientMessage;

use crate::{
    net::ToServer,
    session::GameState,
    ui::{UiScreen, UiTheme, button, playing, screen::CloseOnEscapeSystems, typing},
};

pub struct PausePlugin;

impl Plugin for PausePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Paused>()
            .register_type::<PauseMenu>()
            .add_systems(
                Update,
                (
                    // Before Escape closes screens, so the Escape that
                    // closes one doesn't open this too; and without a sync
                    // point between, so the same Escape doesn't close it.
                    open_pause_menu
                        .run_if(in_state(GameState::Playing))
                        .run_if(playing)
                        .run_if(not(typing))
                        .before_ignore_deferred(CloseOnEscapeSystems),
                    note_paused.after(CloseOnEscapeSystems),
                ),
            )
            .add_systems(OnExit(GameState::Playing), stop_pausing);
    }
}

/// The pause menu's root.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
#[require(UiScreen)]
pub struct PauseMenu;

/// Whether the player is paused: the pause menu is open.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Paused(pub bool);

fn open_pause_menu(keys: Res<ButtonInput<KeyCode>>, mut commands: Commands, theme: Res<UiTheme>) {
    if keys.just_pressed(KeyCode::Escape) {
        spawn_pause_menu(&mut commands, &theme);
    }
}

/// Spawns the pause menu.
pub fn spawn_pause_menu(commands: &mut Commands, theme: &UiTheme) {
    commands
        .spawn((
            Name::new("Pause menu"),
            PauseMenu,
            DespawnOnExit(GameState::Playing),
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(theme.backdrop),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Stretch,
                    padding: UiRect::all(px(theme.panel_padding)),
                    border: UiRect::all(px(1)),
                    row_gap: px(10),
                    min_width: px(220),
                    ..default()
                },
                BackgroundColor(theme.panel),
                BorderColor::all(theme.panel_border),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("Paused"),
                    theme.title_font(),
                    TextColor(theme.text),
                    Node {
                        align_self: AlignSelf::Center,
                        ..default()
                    },
                ));
                panel
                    .spawn((Name::new("Resume"), button(theme, "Resume")))
                    .observe(resume);
                panel
                    .spawn((Name::new("Main menu"), button(theme, "Main menu")))
                    .observe(leave);
            });
        });
}

fn resume(click: On<PointerClick>, mut commands: Commands, menus: Query<Entity, With<PauseMenu>>) {
    // The left button only (Bevy's buttons take any).
    if click.button != PointerButton::Primary {
        return;
    }
    for menu in &menus {
        commands.entity(menu).try_despawn();
    }
}

fn leave(click: On<PointerClick>, mut next: ResMut<NextState<GameState>>) {
    if click.button == PointerButton::Primary {
        next.set(GameState::Menu);
    }
}

/// The player's paused while the pause menu's open; the server is told
/// when that changes.
fn note_paused(
    menus: Query<(), With<PauseMenu>>,
    mut paused: ResMut<Paused>,
    mut to_server: MessageWriter<ToServer>,
) {
    let now = Paused(!menus.is_empty());
    if *paused != now {
        *paused = now;
        to_server.write(ToServer(ClientMessage::Paused(now.0)));
    }
}

fn stop_pausing(mut paused: ResMut<Paused>) {
    *paused = Paused(false);
}
