//! The mouse pointer over the UI: which widget it's over, and clicks on it.
//!
//! This does its own hit testing against the laid-out nodes rather than
//! going through `bevy_picking`, whose mouse pointer only ever targets the
//! window: headless runs draw into an image, where picking would never hit
//! anything. Testing the window's cursor against each node's rectangle
//! works for both, and keeps UI input deterministic.
//!
//! - Nodes with [`Hoverable`] can be hovered and clicked. The one on top
//!   under the cursor, outside any clipping (a scrolled-away slot isn't
//!   there), gets [`Hovered`] and is in [`UiPointer::hovered`].
//! - A mouse button pressed over it triggers [`Clicked`] on it, so widgets
//!   handle their clicks with an observer.
//! - The pointer only tracks while a screen is open: while playing, the
//!   cursor is locked and the mouse belongs to the game.

use bevy::{
    input::InputSystems,
    prelude::*,
    ui::{CalculatedClip, ComputedStackIndex, UiGlobalTransform},
    window::PrimaryWindow,
};

use super::screen::{UiScreen, playing};

pub struct PointerPlugin;

impl Plugin for PointerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiPointer>()
            .register_type::<Hoverable>()
            .register_type::<Hovered>()
            .add_systems(
                PreUpdate,
                (track_pointer, click)
                    .chain()
                    .in_set(UiPointerSystems)
                    .after(InputSystems),
            );
    }
}

/// Finds what the pointer is over and sends clicks, in `PreUpdate` after
/// input, so `Update` sees this frame's hover.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct UiPointerSystems;

/// Where the pointer is and what it's over.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct UiPointer {
    /// In physical pixels of the window, the same space as the UI's
    /// layout. `None` while playing or when the cursor is outside.
    pub position: Option<Vec2>,
    /// The [`Hoverable`] node on top under the pointer.
    pub hovered: Option<Entity>,
}

impl UiPointer {
    /// The visible node on top under the pointer, of those `nodes` (a
    /// query of [`HitTestData`]) holds.
    pub fn topmost<F: bevy::ecs::query::QueryFilter>(
        &self,
        nodes: &Query<HitTestData, F>,
    ) -> Option<Entity> {
        nodes
            .iter()
            .filter(|(_, node, transform, visible, clip, _)| {
                visible.get() && self.is_over(node, transform, *clip)
            })
            .max_by_key(|(.., stack)| stack.0)
            .map(|(entity, ..)| entity)
    }

    /// Whether the pointer is over `node`, inside whatever clips it.
    pub fn is_over(
        &self,
        node: &ComputedNode,
        transform: &UiGlobalTransform,
        clip: Option<&CalculatedClip>,
    ) -> bool {
        self.position.is_some_and(|point| {
            node.contains_point(*transform, point)
                && clip.is_none_or(|clip| clip.contains_point(point))
        })
    }
}

/// A node the pointer can hover and click.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct Hoverable;

/// On the [`Hoverable`] node the pointer is over, while it is.
#[derive(Component, Reflect, Debug, Clone, Copy, Default)]
#[reflect(Component)]
pub struct Hovered;

/// A mouse button went down over a [`Hoverable`] node.
#[derive(EntityEvent, Debug, Clone, Copy)]
pub struct Clicked {
    pub entity: Entity,
    pub button: MouseButton,
}

/// What hit testing reads from a node: [`UiPointer::topmost`] takes these.
pub type HitTestData = (
    Entity,
    &'static ComputedNode,
    &'static UiGlobalTransform,
    &'static InheritedVisibility,
    Option<&'static CalculatedClip>,
    &'static ComputedStackIndex,
);

fn track_pointer(
    mut commands: Commands,
    mut pointer: ResMut<UiPointer>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    screens: Query<(), With<UiScreen>>,
    nodes: Query<HitTestData, With<Hoverable>>,
    hovered: Query<Entity, With<Hovered>>,
) {
    let position = if playing(screens) {
        None
    } else {
        window.and_then(|window| window.physical_cursor_position())
    };
    let mut next = UiPointer {
        position,
        hovered: None,
    };
    next.hovered = next.topmost(&nodes);
    for entity in &hovered {
        if Some(entity) != next.hovered {
            commands.entity(entity).remove::<Hovered>();
        }
    }
    if let Some(entity) = next.hovered
        && !hovered.contains(entity)
    {
        commands.entity(entity).insert(Hovered);
    }
    pointer.set_if_neq(next);
}

