//! The main menu, the world list, new worlds and the pause menu, in the
//! game.

use bevy::{ecs::system::RunSystemOnce, prelude::*, ui::UiGlobalTransform, window::CursorOptions};
use defaria_client::{
    menu::{MenuPage, NameField, SeedField},
    pause::PauseMenu,
    session::{GameState, WorldsFolder},
};
use defaria_core::{
    content::block,
    player::LocalPlayer,
    save::{WorldFolder, tests::temp_folder},
    world::{ChunkMap, Voxels},
};

use crate::headless::{Script, run_more, settle, test_game};

/// A game whose world list is in a folder of its own, playing.
fn game(name: &str) -> (App, std::path::PathBuf) {
    let worlds = temp_folder(name);
    let mut app = test_game(Script::parse("wait 1").unwrap());
    app.insert_resource(WorldsFolder(worlds.clone()));
    settle(&mut app);
    // Lock the cursor, playing.
    run_more(&mut app, "hold MouseLeft 1\nwait 1");
    (app, worlds)
}

/// Where the UI node named `name` is on screen.
fn at(app: &mut App, name: &str) -> Vec2 {
    let mut nodes = app.world_mut().query::<(&Name, &UiGlobalTransform)>();
    nodes
        .iter(app.world())
        .find(|(node, _)| node.as_str() == name)
        .unwrap_or_else(|| panic!("nothing named {name}"))
        .1
        .translation
}

/// Clicks the button named `name`, a moment after its page opened (menu
/// buttons don't take clicks before: see `menu::MenuClick`).
fn click(app: &mut App, name: &str) {
    let at = at(app, name);
    run_more(
        app,
        &format!(
            "cursor {} {}\nwait 30\nhold MouseLeft 1\nwait 3",
            at.x, at.y
        ),
    );
}

/// Clicks a text field and types `text` into it.
fn type_into<F: Component>(app: &mut App, text: &str) {
    let mut fields = app
        .world_mut()
        .query_filtered::<&UiGlobalTransform, With<F>>();
    let at = fields.single(app.world()).unwrap().translation;
    run_more(
        app,
        &format!(
            "cursor {} {}\nwait 1\nhold MouseLeft 1\nwait 1\ntype {text}\nwait 2",
            at.x, at.y
        ),
    );
}

fn state(app: &App) -> GameState {
    *app.world().resource::<State<GameState>>().get()
}

fn count<T: Component>(app: &mut App) -> usize {
    let mut query = app.world_mut().query_filtered::<(), With<T>>();
    query.iter(app.world()).count()
}

fn page(app: &mut App) -> Option<MenuPage> {
    let mut pages = app.world_mut().query::<&MenuPage>();
    pages.iter(app.world()).next().copied()
}

/// Escape opens the pause menu, which frees the cursor and pauses the
/// game (the server's time and the client's); Escape again closes it.
#[test]
fn escape_pauses_and_resumes() {
    let (mut app, _) = game("menus-pause");
    run_more(&mut app, "hold Escape 1\nwait 3");
    assert_eq!(count::<PauseMenu>(&mut app), 1);
    let mut cursors = app.world_mut().query::<&CursorOptions>();
    let cursor = cursors.single(app.world()).unwrap().grab_mode;
    assert_eq!(cursor, bevy::window::CursorGrabMode::None);
    assert!(app.world().resource::<Time<Virtual>>().is_paused());
    let server_paused = crate::headless::server_world(&app)
        .resource::<Time<Virtual>>()
        .is_paused();
    assert!(server_paused);

    run_more(&mut app, "hold Escape 1\nwait 3");
    assert_eq!(count::<PauseMenu>(&mut app), 0);
    assert!(!app.world().resource::<Time<Virtual>>().is_paused());
}

