//! The main menu, in [`GameState::Menu`]: **Singleplayer** lists the saved
//! worlds in [`WorldsFolder`], the most recently played first, to play;
//! **New world** asks for a name and a seed (a number, or any text, or
//! nothing for a random one) and creates it; **Quit** quits.
//!
//! Playing a world puts its folder in [`WorldFolder`] and starts playing;
//! whoever runs the server (`defaria::integrated`) opens it from there.
//! Each page is a screen of its own ([`MenuPage`]), replaced by the next.

use bevy::{prelude::*, ui_widgets::Activate};
use defaria_core::save::{self, SavedWorld, WorldFolder};

use crate::{
    session::{GameState, WorldsFolder},
    ui::{TextField, UiScreen, UiTheme, button, text_field},
};

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<MenuPage>().add_systems(
            OnEnter(GameState::Menu),
            (spawn_menu_camera, open_main_menu),
        );
    }
}

/// A page of the menu: its root. Opening another despawns it.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
#[require(UiScreen)]
pub enum MenuPage {
    Main,
    Worlds,
    NewWorld,
}

/// The new world screen's name field.
#[derive(Component, Debug, Clone, Copy)]
pub struct NameField;

/// The new world screen's seed field.
#[derive(Component, Debug, Clone, Copy)]
pub struct SeedField;

/// A line on the new world screen saying what went wrong.
#[derive(Component, Debug, Clone, Copy)]
pub struct MenuError;

/// The world a world list entry plays.
#[derive(Component, Debug, Clone)]
pub struct WorldEntry(pub SavedWorld);

/// The menus draw through a camera of their own; playing, the player's
/// draws the UI.
fn spawn_menu_camera(mut commands: Commands) {
    commands.spawn((
        Name::new("Menu camera"),
        Camera2d,
        DespawnOnExit(GameState::Menu),
    ));
}

fn open_main_menu(
    mut commands: Commands,
    theme: Res<UiTheme>,
    pages: Query<Entity, With<MenuPage>>,
) {
    open_page(
        &mut commands,
        &theme,
        &pages,
        MenuPage::Main,
        |panel, theme| {
            panel
                .spawn((Name::new("Singleplayer"), button(theme, "Singleplayer")))
                .observe(show_worlds);
            panel
                .spawn((Name::new("Quit"), button(theme, "Quit")))
                .observe(|_: On<Activate>, mut exit: MessageWriter<AppExit>| {
                    exit.write(AppExit::Success);
                });
        },
    );
}

/// Despawns the page that's open, and opens `page`: a panel with its
/// title, filled by `fill`.
fn open_page(
    commands: &mut Commands,
    theme: &UiTheme,
    pages: &Query<Entity, With<MenuPage>>,
    page: MenuPage,
    fill: impl FnOnce(&mut ChildSpawnerCommands, &UiTheme),
) {
    for open in pages {
        commands.entity(open).try_despawn();
    }
    let title = match page {
        MenuPage::Main => "Defaria",
        MenuPage::Worlds => "Singleplayer",
        MenuPage::NewWorld => "New world",
    };
    commands
        .spawn((
            Name::new(format!("Menu: {title}")),
            page,
            DespawnOnExit(GameState::Menu),
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
                    min_width: px(260),
                    ..default()
                },
                BackgroundColor(theme.panel),
                BorderColor::all(theme.panel_border),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new(title),
                    theme.title_font(),
                    TextColor(theme.text),
                    Node {
                        align_self: AlignSelf::Center,
                        ..default()
                    },
                ));
                fill(panel, theme);
            });
        });
}

fn show_worlds(
    _: On<Activate>,
    mut commands: Commands,
    theme: Res<UiTheme>,
    folder: Option<Res<WorldsFolder>>,
    pages: Query<Entity, With<MenuPage>>,
) {
    let worlds = folder.map_or_else(Vec::new, |folder| save::list_worlds(&folder.0));
    open_page(
        &mut commands,
        &theme,
        &pages,
        MenuPage::Worlds,
        |panel, theme| {
            if worlds.is_empty() {
                panel.spawn((
                    Text::new("No worlds yet"),
                    theme.text_font(),
                    TextColor(theme.text_dim),
                ));
            }
            for world in worlds {
                panel
                    .spawn((
                        Name::new(format!("World: {}", world.name)),
                        WorldEntry(world.clone()),
                        button(theme, world.name.clone()),
                    ))
                    .observe(play_world);
            }
            panel
                .spawn((Name::new("New world"), button(theme, "New world")))
                .observe(show_new_world);
            panel
                .spawn((Name::new("Back"), button(theme, "Back")))
                .observe(back_to_main);
        },
    );
}

fn back_to_main(
    _: On<Activate>,
    commands: Commands,
    theme: Res<UiTheme>,
    pages: Query<Entity, With<MenuPage>>,
) {
    open_main_menu(commands, theme, pages);
}

fn show_new_world(
    _: On<Activate>,
    mut commands: Commands,
    theme: Res<UiTheme>,
    pages: Query<Entity, With<MenuPage>>,
) {
    open_page(
        &mut commands,
        &theme,
        &pages,
        MenuPage::NewWorld,
        |panel, theme| {
            let label = |text: &str| {
                (
                    Text::new(text),
                    theme.text_font(),
                    TextColor(theme.text_dim),
                )
            };
            panel.spawn(label("Name"));
            panel.spawn((
                Name::new("Name field"),
                NameField,
                text_field(theme, 240.0, "New World", 40),
            ));
            panel.spawn(label("Seed"));
            panel.spawn((
                Name::new("Seed field"),
                SeedField,
                text_field(theme, 240.0, "Random", 40),
            ));
            panel.spawn((
                MenuError,
                Text::default(),
                theme.text_font(),
                TextColor(theme.text_dim),
            ));
            panel
                .spawn((Name::new("Create"), button(theme, "Create")))
                .observe(create_world);
            panel
                .spawn((Name::new("Back"), button(theme, "Back")))
                .observe(show_worlds);
        },
    );
}

/// Creates the world the new world screen describes, and plays it.
fn create_world(
    _: On<Activate>,
    mut commands: Commands,
    folder: Option<Res<WorldsFolder>>,
    names: Query<&TextField, With<NameField>>,
    seeds: Query<&TextField, With<SeedField>>,
    mut errors: Query<&mut Text, With<MenuError>>,
    mut next: ResMut<NextState<GameState>>,
) {
    let name = names.iter().next().map_or("", |field| field.value.as_str());
    let seed = save::seed_from_text(seeds.iter().next().map_or("", |field| field.value.as_str()));
    let created = match folder {
        Some(folder) => save::create_world(&folder.0, name, seed),
        None => Err("there's nowhere to save worlds".into()),
    };
    match created {
        Ok(world) => {
            commands.insert_resource(WorldFolder(world));
            next.set(GameState::Playing);
        }
        Err(error) => {
            error!("{error}");
            for mut text in &mut errors {
                text.0 = format!("Can't create it: {error}");
            }
        }
    }
}

/// Plays the world a world list entry names.
fn play_world(
    activate: On<Activate>,
    mut commands: Commands,
    entries: Query<&WorldEntry>,
    mut next: ResMut<NextState<GameState>>,
) {
    if let Ok(WorldEntry(world)) = entries.get(activate.entity) {
        commands.insert_resource(WorldFolder(world.folder.clone()));
        next.set(GameState::Playing);
    }
}
