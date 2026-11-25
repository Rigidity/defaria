//! Scrolling: the mouse wheel over a [`Scrollable`] node scrolls it, and a
//! [`ScrollbarThumb`] shows how far.
//!
//! A scrollable node needs `overflow: Overflow::scroll_y()` in its [`Node`];
//! [`scroll_panel`] gives it that. Only vertical scrolling for now.

use bevy::{
    input::mouse::{AccumulatedMouseScroll, MouseScrollUnit},
    prelude::*,
    ui::{CalculatedClip, UiGlobalTransform, UiSystems},
};

use super::{UiTheme, pointer::UiPointer};

pub struct ScrollPlugin;

impl Plugin for ScrollPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Scrollable>()
            .add_systems(Update, scroll_with_wheel)
            .add_systems(
                PostUpdate,
                (keep_scroll_in_range, move_thumbs)
                    .chain()
                    .after(UiSystems::Layout),
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

/// The thumb of a scrollbar: sized and placed within its parent (the track)
/// to show which part of `target`'s content is in view. The track is
/// hidden when it all fits.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct ScrollbarThumb {
    pub target: Entity,
}

/// A scrollable node laid out as `node` (its `height` is the view's), with
/// a scrollbar beside it, spawned under `commands`. `line` is how far a
/// notch of the wheel scrolls. Spawn the content as children of the
/// scrolled node. Returns (the row holding both, the scrolled node).
pub fn scroll_panel(
    commands: &mut ChildSpawnerCommands,
    theme: &UiTheme,
    node: Node,
    line: f32,
    extra: impl Bundle,
) -> (Entity, Entity) {
    let height = node.height;
    let mut row = commands.spawn(Node {
        column_gap: px(6),
        ..default()
    });
    let row_id = row.id();
    let mut scrolled = Entity::PLACEHOLDER;
    row.with_children(|row| {
        scrolled = row
            .spawn((
                Scrollable { line },
                Node {
                    overflow: Overflow::scroll_y(),
                    ..node
                },
                extra,
            ))
            .id();
        row.spawn((
            Name::new("Scrollbar"),
            Node {
                width: px(6),
                height,
                ..default()
            },
            BackgroundColor(theme.scrollbar_track),
            children![(
                ScrollbarThumb { target: scrolled },
                Node {
                    position_type: PositionType::Absolute,
                    width: percent(100),
                    ..default()
                },
                BackgroundColor(theme.scrollbar_thumb),
            )],
        ));
    });
    (row_id, scrolled)
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
    mut nodes: Query<(
        &Scrollable,
        &mut ScrollPosition,
        &ComputedNode,
        &UiGlobalTransform,
        Option<&CalculatedClip>,
        &bevy::ui::ComputedStackIndex,
    )>,
) {
    if wheel.delta.y == 0.0 {
        return;
    }
    let Some((scrollable, mut position, node, ..)) = nodes
        .iter_mut()
        .filter(|(_, _, node, transform, clip, _)| pointer.is_over(node, transform, *clip))
        .max_by_key(|(.., stack)| stack.0)
    else {
        return;
    };
    let pixels = match wheel.unit {
        MouseScrollUnit::Line => wheel.delta.y * scrollable.line,
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

fn move_thumbs(
    mut thumbs: Query<(&ScrollbarThumb, &ChildOf, &mut Node)>,
    mut tracks: Query<&mut Visibility>,
    targets: Query<(&ScrollPosition, &ComputedNode)>,
) {
    for (thumb, track, mut style) in &mut thumbs {
        let Ok((position, node)) = targets.get(thumb.target) else {
            continue;
        };
        let Ok(mut visibility) = tracks.get_mut(track.parent()) else {
            continue;
        };
        let (view, content) = (node.size.y, node.content_size.y);
        let fits = content <= view + 0.5 || view <= 0.0;
        let wanted = if fits {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        visibility.set_if_neq(wanted);
        if fits {
            continue;
        }
        let scrolled = position.y / node.inverse_scale_factor;
        let (top, height) = (
            percent(100.0 * scrolled / content),
            percent(100.0 * view / content),
        );
        if style.top != top || style.height != height {
            style.top = top;
            style.height = height;
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
    fn content_that_fits_doesnt_scroll() {
        let node = ComputedNode {
            size: Vec2::splat(100.0),
            content_size: Vec2::splat(80.0),
            ..default()
        };
        assert_eq!(max_scroll(&node), 0.0);
    }
}
