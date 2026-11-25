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

    /// On a scale-2 window the pointer is in physical pixels and the box
    /// is placed in logical ones, flipping by its logical size at the edge.
    #[test]
    fn hovering_a_node_with_a_tooltip_shows_it() {
        let mut app = pointer::app();
        app.init_resource::<UiTheme>().add_plugins(TooltipPlugin);
        let mut windows = app.world_mut().query::<&mut Window>();
        windows
            .single_mut(app.world_mut())
            .unwrap()
            .resolution
            .set_scale_factor_override(Some(2.0));
        pointer::open_screen(&mut app);
        for (at, name) in [
            (Vec2::new(100.0, 100.0), "Stone"),
            (Vec2::new(1260.0, 700.0), "Dirt"),
        ] {
            let node = pointer::node(&mut app, at, 20.0, 1);
            app.world_mut()
                .entity_mut(node)
                .insert((Hoverable, Tooltip(name.into())));
        }
        app.update();
        // The box's size from layout: 200×60 physical, 100×30 logical.
        let mut boxes = app
            .world_mut()
            .query_filtered::<(&Node, &Text, &Visibility, &mut ComputedNode), With<TooltipBox>>();
        *boxes.single_mut(app.world_mut()).unwrap().3 = ComputedNode {
            size: Vec2::new(200.0, 60.0),
            inverse_scale_factor: 0.5,
            ..default()
        };

        pointer::move_to(&mut app, Vec2::new(100.0, 100.0));
        app.update();
        let (style, text, visibility, _) = boxes.single(app.world()).unwrap();
        assert_eq!(text.0, "Stone");
        assert_eq!(*visibility, Visibility::Inherited);
        assert_eq!((style.left, style.top), (px(64.0), px(64.0)));

        // At (630, 350) logical in a 640×360 window: above and left.
        pointer::move_to(&mut app, Vec2::new(1260.0, 700.0));
        app.update();
        let (style, text, ..) = boxes.single(app.world()).unwrap();
        assert_eq!(text.0, "Dirt");
        assert_eq!((style.left, style.top), (px(516.0), px(306.0)));

        pointer::move_to(&mut app, Vec2::new(600.0, 50.0));
        app.update();
        let (_, _, visibility, _) = boxes.single(app.world()).unwrap();
        assert_eq!(*visibility, Visibility::Hidden);
    }
}
