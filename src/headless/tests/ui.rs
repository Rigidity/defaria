//! UI widgets in the game: text fields and tooltips.

use bevy::{prelude::*, ui::UiGlobalTransform, ui_widgets::ValueChange};
use defaria_client::ui::{
    TextField, Tooltip, UiScreen, UiTheme,
    text_field::{Placeholder, text_field, typing},
    tooltip::TooltipBox,
};
use defaria_core::hotbar::ActiveSlot;

use crate::headless::{Script, run_more, settle, test_game};

#[derive(Resource, Default)]
struct Changes(Vec<String>);

/// Through the real game and input: clicking a field gives it the
/// keyboard; typing, Backspace and the arrows edit it, each change is
/// reported; and the number keys type rather than pick a hotbar slot.
#[test]
fn typing_edits_the_field_and_keeps_keys_from_the_game() {
    let mut app = test_game(Script::parse("wait 1").unwrap());
    settle(&mut app);
    let theme = app.world().resource::<UiTheme>().clone();
    let field = app
        .world_mut()
        .spawn(text_field(&theme, 300.0, "Search", 20))
        .id();
    app.world_mut()
        .spawn((UiScreen, Node::default()))
        .add_child(field);
    app.init_resource::<Changes>().add_observer(
        |change: On<ValueChange<String>>, mut changes: ResMut<Changes>| {
            changes.0.push(change.value.clone());
        },
    );
    run_more(&mut app, "wait 2");
    let value = |app: &App| app.world().get::<TextField>(field).unwrap().value.clone();
    let at = app
        .world()
        .get::<UiGlobalTransform>(field)
        .unwrap()
        .translation;

    // Not focused yet: typing goes nowhere.
    run_more(&mut app, "type x\nwait 2");
    assert_eq!(value(&app), "");
    assert!(!app.world_mut().run_system_cached(typing).unwrap());

    run_more(
        &mut app,
        &format!("cursor {} {}\nwait 1\nhold MouseLeft 1\nwait 1", at.x, at.y),
    );
    assert!(app.world_mut().run_system_cached(typing).unwrap());
    // The placeholder goes on the very frame the first character does.
    run_more(&mut app, "type i");
    let mut placeholders = app
        .world_mut()
        .query_filtered::<&TextColor, With<Placeholder>>();
    assert_eq!(placeholders.single(app.world()).unwrap().0, Color::NONE);
    run_more(&mut app, "type ron\nwait 2");
    assert_eq!(value(&app), "iron");
    assert_eq!(app.world().resource::<Changes>().0.last().unwrap(), "iron");

    run_more(
        &mut app,
        "hold Digit7 1\nwait 1\nhold ArrowLeft 1\nwait 1\nhold Backspace 1\nwait 2",
    );
    assert_eq!(value(&app), "iro7");
    let mut players = app
        .world_mut()
        .query_filtered::<&ActiveSlot, With<defaria_core::player::LocalPlayer>>();
    assert_eq!(*players.single(app.world()).unwrap(), ActiveSlot(0));

    // Escape takes the keyboard, and closes the screen as ever.
    run_more(&mut app, "hold Escape 1\nwait 2");
    assert!(!app.world_mut().run_system_cached(typing).unwrap());
    assert!(app.world().get_entity(field).is_err());
}

/// Through the real game: the pointer over a slot in the inventory
/// shows its item's name beside it, and off it the box hides.
#[test]
fn hovering_a_slot_shows_its_name() {
    use crate::headless::{Script, run_more, settle, test_game};
    let mut app = test_game(Script::parse("wait 1").unwrap());
    settle(&mut app);
    run_more(&mut app, "hold KeyE 1\nwait 3");
    let mut slots = app
        .world_mut()
        .query::<(&Tooltip, &bevy::ui::UiGlobalTransform)>();
    let at = slots
        .iter(app.world())
        .find(|(tip, _)| tip.0 == "Stone")
        .unwrap()
        .1
        .translation;
    run_more(&mut app, &format!("cursor {} {}\nwait 2", at.x, at.y));
    let mut boxes = app
        .world_mut()
        .query_filtered::<(&Node, &Text, &Visibility), With<TooltipBox>>();
    let (node, text, visibility) = boxes.single(app.world()).unwrap();
    assert_eq!(text.0, "Stone");
    assert_eq!(*visibility, Visibility::Inherited);
    assert_eq!((node.left, node.top), (px(at.x + 14.0), px(at.y + 14.0)));

    // Hidden while a button is held.
    run_more(&mut app, "press MouseRight\nwait 2");
    let (_, _, visibility) = boxes.single(app.world()).unwrap();
    assert_eq!(*visibility, Visibility::Hidden);
    run_more(&mut app, "release MouseRight\nwait 2\ncursor 5 5\nwait 2");
    let (_, _, visibility) = boxes.single(app.world()).unwrap();
    assert_eq!(*visibility, Visibility::Hidden);
}
