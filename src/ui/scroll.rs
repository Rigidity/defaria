//! Scrolling: the mouse wheel over a [`Scrollable`] node scrolls it, and a
//! scrollbar beside it shows how far.
//!
//! [`scroll_panel`] spawns both. The scrollbar is Bevy's
//! (`bevy::ui_widgets::Scrollbar`), which sizes and places its thumb from
//! the scrolled node, and can be dragged; it's hidden (keeping its space)
//! while everything fits. Only vertical scrolling for now.
//!
//! The wheel arrives as picking's `PointerScroll`, bubbling up from the
//! node under the pointer to the [`Scrollable`] holding it. It isn't
//! Bevy's `ScrollArea`, which scrolls every list by the same fixed 100
//! pixels a notch: a slot grid scrolls a row of slots.

use bevy::{
    input::mouse::MouseScrollUnit,
    picking::events::PointerScroll,
    prelude::*,
    ui::UiSystems,
    ui_widgets::{ControlOrientation, Scrollbar, ScrollbarThumb},
};

use super::UiTheme;

pub struct ScrollPlugin;

impl Plugin for ScrollPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Scrollable>()
            .add_observer(scroll_with_wheel)
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

/// A scrollable node laid out as `node`, with a scrollbar beside it (as
/// tall as the node), spawned under `parent`. `line` is how far a notch of
/// the wheel scrolls; `extra` is added to the scrolled node, and `content`
/// spawns its children. Returns the scrolled node.
///
/// Unlike the other widgets this spawns rather than returning a bundle,
/// since the scrollbar needs the scrolled node's entity.
pub fn scroll_panel(
    parent: &mut ChildSpawnerCommands,
    theme: &UiTheme,
    node: Node,
    line: f32,
    extra: impl Bundle,
    content: impl FnOnce(&mut ChildSpawnerCommands),
) -> Entity {
    let mut scrolled = Entity::PLACEHOLDER;
    parent
        .spawn(Node {
            column_gap: px(theme.scrollbar_gap),
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
                    theme.scrollbar_min_thumb,
                ),
                // Stretched to the row's height. Hidden until the first
                // layout shows the content doesn't fit, so it never flashes
                // up on a list that fits. Hidden, it keeps its space, so
                // the panel doesn't change width as a list grows or
                // shrinks (filtering it, say).
                Node {
                    width: px(theme.scrollbar_width),
                    ..default()
                },
                Visibility::Hidden,
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

/// The wheel scrolls the scrollable node it's over, or the one whose
/// scrollbar it's over, and goes no further up.
fn scroll_with_wheel(
    mut scroll: On<PointerScroll>,
    bars: Query<&Scrollbar>,
    mut nodes: Query<(&Scrollable, &mut ScrollPosition, &ComputedNode)>,
) {
    let target = bars
        .get(scroll.entity)
        .map_or(scroll.entity, |bar| bar.target);
    let Ok((scrollable, mut position, node)) = nodes.get_mut(target) else {
        return;
    };
    scroll.propagate(false);
    let pixels = match scroll.unit {
        MouseScrollUnit::Line => scroll.y * scrollable.line,
        // Physical pixels, from a touchpad.
        MouseScrollUnit::Pixel => scroll.y * node.inverse_scale_factor,
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

/// Hides a scrollbar while everything it scrolls fits in view.
fn hide_idle_scrollbars(
    mut bars: Query<(&Scrollbar, &mut Visibility)>,
    targets: Query<&ComputedNode, With<Scrollable>>,
) {
    for (bar, mut visibility) in &mut bars {
        let Ok(node) = targets.get(bar.target) else {
            continue;
        };
        let wanted = if max_scroll(node) > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        visibility.set_if_neq(wanted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        camera::NormalizedRenderTarget,
        input::touch::TouchPhase,
        picking::{
            backend::HitData,
            events::Pointer,
            pointer::{Location, PointerId},
        },
    };

    fn app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, ScrollPlugin));
        // Picking's events bubble up by a query that reads windows.
        app.world_mut().register_component::<Window>();
        let list = app
            .world_mut()
            .spawn((
                Scrollable { line: 20.0 },
                ComputedNode {
                    size: Vec2::splat(100.0),
                    content_size: Vec2::new(100.0, 250.0),
                    inverse_scale_factor: 0.5,
                    ..default()
                },
            ))
            .id();
        (app, list)
    }

    /// Picking's wheel event on `entity`.
    fn turn(app: &mut App, entity: Entity, unit: MouseScrollUnit, y: f32) {
        let target = NormalizedRenderTarget::None {
            width: 1280,
            height: 720,
        };
        app.world_mut().trigger(PointerScroll {
            entity,
            pointer: Pointer::new(
                PointerId::Mouse,
                Location {
                    target,
                    position: Vec2::ZERO,
                },
            ),
            unit,
            x: 0.0,
            y,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            phase: TouchPhase::Moved,
        });
        app.update();
    }

    fn scrolled(app: &App, list: Entity) -> f32 {
        app.world().get::<ScrollPosition>(list).unwrap().y
    }

    #[test]
    fn the_wheel_scrolls_the_node_within_its_content() {
        let (mut app, list) = app();
        turn(&mut app, list, MouseScrollUnit::Line, -2.0);
        assert_eq!(scrolled(&app, list), 40.0);
        // Pixels are physical: 20 of them at scale 2 is 10 logical.
        turn(&mut app, list, MouseScrollUnit::Pixel, -20.0);
        assert_eq!(scrolled(&app, list), 50.0);
        // 150 physical pixels past the view is as far as it goes: 75.
        turn(&mut app, list, MouseScrollUnit::Line, -10.0);
        assert_eq!(scrolled(&app, list), 75.0);
        turn(&mut app, list, MouseScrollUnit::Line, 100.0);
        assert_eq!(scrolled(&app, list), 0.0);
    }

    /// The wheel over a node inside the list (a slot) bubbles up to it.
    #[test]
    fn the_wheel_over_a_child_scrolls_the_list() {
        let (mut app, list) = app();
        let child = app.world_mut().spawn(ChildOf(list)).id();
        turn(&mut app, child, MouseScrollUnit::Line, -1.0);
        assert_eq!(scrolled(&app, list), 20.0);
    }

    /// Over its scrollbar, the wheel scrolls what the bar scrolls.
    #[test]
    fn the_wheel_over_a_scrollbar_scrolls_its_target() {
        let (mut app, list) = app();
        let bar = app
            .world_mut()
            .spawn(Scrollbar::new(list, ControlOrientation::Vertical, 10.0))
            .id();
        turn(&mut app, bar, MouseScrollUnit::Line, -1.0);
        assert_eq!(scrolled(&app, list), 20.0);
    }

    /// When the content shrinks, the position comes back in range.
    #[test]
    fn the_position_is_clamped_when_the_content_shrinks() {
        let (mut app, list) = app();
        turn(&mut app, list, MouseScrollUnit::Line, -10.0);
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
    fn the_scrollbar_is_hidden_while_everything_fits() {
        let (mut app, list) = app();
        let bar = app
            .world_mut()
            .spawn((
                Scrollbar::new(list, ControlOrientation::Vertical, 10.0),
                Visibility::Hidden,
            ))
            .id();
        let display = |app: &App| *app.world().get::<Visibility>(bar).unwrap();
        app.update();
        assert_eq!(display(&app), Visibility::Inherited);
        app.world_mut()
            .get_mut::<ComputedNode>(list)
            .unwrap()
            .content_size
            .y = 80.0;
        app.update();
        assert_eq!(display(&app), Visibility::Hidden);
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
