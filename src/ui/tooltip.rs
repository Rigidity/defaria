//! Tooltips: text shown beside the pointer while it's over a node with a
//! [`Tooltip`]. One tooltip box exists, drawn above everything, and moves
//! to whatever is hovered. Picking passes through it. It hides while a
//! mouse button is held, so it doesn't get in the way of a drag.

use bevy::{picking::hover::Hovered, prelude::*, window::PrimaryWindow};

use super::{UiPointer, UiTheme};

pub struct TooltipPlugin;

impl Plugin for TooltipPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Tooltip>()
            .add_systems(Startup, spawn_tooltip)
            .add_systems(Update, show_tooltip);
    }
}

/// The text to show while the pointer is over this node. Empty shows
/// nothing.
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq, Eq)]
#[reflect(Component)]
#[require(Hovered)]
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
        Pickable::IGNORE,
        Visibility::Hidden,
        Text::default(),
        theme.text_font(),
        TextColor(theme.text),
    ));
}

fn show_tooltip(
    pointer: UiPointer,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    tips: Query<(&Tooltip, &Hovered)>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    mut boxes: Query<(&mut Node, &mut Text, &mut Visibility, &ComputedNode), With<TooltipBox>>,
) {
    let held = mouse.is_some_and(|mouse| mouse.get_pressed().next().is_some());
    let tip = tips
        .iter()
        .find(|(tip, hovered)| hovered.get() && !tip.0.is_empty())
        .map(|(tip, _)| tip)
        .filter(|_| !held);
    let screen = window.map_or(Vec2::INFINITY, |window| window.size());
    for (mut node, mut text, mut visibility, computed) in &mut boxes {
        let (Some(tip), Some(at)) = (tip, pointer.position()) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        visibility.set_if_neq(Visibility::Inherited);
        if text.0 != tip.0 {
            text.0.clone_from(&tip.0);
        }
        // Last frame's size, which is this text's unless it just changed.
        let size = computed.size * computed.inverse_scale_factor;
        let at = place(at, size, screen);
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
}
