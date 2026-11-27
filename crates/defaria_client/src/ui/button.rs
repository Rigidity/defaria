//! Buttons: Bevy's headless [`Button`](bevy::ui_widgets::Button), framed
//! and colored from the theme.
//!
//! Bevy's widget does the behaviour: it's pressed while the pointer holds
//! it down ([`Pressed`]), sends [`Activate`](bevy::ui_widgets::Activate)
//! when it's clicked (or Enter or Space while it has the keyboard focus),
//! and does nothing with [`InteractionDisabled`]. Whoever spawns one
//! observes its `Activate`.

use bevy::{
    picking::hover::Hovered,
    prelude::*,
    ui::{InteractionDisabled, Pressed},
};

use super::{UiTheme, UiWidgetSystems};

pub struct ButtonPlugin;

impl Plugin for ButtonPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<UiButton>()
            .add_systems(PostUpdate, style_buttons.in_set(UiWidgetSystems));
    }
}

/// One of our buttons, styled by the theme.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
#[require(bevy::ui_widgets::Button, Hovered)]
pub struct UiButton;

/// A button showing `label`, ready to spawn.
pub fn button(theme: &UiTheme, label: impl Into<String>) -> impl Bundle {
    (
        UiButton,
        Node {
            padding: UiRect::axes(px(theme.button_padding), px(theme.button_padding / 2.0)),
            border: UiRect::all(px(theme.slot_border)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(theme.button),
        BorderColor::all(theme.button_edge),
        children![(
            Text::new(label),
            theme.text_font(),
            TextColor(theme.text),
            // Picking finds the button itself.
            Pickable::IGNORE,
        )],
    )
}

/// The colors for how the button is: disabled, held down, hovered or at
/// rest.
pub fn button_colors(
    theme: &UiTheme,
    hovered: bool,
    pressed: bool,
    disabled: bool,
) -> (Color, Color, Color) {
    if disabled {
        (theme.slot, theme.slot_edge, theme.text_dim)
    } else if pressed {
        (theme.button_pressed, theme.slot_hovered, theme.text)
    } else if hovered {
        (theme.button_hovered, theme.slot_hovered, theme.text)
    } else {
        (theme.button, theme.button_edge, theme.text)
    }
}

fn style_buttons(
    theme: Res<UiTheme>,
    mut buttons: Query<
        (
            &Hovered,
            Has<Pressed>,
            Has<InteractionDisabled>,
            &mut BackgroundColor,
            &mut BorderColor,
            &Children,
        ),
        With<UiButton>,
    >,
    mut labels: Query<&mut TextColor>,
) {
    for (hovered, pressed, disabled, mut background, mut border, children) in &mut buttons {
        let (fill, edge, text) = button_colors(&theme, hovered.get(), pressed, disabled);
        background.set_if_neq(BackgroundColor(fill));
        border.set_if_neq(BorderColor::all(edge));
        for &child in children {
            if let Ok(mut color) = labels.get_mut(child) {
                color.set_if_neq(TextColor(text));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_button_shows_how_it_is() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<UiTheme>()
            .add_plugins(ButtonPlugin);
        let theme = UiTheme::default();
        let button = app.world_mut().spawn(button(&theme, "Done")).id();
        let look = |app: &App| {
            let world = app.world();
            let label = world.get::<Children>(button).unwrap()[0];
            (
                world.get::<BackgroundColor>(button).unwrap().0,
                world.get::<TextColor>(label).unwrap().0,
            )
        };
        app.update();
        assert_eq!(look(&app), (theme.button, theme.text));
        app.world_mut().entity_mut(button).insert(Hovered(true));
        app.update();
        assert_eq!(look(&app), (theme.button_hovered, theme.text));
        app.world_mut().entity_mut(button).insert(Pressed);
        app.update();
        assert_eq!(look(&app), (theme.button_pressed, theme.text));
        app.world_mut()
            .entity_mut(button)
            .insert(InteractionDisabled);
        app.update();
        assert_eq!(look(&app), (theme.slot, theme.text_dim));
    }
}