/// **Main menu** leaves the world for the main menu: the client keeps
/// nothing of it, and its server goes. A new world, named and seeded on
/// its screen, is created in the world list's folder and played; leaving
/// it saves it, and playing it again from the list finds it as it was.
#[test]
fn worlds_are_created_left_and_played_again() {
    let (mut app, worlds) = game("menus-worlds");
    run_more(&mut app, "hold Escape 1\nwait 2");
    click(&mut app, "Main menu");
    assert_eq!(state(&app), GameState::Menu);
    assert_eq!(page(&mut app), Some(MenuPage::Main));
    assert_eq!(count::<LocalPlayer>(&mut app), 0);
    assert_eq!(app.world().resource::<ChunkMap>().len(), 0);
    assert!(
        app.world()
            .get_non_send::<crate::IntegratedServer>()
            .is_none()
    );

    click(&mut app, "Singleplayer");
    assert_eq!(page(&mut app), Some(MenuPage::Worlds));
    click(&mut app, "New world");
    type_into::<NameField>(&mut app, "Alps");
    type_into::<SeedField>(&mut app, "42");
    click(&mut app, "Create");
    assert_eq!(state(&app), GameState::Playing);
    let folder = worlds.join("Alps");
    assert_eq!(
        app.world().resource::<WorldFolder>(),
        &WorldFolder(folder.clone())
    );
    let info = std::fs::read_to_string(folder.join("world.json5")).unwrap();
    assert!(info.contains("seed: 42"), "{info}");

    // Build something, and leave.
    settle(&mut app);
    run_more(&mut app, "fill 64 70 64 64 70 64 glass\nwait 2");
    run_more(&mut app, "hold MouseLeft 1\nwait 1\nhold Escape 1\nwait 2");
    click(&mut app, "Main menu");
    assert_eq!(state(&app), GameState::Menu);

    // Back in from the list: it's still there.
    click(&mut app, "Singleplayer");
    click(&mut app, "World: Alps");
    assert_eq!(state(&app), GameState::Playing);
    settle(&mut app);
    let there = app
        .world_mut()
        .run_system_once(|voxels: Voxels| voxels.get(IVec3::new(64, 70, 64)))
        .unwrap();
    assert_eq!(there, Some(block("glass")));
}

/// Escape on a menu page leaves it open (its **Back** goes back), and a
/// long world list scrolls rather than pushing its buttons off screen.
/// (A second click right after **Main menu** lands on **Quit**, which
/// ignores clicks for its first moments; a run can't tell that exit from
/// its script's own end, so that's only exercised here.)
#[test]
fn the_menus_stay_usable() {
    let (mut app, worlds) = game("menus-usable");
    for n in 0..15 {
        defaria_core::save::create_world(&worlds, &format!("World {n}"), n).unwrap();
    }
    run_more(&mut app, "hold Escape 1\nwait 2");
    let main_menu = at(&mut app, "Main menu");
    // Twice, quickly: the second lands on Quit.
    run_more(
        &mut app,
        &format!(
            "cursor {} {}\nwait 1\nhold MouseLeft 1\nwait 2\nhold MouseLeft 1\nwait 3",
            main_menu.x, main_menu.y
        ),
    );
    assert_eq!(page(&mut app), Some(MenuPage::Main));
    // Singleplayer twice, quickly: the second lands on a world, which
    // doesn't take it.
    let singleplayer = at(&mut app, "Singleplayer");
    run_more(
        &mut app,
        &format!(
            "cursor {} {}\nwait 30\nhold MouseLeft 1\nwait 2\nhold MouseLeft 1\nwait 3",
            singleplayer.x, singleplayer.y
        ),
    );
    assert_eq!(state(&app), GameState::Menu);
    assert_eq!(page(&mut app), Some(MenuPage::Worlds));
    click(&mut app, "Back");
    // The right button does nothing.
    let singleplayer = at(&mut app, "Singleplayer");
    run_more(
        &mut app,
        &format!(
            "cursor {} {}\nwait 30\nhold MouseRight 1\nwait 3",
            singleplayer.x, singleplayer.y
        ),
    );
    assert_eq!(page(&mut app), Some(MenuPage::Main));

    run_more(&mut app, "hold Escape 1\nwait 3");
    assert_eq!(page(&mut app), Some(MenuPage::Main));

    click(&mut app, "Singleplayer");
    run_more(&mut app, "hold Escape 1\nwait 3");
    assert_eq!(page(&mut app), Some(MenuPage::Worlds));
    let back = at(&mut app, "Back");
    assert!(back.y < 720.0, "Back is off screen, at {back}");
    click(&mut app, "Back");
    assert_eq!(page(&mut app), Some(MenuPage::Main));
}
