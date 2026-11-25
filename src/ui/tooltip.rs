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
    mut boxes: Query<(&mut Node, &mut Text, &mut Visibility), With<TooltipBox>>,
) {
    let tip = pointer
        .hovered
        .and_then(|entity| tips.get(entity).ok())
        .filter(|tip| !tip.0.is_empty());
    let scale = window.map_or(1.0, |window| window.scale_factor());
    for (mut node, mut text, mut visibility) in &mut boxes {
        let (Some(tip), Some(at)) = (tip, pointer.position) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        visibility.set_if_neq(Visibility::Inherited);
        if text.0 != tip.0 {
            text.0.clone_from(&tip.0);
        }
        let at = at / scale + OFFSET;
        let (left, top) = (px(at.x), px(at.y));
        if node.left != left || node.top != top {
            node.left = left;
            node.top = top;
        }
    }
}
