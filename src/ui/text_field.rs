//! Single-line text fields, on Bevy's [`TextInput`]: typing, moving the
//! cursor (arrows, Home and End, by word with Option or Ctrl), selecting
//! (Shift, dragging, double click), the clipboard, and IME all come from
//! Bevy. This adds the frame, a placeholder shown while it's empty, the
//! frame lighting up while it has the keyboard, and a
//! [`ValueChange<String>`] on the field whenever its text changes.
//!
//! Clicking a field gives it the keyboard; clicking elsewhere or Escape
//! takes it away. While a field has the keyboard, keys meant for the game
//! (E, the number keys) are text: their systems run only while not
//! [`typing`].

use bevy::{
    input_focus::{InputFocus, tab_navigation::TabIndex},
    picking::hover::Hovered,
    prelude::*,
    text::{EditableText, TextCursorStyle, TextEditChange},
    ui_widgets::{TextInput, ValueChange},
};

use super::{UiTheme, UiWidgetSystems};

pub struct TextFieldPlugin;

impl Plugin for TextFieldPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<TextField>()
            .add_observer(report_changes)
            .add_systems(PostUpdate, style_text_fields.in_set(UiWidgetSystems));
    }
}

/// A text field's frame, with what's typed in it so far.
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq, Eq)]
#[reflect(Component)]
#[require(Hovered)]
pub struct TextField {
    pub value: String,
}

/// The editable text inside a [`TextField`].
#[derive(Component, Debug, Clone, Copy)]
pub struct TextFieldInput;

/// The text a [`TextField`] shows while it's empty.
#[derive(Component, Debug, Clone, Copy)]
struct Placeholder;

/// A run condition: a text field has the keyboard, so keys are text.
pub fn typing(focus: Option<Res<InputFocus>>, inputs: Query<(), With<EditableText>>) -> bool {
    focus
        .and_then(|focus| focus.get())
        .is_some_and(|entity| inputs.contains(entity))
}

/// An empty text field `width` logical pixels wide, showing `placeholder`
/// until something's typed, and holding at most `max` characters.
pub fn text_field(
    theme: &UiTheme,
    width: f32,
    placeholder: impl Into<String>,
    max: usize,
) -> impl Bundle {
    (
        TextField::default(),
        Node {
            width: px(width),
            border: UiRect::all(px(theme.slot_border)),
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(theme.field),
        BorderColor::all(theme.slot_edge),
        children![
            (
                TextFieldInput,
                TextInput,
                // Focusable: clicking it gives it the keyboard.
                TabIndex(0),
                EditableText {
                    max_characters: Some(max),
                    ..default()
                },
                theme.text_font(),
                TextColor(theme.text),
                TextCursorStyle {
                    color: theme.text,
                    selection_color: theme.field_focused.with_alpha(0.4),
                    unfocused_selection_color: Color::NONE,
                    ..default()
                },
                Node {
                    flex_grow: 1.0,
                    padding: UiRect::all(px(theme.field_padding)),
                    ..default()
                },
            ),
            (
                Placeholder,
                Text::new(placeholder),
                theme.text_font(),
                TextColor(theme.text_dim),
                Node {
                    position_type: PositionType::Absolute,
                    left: px(theme.field_padding),
                    ..default()
                },
                Pickable::IGNORE,
            ),
        ],
    )
}

/// Copies an edited input's text to its field, and tells the field's
/// observers if it changed (Bevy reports cursor moves too).
fn report_changes(
    change: On<TextEditChange>,
    inputs: Query<(&EditableText, &ChildOf), With<TextFieldInput>>,
    mut fields: Query<&mut TextField>,
    mut commands: Commands,
) {
    let Ok((text, parent)) = inputs.get(change.event_target()) else {
        return;
    };
    let Ok(mut field) = fields.get_mut(parent.parent()) else {
        return;
    };
    let value = text.value().to_string();
    if field.value != value {
        field.value.clone_from(&value);
        commands.trigger(ValueChange {
            source: parent.parent(),
            value,
            is_final: false,
        });
    }
}

/// The frame lights up while the field has the keyboard, and the
/// placeholder shows while it's empty.
fn style_text_fields(
    theme: Res<UiTheme>,
    focus: Option<Res<InputFocus>>,
    mut fields: Query<(&TextField, &Hovered, &Children, &mut BorderColor)>,
    mut placeholders: Query<&mut Visibility, With<Placeholder>>,
) {
    let focused = focus.and_then(|focus| focus.get());
    for (field, hovered, children, mut border) in &mut fields {
        let has_focus = focused.is_some_and(|entity| children.contains(&entity));
        let edge = if has_focus {
            theme.field_focused
        } else if hovered.get() {
            theme.slot_hovered
        } else {
            theme.slot_edge
        };
        border.set_if_neq(BorderColor::all(edge));
        let shown = if field.value.is_empty() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        for &child in children {
            if let Ok(mut visibility) = placeholders.get_mut(child) {
                visibility.set_if_neq(shown);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        headless::{Script, run_more, settle, test_game},
        hotbar::ActiveSlot,
        ui::UiScreen,
    };
    use bevy::ui::UiGlobalTransform;

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
        run_more(&mut app, "type iron\nwait 2");
        assert_eq!(value(&app), "iron");
        assert_eq!(app.world().resource::<Changes>().0.last().unwrap(), "iron");

        run_more(
            &mut app,
            "hold Digit7 1\nwait 1\nhold ArrowLeft 1\nwait 1\nhold Backspace 1\nwait 2",
        );
        assert_eq!(value(&app), "iro7");
        assert_eq!(*app.world().resource::<ActiveSlot>(), ActiveSlot(0));

        // Escape takes the keyboard, and closes the screen as ever.
        run_more(&mut app, "hold Escape 1\nwait 2");
        assert!(!app.world_mut().run_system_cached(typing).unwrap());
        assert!(app.world().get_entity(field).is_err());
    }

    #[test]
    fn the_placeholder_shows_only_while_empty() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<UiTheme>()
            .add_plugins(TextFieldPlugin);
        let theme = UiTheme::default();
        let field = app
            .world_mut()
            .spawn(text_field(&theme, 100.0, "Search", 8))
            .id();
        let shown = |app: &mut App| {
            let mut placeholders = app
                .world_mut()
                .query_filtered::<&Visibility, With<Placeholder>>();
            *placeholders.single(app.world()).unwrap()
        };
        app.update();
        assert_eq!(shown(&mut app), Visibility::Inherited);
        app.world_mut().get_mut::<TextField>(field).unwrap().value = "a".into();
        app.update();
        assert_eq!(shown(&mut app), Visibility::Hidden);
    }
}
