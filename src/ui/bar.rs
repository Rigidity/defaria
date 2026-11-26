//! Bars: a framed track filled part of the way, for how full something is
//! (a machine's tank or energy buffer) or how far along (a recipe).
//! Vertical bars fill from the bottom, horizontal ones from the left.
//!
//! Whoever spawns one sets its [`Bar`] as what it shows changes; the fill
//! follows. Give it a [`Tooltip`](super::Tooltip) to say the numbers.

use bevy::prelude::*;

use super::{UiTheme, UiWidgetSystems};

pub struct BarPlugin;

impl Plugin for BarPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Bar>()
            .add_systems(PostUpdate, fill_bars.in_set(UiWidgetSystems));
    }
}

/// How full a bar is, and in what color.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq)]
#[reflect(Component)]
pub struct Bar {
    /// From 0 (empty) to 1 (full); anything outside is clamped.
    pub fraction: f32,
    pub color: Color,
    /// Filling upward rather than rightward.
    pub vertical: bool,
}

/// The filled part of a [`Bar`].
#[derive(Component, Debug, Clone, Copy)]
struct BarFill;

/// A bar `width` by `height` logical pixels, border included, ready to
/// spawn.
pub fn bar(theme: &UiTheme, bar: Bar, width: f32, height: f32) -> impl Bundle {
    (
        bar,
        Node {
            width: px(width),
            height: px(height),
            border: UiRect::all(px(theme.slot_border)),
            ..default()
        },
        BackgroundColor(theme.slot),
        BorderColor::all(theme.slot_edge),
        children![(
            BarFill,
            fill_node(&bar),
            BackgroundColor(bar.color),
            // Picking (and so the tooltip) finds the bar itself.
            Pickable::IGNORE,
        )],
    )
}

/// The fill's node for `bar`: anchored at the bottom or the left.
fn fill_node(bar: &Bar) -> Node {
    let filled = percent(bar.fraction.clamp(0.0, 1.0) * 100.0);
    let mut node = Node {
        position_type: PositionType::Absolute,
        left: px(0),
        bottom: px(0),
        width: percent(100),
        height: percent(100),
        ..default()
    };
    if bar.vertical {
        node.height = filled;
    } else {
        node.width = filled;
    }
    node
}

fn fill_bars(
    bars: Query<(&Bar, &Children), Changed<Bar>>,
    mut fills: Query<(&mut Node, &mut BackgroundColor), With<BarFill>>,
) {
    for (bar, children) in &bars {
        for &child in children {
            if let Ok((mut node, mut color)) = fills.get_mut(child) {
                let wanted = fill_node(bar);
                if *node != wanted {
                    *node = wanted;
                }
                color.set_if_neq(BackgroundColor(bar.color));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bar_fills_as_far_as_its_fraction() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<UiTheme>()
            .add_plugins(BarPlugin);
        let theme = UiTheme::default();
        let red = Color::srgb(1.0, 0.0, 0.0);
        let shown = Bar {
            fraction: 0.25,
            color: red,
            vertical: true,
        };
        let entity = app.world_mut().spawn(bar(&theme, shown, 20.0, 100.0)).id();
        app.update();
        let fill = app.world().get::<Children>(entity).unwrap()[0];
        let node = |app: &App| app.world().get::<Node>(fill).unwrap().clone();
        assert_eq!(
            (node(&app).width, node(&app).height),
            (percent(100), percent(25))
        );

        // Over full is full, and a horizontal bar fills its width.
        let blue = Color::srgb(0.0, 0.0, 1.0);
        *app.world_mut().get_mut::<Bar>(entity).unwrap() = Bar {
            fraction: 1.5,
            color: blue,
            vertical: false,
        };
        app.update();
        assert_eq!(
            (node(&app).width, node(&app).height),
            (percent(100), percent(100))
        );
        app.world_mut().get_mut::<Bar>(entity).unwrap().fraction = 0.5;
        app.update();
        assert_eq!(node(&app).width, percent(50));
        assert_eq!(app.world().get::<BackgroundColor>(fill).unwrap().0, blue);
    }
}
