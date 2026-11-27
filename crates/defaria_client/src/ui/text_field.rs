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
    text::{EditableText, EditableTextSystems, TextCursorStyle, TextEditChange},
    ui::UiSystems,
    ui_widgets::{TextInput, ValueChange},
};

use super::{UiTheme, UiWidgetSystems};

pub struct TextFieldPlugin;

impl Plugin for TextFieldPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<TextField>()
            .add_observer(report_changes)
            .register_type::<TextFieldInput>()
            .add_systems(
                PostUpdate,
                (
                    style_text_fields.in_set(UiWidgetSystems),
                    // After Bevy applies the edits (which it does after
                    // layout), so the placeholder goes on the frame the
                    // first character is typed.
                    show_placeholders
                        .after(EditableTextSystems)
                        .before(UiSystems::PostLayout),
                ),
            );
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
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct TextFieldInput;

/// The text a [`TextField`] shows while it's empty.
#[derive(Component, Debug, Clone, Copy)]
pub struct Placeholder;

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

/// The frame lights up while the field has the keyboard.
fn style_text_fields(
    theme: Res<UiTheme>,
    focus: Option<Res<InputFocus>>,
    mut fields: Query<(&Hovered, &Children, &mut BorderColor), With<TextField>>,
) {
    let focused = focus.and_then(|focus| focus.get());
    for (hovered, children, mut border) in &mut fields {
        let has_focus = focused.is_some_and(|entity| children.contains(&entity));
        let edge = if has_focus {
            theme.field_focused
        } else if hovered.get() {
            theme.slot_hovered
        } else {
            theme.slot_edge
        };
        border.set_if_neq(BorderColor::all(edge));
    }
}

/// The placeholder shows while the field is empty: its colour, rather than
/// its visibility, since this runs after visibility has been worked out.
fn show_placeholders(
    theme: Res<UiTheme>,
    fields: Query<(&TextField, &Children)>,
    mut placeholders: Query<&mut TextColor, With<Placeholder>>,
) {
    for (field, children) in &fields {
        let color = if field.value.is_empty() {
            theme.text_dim
        } else {
            Color::NONE
        };
        for &child in children {
            if let Ok(mut text) = placeholders.get_mut(child) {
                text.set_if_neq(TextColor(color));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                .query_filtered::<&TextColor, With<Placeholder>>();
            placeholders.single(app.world()).unwrap().0
        };
        app.update();
        assert_eq!(shown(&mut app), theme.text_dim);
        app.world_mut().get_mut::<TextField>(field).unwrap().value = "a".into();
        app.update();
        assert_eq!(shown(&mut app), Color::NONE);
    }
}
