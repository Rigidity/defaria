//! Scrolling: the mouse wheel over a [`Scrollable`] node scrolls it, and a
//! scrollbar beside it shows how far.
//!
//! [`scroll_panel`] spawns both. The scrollbar is Bevy's
//! (`bevy::ui_widgets::Scrollbar`), which sizes and places its thumb from
//! the scrolled node; it's left out while everything fits. Only vertical
//! scrolling for now.

use bevy::{
    input::mouse::{AccumulatedMouseScroll, MouseScrollUnit},
    prelude::*,
    ui::UiSystems,
    ui_widgets::{ControlOrientation, Scrollbar, ScrollbarThumb},
};

use super::{
    UiTheme,
    pointer::{HitTestData, UiPointer},
};

pub struct ScrollPlugin;

impl Plugin for ScrollPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Scrollable>()
            .add_systems(Update, scroll_with_wheel)
            .add_systems(
                PostUpdate,
                (keep_scroll_in_range, hide_idle_scrollbars).after(UiSystems::Layout),
            );
    }
}

/// A node the wheel scrolls while the pointer is over it.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
#[require(ScrollPosition)]
pub struct Scrollable {
    /// How far one notch of the wheel scrolls, in logical pixels.
    pub line: f32,
}

/// A scrollable node laid out as `node` (its `height` is the view's), with
/// a scrollbar beside it, spawned under `parent`. `line` is how far a notch
/// of the wheel scrolls; `extra` is added to the scrolled node, and
/// `content` spawns its children. Returns the scrolled node.
pub fn scroll_panel(
    parent: &mut ChildSpawnerCommands,
    theme: &UiTheme,
    node: Node,
    line: f32,
    extra: impl Bundle,
    content: impl FnOnce(&mut ChildSpawnerCommands),
) -> Entity {
    let height = node.height;
    let mut scrolled = Entity::PLACEHOLDER;
    parent
        .spawn(Node {
            column_gap: px(theme.slot_gap),
            ..default()
        })
        .with_children(|row| {
            scrolled = row
                .spawn((
                    Scrollable { line },
                    Node {
                        overflow: Overflow::scroll_y(),
                        ..node
                    },
                    extra,
                ))
                .with_children(content)
                .id();
            row.spawn((
                Name::new("Scrollbar"),
                Scrollbar::new(
                    scrolled,
                    ControlOrientation::Vertical,
                    theme.slot_size / 2.0,
                ),
                Node {
                    width: px(theme.scrollbar_width),
                    height,
                    ..default()
                },
                BackgroundColor(theme.scrollbar_track),
                children![(
                    ScrollbarThumb::default(),
                    BackgroundColor(theme.scrollbar_thumb),
                )],
            ));
        });
    scrolled
}

/// How far down a scroll position can go, in logical pixels: the content
/// past the bottom of the view.
pub fn max_scroll(node: &ComputedNode) -> f32 {
    let view = node.size.y - node.border.min_inset.y - node.border.max_inset.y;
    (node.content_size.y - view).max(0.0) * node.inverse_scale_factor
}

/// The wheel scrolls the top scrollable node the pointer is over.
fn scroll_with_wheel(
    wheel: Res<AccumulatedMouseScroll>,
    pointer: Res<UiPointer>,
    targets: Query<HitTestData, With<Scrollable>>,
    mut nodes: Query<(&Scrollable, &mut ScrollPosition, &ComputedNode)>,
) {
    if wheel.delta.y == 0.0 {
        return;
    }
    let Some((scrollable, mut position, node)) = pointer
        .topmost(&targets)
        .and_then(|entity| nodes.get_mut(entity).ok())
    else {
        return;
    };
    let pixels = match wheel.unit {
        MouseScrollUnit::Line => wheel.delta.y * scrollable.line,
        // Physical pixels, from a touchpad.
        MouseScrollUnit::Pixel => wheel.delta.y * node.inverse_scale_factor,
    };
    // The wheel turned toward you (negative) scrolls down.
    position.y = (position.y - pixels).clamp(0.0, max_scroll(node));
}

/// The layout clamps what it shows but not the [`ScrollPosition`] itself;
/// keep that in range too, so turning back the other way acts at once
/// after the content shrinks.
fn keep_scroll_in_range(mut nodes: Query<(&mut ScrollPosition, &ComputedNode), With<Scrollable>>) {
    for (mut position, node) in &mut nodes {
        let clamped = position.y.clamp(0.0, max_scroll(node));
        if clamped != position.y {
            position.y = clamped;
        }
    }
}

