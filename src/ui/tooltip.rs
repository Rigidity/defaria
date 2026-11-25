//! Tooltips: text shown beside the pointer while it's over a node with a
//! [`Tooltip`]. One tooltip box exists, drawn above everything, and moves
//! to whatever is hovered.

use bevy::{prelude::*, window::PrimaryWindow};

use super::{UiPointer, UiPointerSystems, UiTheme};

pub struct TooltipPlugin;

impl Plugin for TooltipPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Tooltip>()
            .add_systems(Startup, spawn_tooltip)
            .add_systems(PreUpdate, show_tooltip.after(UiPointerSystems));
    }
}

/// The text to show while the pointer is over this node. Empty shows
/// nothing.
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq, Eq)]
#[reflect(Component)]
pub struct Tooltip(pub String);

/// The tooltip box.
#[derive(Component, Debug, Clone, Copy)]
pub struct TooltipBox;

/// From the pointer to the tooltip's top left corner, in logical pixels.
const OFFSET: Vec2 = Vec2::new(14.0, 14.0);

fn spawn_tooltip(mut commands: Commands, theme: Res<UiTheme>) {
    commands.spawn((
        Name::new("Tooltip"),
        TooltipBox,
        Node {
            position_type: PositionType::Absolute,
            padding: UiRect::axes(px(8), px(4)),
            border: UiRect::all(px(1)),
            ..default()
        },
        BackgroundColor(theme.tooltip),
        BorderColor::all(theme.panel_border),
        GlobalZIndex(i32::MAX),
        Visibility::Hidden,
        Text::default(),
        TextFont::from_font_size(theme.font_size),
        TextColor(theme.text),
    ));
}

fn show_tooltip(
    pointer: Res<UiPointer>,
    tips: Query<&Tooltip>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    mut boxes: Query<(&mut Node, &mut Text, &mut Visibility, &ComputedNode), With<TooltipBox>>,
) {
    let tip = pointer
        .hovered
        .and_then(|entity| tips.get(entity).ok())
        .filter(|tip| !tip.0.is_empty());
    let (scale, screen) = window.map_or((1.0, Vec2::INFINITY), |window| {
        (window.scale_factor(), window.size())
    });
    for (mut node, mut text, mut visibility, computed) in &mut boxes {
        let (Some(tip), Some(at)) = (tip, pointer.position) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        visibility.set_if_neq(Visibility::Inherited);
        if text.0 != tip.0 {
            text.0.clone_from(&tip.0);
        }
        // Last frame's size, which is this text's unless it just changed.
        let size = computed.size * computed.inverse_scale_factor;
        let at = place(at / scale, size, screen);
        let (left, top) = (px(at.x), px(at.y));
        if node.left != left || node.top != top {
            node.left = left;
            node.top = top;
        }
    }
}

/// Where a tooltip `size` big goes for a pointer at `pointer`, all in
/// logical pixels: below and right of it, or above or left where it would
/// run off a `screen`-sized window, and never off the top or left.
pub fn place(pointer: Vec2, size: Vec2, screen: Vec2) -> Vec2 {
    let fit = |axis: usize| {
        let after = pointer[axis] + OFFSET[axis];
        let at = if after + size[axis] > screen[axis] {
            pointer[axis] - OFFSET[axis] - size[axis]
        } else {
            after
        };
        at.max(0.0)
    };
    Vec2::new(fit(0), fit(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{Hoverable, pointer::tests as pointer};

    #[test]
    fn tooltips_sit_beside_the_pointer_and_stay_on_screen() {
        let screen = Vec2::new(1280.0, 720.0);
        let size = Vec2::new(100.0, 30.0);
        assert_eq!(
            place(Vec2::new(10.0, 10.0), size, screen),
            Vec2::new(24.0, 24.0)
        );
        // Near the bottom right corner: above and left of the pointer.
        assert_eq!(
            place(Vec2::new(1250.0, 700.0), size, screen),
            Vec2::new(1136.0, 656.0)
        );
        // Bigger than the window: pinned to its top left.
        assert_eq!(place(Vec2::new(5.0, 5.0), screen * 2.0, screen), Vec2::ZERO);
    }

    #[test]
    fn hovering_a_node_with_a_tooltip_shows_it() {
        let mut app = pointer::app();
        app.init_resource::<UiTheme>().add_plugins(TooltipPlugin);
        pointer::open_screen(&mut app);
        let node = pointer::node(&mut app, Vec2::new(50.0, 50.0), 40.0, 1);
        app.world_mut()
            .entity_mut(node)
            .insert((Hoverable, Tooltip("Stone".into())));
        pointer::move_to(&mut app, Vec2::new(50.0, 50.0));
        app.update();
        let mut boxes = app
            .world_mut()
            .query_filtered::<(&Node, &Text, &Visibility), With<TooltipBox>>();
        let (node_style, text, visibility) = boxes.single(app.world()).unwrap();
        assert_eq!(text.0, "Stone");
        assert_eq!(*visibility, Visibility::Inherited);
        assert_eq!((node_style.left, node_style.top), (px(64.0), px(64.0)));

        pointer::move_to(&mut app, Vec2::new(500.0, 50.0));
        app.update();
        let (_, _, visibility) = boxes.single(app.world()).unwrap();
        assert_eq!(*visibility, Visibility::Hidden);
    }
}