fn click(mut commands: Commands, pointer: Res<UiPointer>, mouse: Res<ButtonInput<MouseButton>>) {
    let Some(entity) = pointer.hovered else {
        return;
    };
    for &button in mouse.get_just_pressed() {
        commands.trigger(Clicked { entity, button });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PointerPlugin))
            .init_resource::<ButtonInput<MouseButton>>();
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app
    }

    /// A hoverable node `size` square centered at `center`, at `stack`.
    pub fn node(app: &mut App, center: Vec2, size: f32, stack: u32) -> Entity {
        app.world_mut()
            .spawn((
                Hoverable,
                ComputedNode {
                    size: Vec2::splat(size),
                    ..default()
                },
                UiGlobalTransform::from_translation(center),
                InheritedVisibility::VISIBLE,
                ComputedStackIndex(stack),
            ))
            .id()
    }

    pub fn move_to(app: &mut App, position: Vec2) {
        let mut windows = app.world_mut().query::<&mut Window>();
        windows
            .single_mut(app.world_mut())
            .unwrap()
            .set_physical_cursor_position(Some(position.as_dvec2()));
    }

    pub fn open_screen(app: &mut App) {
        app.world_mut().spawn(UiScreen);
    }

    #[derive(Resource, Default)]
    struct Clicks(Vec<(Entity, MouseButton)>);

    #[test]
    fn the_top_node_under_the_pointer_is_hovered_and_clicked() {
        let mut app = app();
        open_screen(&mut app);
        let below = node(&mut app, Vec2::new(50.0, 50.0), 40.0, 1);
        let above = node(&mut app, Vec2::new(60.0, 50.0), 40.0, 2);
        app.init_resource::<Clicks>().add_observer(
            |click: On<Clicked>, mut clicks: ResMut<Clicks>| {
                clicks.0.push((click.entity, click.button));
            },
        );

        move_to(&mut app, Vec2::new(35.0, 50.0));
        app.update();
        assert_eq!(app.world().resource::<UiPointer>().hovered, Some(below));
        assert!(app.world().get::<Hovered>(below).is_some());

        // Where they overlap, the one on top wins.
        move_to(&mut app, Vec2::new(55.0, 50.0));
        app.update();
        assert_eq!(app.world().resource::<UiPointer>().hovered, Some(above));
        assert!(app.world().get::<Hovered>(below).is_none());

        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        app.update();
        assert_eq!(
            app.world().resource::<Clicks>().0,
            [(above, MouseButton::Right)]
        );

        // Off both: nothing.
        move_to(&mut app, Vec2::new(500.0, 50.0));
        app.update();
        assert_eq!(app.world().resource::<UiPointer>().hovered, None);
        assert!(app.world().get::<Hovered>(above).is_none());
    }

    #[test]
    fn nothing_is_hovered_while_playing() {
        let mut app = app();
        node(&mut app, Vec2::new(50.0, 50.0), 40.0, 1);
        move_to(&mut app, Vec2::new(50.0, 50.0));
        app.update();
        assert_eq!(*app.world().resource::<UiPointer>(), UiPointer::default());
    }

    #[test]
    fn clipped_and_hidden_nodes_arent_hovered() {
        let mut app = app();
        open_screen(&mut app);
        let clipped = node(&mut app, Vec2::new(50.0, 50.0), 40.0, 1);
        app.world_mut()
            .entity_mut(clipped)
            .insert(CalculatedClip::FullyClipped);
        let hidden = node(&mut app, Vec2::new(150.0, 50.0), 40.0, 1);
        app.world_mut()
            .entity_mut(hidden)
            .insert(InheritedVisibility::HIDDEN);
        for x in [50.0, 150.0] {
            move_to(&mut app, Vec2::new(x, 50.0));
            app.update();
            assert_eq!(app.world().resource::<UiPointer>().hovered, None);
        }
    }

    /// A node partly scrolled out of its parent is clipped by a rectangle:
    /// only the part inside it can be hovered.
    #[test]
    fn only_the_unclipped_part_of_a_node_is_hovered() {
        let mut app = app();
        open_screen(&mut app);
        // The node spans 30..70 both ways; the clip keeps y 30..50.
        let node = node(&mut app, Vec2::new(50.0, 50.0), 40.0, 1);
        let clip = CalculatedClip::default().with_rect(
            Rect::new(30.0, 30.0, 70.0, 50.0),
            &UiGlobalTransform::default(),
        );
        app.world_mut().entity_mut(node).insert(clip);
        move_to(&mut app, Vec2::new(50.0, 40.0));
        app.update();
        assert_eq!(app.world().resource::<UiPointer>().hovered, Some(node));
        move_to(&mut app, Vec2::new(50.0, 60.0));
        app.update();
        assert_eq!(app.world().resource::<UiPointer>().hovered, None);
    }
}
