//! A screen showing every widget, for working on how they look and
//! behave: F10 opens and closes it. Only in dev builds.
//!
//! A button counts its clicks into the line under the widgets, and the
//! text field echoes what's typed there, so a scenario can see both work.

use bevy::{
    prelude::*,
    ui::InteractionDisabled,
    ui_widgets::{Activate, ValueChange},
};

use super::{ItemSlot, UiScreen, UiTheme, button, item_slot, text_field, typing};
use crate::content::Content;

pub struct GalleryPlugin;

impl Plugin for GalleryPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Gallery>()
            .add_systems(Update, toggle_gallery.run_if(not(typing)));
    }
}

/// The gallery screen's root.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
#[require(UiScreen)]
pub struct Gallery;

/// The line reporting what the widgets did.
#[derive(Component, Debug, Clone, Copy)]
struct Report;

fn toggle_gallery(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    galleries: Query<Entity, With<Gallery>>,
    screens: Query<(), With<UiScreen>>,
    theme: Res<UiTheme>,
    content: Res<Content>,
) {
    if !keys.just_pressed(KeyCode::F10) {
        return;
    }
    if let Some(gallery) = galleries.iter().next() {
        commands.entity(gallery).try_despawn();
    } else if screens.is_empty() {
        spawn_gallery(&mut commands, &theme, &content);
    }
}

fn spawn_gallery(commands: &mut Commands, theme: &UiTheme, content: &Content) {
    let row = || Node {
        column_gap: px(theme.slot_gap * 2.0),
        align_items: AlignItems::Center,
        ..default()
    };
    commands
        .spawn((
            Name::new("Widget gallery"),
            Gallery,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(theme.backdrop),
            Pickable::IGNORE,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(px(theme.panel_padding)),
                    border: UiRect::all(px(1)),
                    row_gap: px(12),
                    ..default()
                },
                BackgroundColor(theme.panel),
                BorderColor::all(theme.panel_border),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("Widgets"),
                    theme.title_font(),
                    TextColor(theme.text),
                ));
                panel.spawn(row()).with_children(|buttons| {
                    buttons.spawn(button(theme, "Button")).observe(count_clicks);
                    buttons.spawn((button(theme, "Disabled"), InteractionDisabled));
                });
                panel
                    .spawn(text_field(theme, 320.0, "Type here", 32))
                    .observe(echo_text);
                panel.spawn(row()).with_children(|slots| {
                    let count = content.item_states.len().min(3) as u32;
                    for index in 0..count {
                        slots.spawn(item_slot(
                            theme,
                            ItemSlot {
                                count: index * 20,
                                selected: index == 1,
                                ..ItemSlot::item(crate::content::ItemState(index))
                            },
                        ));
                    }
                });
                panel.spawn((
                    Report,
                    Text::new("Nothing yet"),
                    theme.text_font(),
                    TextColor(theme.text_dim),
                ));
            });
        });
}

fn count_clicks(
    _: On<Activate>,
    mut reports: Query<&mut Text, With<Report>>,
    mut clicks: Local<u32>,
) {
    *clicks += 1;
    for mut text in &mut reports {
        text.0 = format!("Clicked {} time(s)", *clicks);
    }
}

fn echo_text(change: On<ValueChange<String>>, mut reports: Query<&mut Text, With<Report>>) {
    for mut text in &mut reports {
        text.0 = format!("Typed: {}", change.value);
    }
}