/// Takes a scrollbar out of the layout while everything it scrolls fits
/// in view.
fn hide_idle_scrollbars(
    mut bars: Query<(&Scrollbar, &mut Node)>,
    targets: Query<&ComputedNode, With<Scrollable>>,
) {
    for (bar, mut style) in &mut bars {
        let Ok(node) = targets.get(bar.target) else {
            continue;
        };
        let wanted = if max_scroll(node) > 0.0 {
            Display::Flex
        } else {
            Display::None
        };
        if style.display != wanted {
            style.display = wanted;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::pointer::tests as pointer;

    fn app() -> (App, Entity) {
        let mut app = pointer::app();
        app.add_plugins(ScrollPlugin)
            .init_resource::<AccumulatedMouseScroll>();
        pointer::open_screen(&mut app);
        let list = pointer::node(&mut app, Vec2::new(100.0, 100.0), 100.0, 1);
        app.world_mut().entity_mut(list).insert((
            Scrollable { line: 20.0 },
            ComputedNode {
                size: Vec2::splat(100.0),
                content_size: Vec2::new(100.0, 250.0),
                inverse_scale_factor: 0.5,
                ..default()
            },
        ));
        (app, list)
    }

    fn turn(app: &mut App, unit: MouseScrollUnit, y: f32) {
        app.insert_resource(AccumulatedMouseScroll {
            unit,
            delta: Vec2::new(0.0, y),
        });
        app.update();
        app.insert_resource(AccumulatedMouseScroll::default());
    }

    fn scrolled(app: &App, list: Entity) -> f32 {
        app.world().get::<ScrollPosition>(list).unwrap().y
    }

    #[test]
    fn the_wheel_scrolls_the_node_under_the_pointer_within_its_content() {
        let (mut app, list) = app();
        // Not over it: nothing.
        pointer::move_to(&mut app, Vec2::new(400.0, 400.0));
        turn(&mut app, MouseScrollUnit::Line, -1.0);
        assert_eq!(scrolled(&app, list), 0.0);

        pointer::move_to(&mut app, Vec2::new(100.0, 100.0));
        turn(&mut app, MouseScrollUnit::Line, -2.0);
        assert_eq!(scrolled(&app, list), 40.0);
        // Pixels are physical: 20 of them at scale 2 is 10 logical.
        turn(&mut app, MouseScrollUnit::Pixel, -20.0);
        assert_eq!(scrolled(&app, list), 50.0);
        // 150 physical pixels past the view is as far as it goes: 75.
        turn(&mut app, MouseScrollUnit::Line, -10.0);
        assert_eq!(scrolled(&app, list), 75.0);
        turn(&mut app, MouseScrollUnit::Line, 100.0);
        assert_eq!(scrolled(&app, list), 0.0);
    }

    #[test]
    fn a_hidden_scrollable_doesnt_take_the_wheel() {
        let (mut app, list) = app();
        app.world_mut()
            .entity_mut(list)
            .insert(InheritedVisibility::HIDDEN);
        pointer::move_to(&mut app, Vec2::new(100.0, 100.0));
        turn(&mut app, MouseScrollUnit::Line, -2.0);
        assert_eq!(scrolled(&app, list), 0.0);
    }

    /// When the content shrinks, the position comes back in range.
    #[test]
    fn the_position_is_clamped_when_the_content_shrinks() {
        let (mut app, list) = app();
        pointer::move_to(&mut app, Vec2::new(100.0, 100.0));
        turn(&mut app, MouseScrollUnit::Line, -10.0);
        assert_eq!(scrolled(&app, list), 75.0);
        app.world_mut()
            .get_mut::<ComputedNode>(list)
            .unwrap()
            .content_size
            .y = 140.0;
        app.update();
        assert_eq!(scrolled(&app, list), 20.0);
    }

    #[test]
    fn the_scrollbar_is_left_out_while_everything_fits() {
        let (mut app, list) = app();
        let bar = app
            .world_mut()
            .spawn((
                Scrollbar::new(list, ControlOrientation::Vertical, 10.0),
                Node::default(),
            ))
            .id();
        let display = |app: &App| app.world().get::<Node>(bar).unwrap().display;
        app.update();
        assert_eq!(display(&app), Display::Flex);
        app.world_mut()
            .get_mut::<ComputedNode>(list)
            .unwrap()
            .content_size
            .y = 80.0;
        app.update();
        assert_eq!(display(&app), Display::None);
    }

    #[test]
    fn content_that_fits_doesnt_scroll() {
        let node = ComputedNode {
            size: Vec2::splat(100.0),
            content_size: Vec2::splat(80.0),
            ..default()
        };
        assert_eq!(max_scroll(&node), 0.0);
    }
}
