//! The hotbar: nine item slots along the bottom of the screen, one of them
//! active. Right click places the active slot's block (see `block_edit`).
//!
//! - The number keys 1–9 pick the active slot, in game or in a screen.
//! - The mouse wheel steps through the slots while playing: away from you
//!   is the slot to the left, toward you the one to the right, wrapping.
//! - Switching slots shows the item's name above the hotbar for a moment.
//! - With a screen open, clicking a slot makes it active and right clicking
//!   empties it. The inventory puts items in the active slot.
//!
//! It starts out with [`DEFAULT_HOTBAR`]. It isn't saved yet.

use bevy::{
    input::mouse::{AccumulatedMouseScroll, MouseScrollUnit},
    prelude::*,
    window::PrimaryWindow,
};

use crate::{
    content::{BlockState, Content, ItemState},
    ui::{Clicked, ItemSlot, UiTheme, item_slot, playing},
};

pub const HOTBAR_SLOTS: usize = 9;

/// What the hotbar starts with, slot 1 first.
pub const DEFAULT_HOTBAR: [&str; HOTBAR_SLOTS] = [
    "defaria:stone",
    "defaria:dirt",
    "defaria:grass",
    "defaria:sand",
    "defaria:stone_slab",
    "defaria:log",
    "defaria:planks",
    "defaria:lamp",
    "defaria:red_lamp",
];

pub const HOTBAR_KEYS: [KeyCode; HOTBAR_SLOTS] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

/// Wheel movement in logical pixels (from a touchpad) that counts as one
/// notch.
const PIXELS_PER_STEP: f32 = 40.0;
/// How long the item name shows after switching, in seconds, and how much
/// of the end of that it spends fading out.
const NAME_TIME: f32 = 2.0;
const NAME_FADE: f32 = 0.5;

pub struct HotbarPlugin;

impl Plugin for HotbarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Hotbar>()
            .init_resource::<ActiveSlot>()
            .register_type::<Hotbar>()
            .register_type::<ActiveSlot>()
            .add_observer(click_slot)
            .add_systems(Startup, spawn_hotbar)
            .add_systems(
                Update,
                (
                    (select_with_keys, cycle_with_wheel.run_if(playing)).in_set(HotbarInput),
                    (show_hotbar, show_active_name).after(HotbarInput),
                ),
            );
    }
}

/// The systems that change the active slot.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct HotbarInput;

/// The item in each slot, slot 1 first.
#[derive(Resource, Reflect, Debug, Clone, PartialEq, Eq)]
#[reflect(Resource)]
pub struct Hotbar(pub [Option<ItemState>; HOTBAR_SLOTS]);

impl FromWorld for Hotbar {
    /// [`DEFAULT_HOTBAR`]. An item that didn't load is reported and leaves
    /// its slot empty.
    fn from_world(world: &mut World) -> Self {
        let content = world.resource::<Content>();
        Self(DEFAULT_HOTBAR.map(|id| {
            let item = content.default_item(id);
            if item.is_none() {
                error!("the hotbar starts with item `{id}`, which didn't load");
            }
            item
        }))
    }
}

/// The slot whose item right click uses, from 0.
#[derive(Resource, Reflect, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[reflect(Resource)]
pub struct ActiveSlot(pub usize);

impl Hotbar {
    /// The item in the active slot.
    pub fn active(&self, slot: ActiveSlot) -> Option<ItemState> {
        self.0.get(slot.0).copied().flatten()
    }

    /// The block the active slot's item places, if it places one.
    pub fn active_block(&self, slot: ActiveSlot, content: &Content) -> Option<BlockState> {
        content.item_state(self.active(slot)?)?.block
    }
}

pub fn select_with_keys(keys: Res<ButtonInput<KeyCode>>, mut active: ResMut<ActiveSlot>) {
    if let Some(index) = HOTBAR_KEYS.iter().position(|&key| keys.just_pressed(key)) {
        active.set_if_neq(ActiveSlot(index));
    }
}

/// The slot `steps` along from `slot`, wrapping around the ends.
pub fn step_slot(slot: usize, steps: i32) -> usize {
    (slot as i32 + steps).rem_euclid(HOTBAR_SLOTS as i32) as usize
}

fn cycle_with_wheel(
    wheel: Res<AccumulatedMouseScroll>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    mut active: ResMut<ActiveSlot>,
    // Wheel movement not yet a whole step, in steps.
    mut partial: Local<f32>,
) {
    if wheel.delta.y == 0.0 {
        return;
    }
    let notches = match wheel.unit {
        MouseScrollUnit::Line => wheel.delta.y,
        // Touchpads report physical pixels.
        MouseScrollUnit::Pixel => {
            let scale = window.map_or(1.0, |window| window.scale_factor());
            wheel.delta.y / scale / PIXELS_PER_STEP
        }
    };
    *partial += notches;
    let whole = partial.trunc();
    *partial -= whole;
    if whole != 0.0 {
        // Turning away from you (positive) moves left.
        active.0 = step_slot(active.0, -whole as i32);
    }
}

/// One slot of the hotbar, showing [`Hotbar`]`[index]`.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct HotbarSlot(pub usize);

/// The item name shown above the hotbar after switching, and the seconds
/// left before it's gone.
#[derive(Component, Debug, Clone, Copy, Default)]
struct ActiveItemName(f32);

/// From the bottom of the window to the hotbar, and from the bar to the
/// item name above it, in logical pixels.
const BOTTOM: f32 = 12.0;
const NAME_GAP: f32 = 8.0;

/// How much of the bottom of the window the hotbar and its item name take
/// up, in logical pixels, so screens can leave it uncovered.
pub fn hotbar_height(theme: &UiTheme) -> f32 {
    // A line of text is about 1.2 times its font size.
    let name = theme.title_font_size * 1.2;
    BOTTOM + theme.slot_size + 2.0 * theme.slot_gap + NAME_GAP + name
}

fn spawn_hotbar(mut commands: Commands, theme: Res<UiTheme>) {
    commands
        .spawn((
            Name::new("Hotbar"),
            Node {
                position_type: PositionType::Absolute,
                bottom: px(BOTTOM),
                width: percent(100),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(NAME_GAP),
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn((
                ActiveItemName::default(),
                Text::default(),
                TextFont::from_font_size(theme.title_font_size),
                TextColor(Color::NONE),
                name_shadow(0.0),
            ));
            root.spawn((
                Node {
                    column_gap: px(theme.slot_gap),
                    padding: UiRect::all(px(theme.slot_gap)),
                    ..default()
                },
                BackgroundColor(theme.backdrop),
            ))
            .with_children(|bar| {
                for index in 0..HOTBAR_SLOTS {
                    bar.spawn((HotbarSlot(index), item_slot(&theme, ItemSlot::default())));
                }
            });
        });
}

/// Left click makes the slot active; right click empties it.
fn click_slot(
    click: On<Clicked>,
    slots: Query<&HotbarSlot>,
    mut hotbar: ResMut<Hotbar>,
    mut active: ResMut<ActiveSlot>,
) {
    let Ok(&HotbarSlot(index)) = slots.get(click.entity) else {
        return;
    };
    if click.button == MouseButton::Left {
        active.set_if_neq(ActiveSlot(index));
    } else if click.button == MouseButton::Right {
        hotbar.0[index] = None;
    }
}

fn show_hotbar(
    hotbar: Res<Hotbar>,
    active: Res<ActiveSlot>,
    mut slots: Query<(&HotbarSlot, &mut ItemSlot)>,
) {
    if !hotbar.is_changed() && !active.is_changed() {
        return;
    }
    for (&HotbarSlot(index), mut slot) in &mut slots {
        slot.set_if_neq(ItemSlot {
            item: hotbar.0[index],
            selected: index == active.0,
        });
    }
}

/// Shows the active item's name when the slot or its item changes, then
/// fades it out.
fn show_active_name(
    time: Res<Time>,
    hotbar: Res<Hotbar>,
    active: Res<ActiveSlot>,
    content: Res<Content>,
    theme: Res<UiTheme>,
    mut names: Query<(
        &mut ActiveItemName,
        &mut Text,
        &mut TextColor,
        &mut TextShadow,
    )>,
) {
    // Not for the starting slot, only when it changes after.
    let switched = (active.is_changed() || hotbar.is_changed()) && !active.is_added();
    for (mut left, mut text, mut color, mut shadow) in &mut names {
        if switched {
            let name = hotbar
                .active(*active)
                .and_then(|item| content.item_name(item));
            text.0 = name.unwrap_or_default().to_owned();
            left.0 = NAME_TIME;
        } else if left.0 > 0.0 {
            left.0 = (left.0 - time.delta_secs()).max(0.0);
        } else {
            continue;
        }
        let alpha = (left.0 / NAME_FADE).min(1.0);
        color.0 = theme.text.with_alpha(alpha);
        // The shadow fades with the text, or it would stay behind.
        *shadow = name_shadow(alpha);
    }
}

/// The name's shadow, as visible as the text at `alpha`.
fn name_shadow(alpha: f32) -> TextShadow {
    let shadow = TextShadow::default();
    TextShadow {
        color: shadow.color.with_alpha(shadow.color.alpha() * alpha),
        ..shadow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::base;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(base().clone())
            .init_resource::<UiTheme>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<AccumulatedMouseScroll>()
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .add_plugins(HotbarPlugin);
        app.update();
        app
    }

    fn active(app: &App) -> usize {
        app.world().resource::<ActiveSlot>().0
    }

    fn turn(app: &mut App, unit: MouseScrollUnit, y: f32) {
        app.insert_resource(AccumulatedMouseScroll {
            unit,
            delta: Vec2::new(0.0, y),
        });
        app.update();
        app.insert_resource(AccumulatedMouseScroll::default());
    }

    #[test]
    fn starts_with_the_default_items() {
        let app = app();
        let hotbar = app.world().resource::<Hotbar>();
        let ids: Vec<&str> = hotbar
            .0
            .iter()
            .map(|item| base().item_of(item.unwrap()).unwrap().id.as_str())
            .collect();
        assert_eq!(ids, DEFAULT_HOTBAR);
    }

    /// Items that didn't load leave their slots empty.
    #[test]
    fn missing_default_items_leave_their_slots_empty() {
        let mut world = World::new();
        world.insert_resource(Content::from_mods(Vec::new()));
        let hotbar = Hotbar::from_world(&mut world);
        assert_eq!(hotbar.0, [None; HOTBAR_SLOTS]);
    }

    #[test]
    fn number_keys_pick_the_slot() {
        let mut app = app();
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::Digit7);
        app.update();
        assert_eq!(active(&app), 6);
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release_all();
        keys.clear();
        keys.press(KeyCode::Digit1);
        app.update();
        assert_eq!(active(&app), 0);
    }

    #[test]
    fn the_wheel_steps_through_the_slots_and_wraps() {
        let mut app = app();
        turn(&mut app, MouseScrollUnit::Line, -1.0);
        assert_eq!(active(&app), 1);
        turn(&mut app, MouseScrollUnit::Line, 3.0);
        assert_eq!(active(&app), 7);
        // A touchpad's pixels add up to whole steps.
        for _ in 0..3 {
            turn(&mut app, MouseScrollUnit::Pixel, -15.0);
        }
        assert_eq!(active(&app), 8);
        turn(&mut app, MouseScrollUnit::Line, -1.0);
        assert_eq!(active(&app), 0);
    }

    #[test]
    fn the_slots_show_the_hotbar_and_the_active_slot() {
        let mut app = app();
        let dirt = base().default_item("defaria:dirt");
        app.world_mut().resource_mut::<Hotbar>().0[4] = dirt;
        app.world_mut().resource_mut::<ActiveSlot>().0 = 4;
        app.update();
        let mut slots = app.world_mut().query::<(&HotbarSlot, &ItemSlot)>();
        let shown: Vec<_> = slots
            .iter(app.world())
            .filter(|(slot, _)| slot.0 == 4 || slot.0 == 0)
            .map(|(slot, item)| (slot.0, *item))
            .collect();
        assert!(shown.contains(&(
            4,
            ItemSlot {
                item: dirt,
                selected: true
            }
        )));
        assert!(shown.contains(&(
            0,
            ItemSlot {
                item: base().default_item("defaria:stone"),
                selected: false
            }
        )));
    }

    #[test]
    fn switching_shows_the_item_name_then_fades_it() {
        let mut app = app();
        app.world_mut().resource_mut::<ActiveSlot>().0 = 5;
        app.update();
        let mut names = app
            .world_mut()
            .query::<(&Text, &TextColor, &TextShadow, &ActiveItemName)>();
        let (text, color, shadow, _) = names.single(app.world()).unwrap();
        assert_eq!(text.0, "Oak Log");
        assert_eq!(color.0.alpha(), 1.0);
        assert_eq!(shadow.color, TextShadow::default().color);
        // 0.1 s a frame: 1.6 s on, 0.4 s are left, partway through the fade.
        for _ in 0..16 {
            app.update();
        }
        let (_, color, shadow, _) = names.single(app.world()).unwrap();
        let alpha = 0.4 / NAME_FADE;
        assert!(
            (color.0.alpha() - alpha).abs() < 1e-3,
            "{}",
            color.0.alpha()
        );
        let full = TextShadow::default().color.alpha();
        assert!(
            (shadow.color.alpha() - full * alpha).abs() < 1e-3,
            "{}",
            shadow.color.alpha()
        );
        for _ in 0..5 {
            app.update();
        }
        // Gone, shadow and all.
        let (_, color, shadow, _) = names.single(app.world()).unwrap();
        assert_eq!(color.0.alpha(), 0.0);
        assert_eq!(shadow.color.alpha(), 0.0);
    }

    /// The starting slot's name doesn't show at startup.
    #[test]
    fn no_name_shows_before_switching() {
        let mut app = app();
        app.update();
        let mut names = app
            .world_mut()
            .query::<(&Text, &TextColor, &TextShadow, &ActiveItemName)>();
        let (text, color, shadow, _) = names.single(app.world()).unwrap();
        assert_eq!((text.0.as_str(), color.0.alpha()), ("", 0.0));
        assert_eq!(shadow.color.alpha(), 0.0);
    }

    /// Touchpad pixels are physical: on a window at scale 2, a step takes
    /// twice as many of them.
    #[test]
    fn touchpad_steps_are_in_logical_pixels() {
        let mut app = app();
        let mut resolution = bevy::window::WindowResolution::default();
        resolution.set_scale_factor_override(Some(2.0));
        app.world_mut().spawn((
            Window {
                resolution,
                ..default()
            },
            PrimaryWindow,
        ));
        turn(&mut app, MouseScrollUnit::Pixel, -PIXELS_PER_STEP);
        assert_eq!(active(&app), 0);
        turn(&mut app, MouseScrollUnit::Pixel, -PIXELS_PER_STEP);
        assert_eq!(active(&app), 1);
    }

    #[test]
    fn step_slot_wraps_both_ways() {
        assert_eq!(step_slot(0, -1), 8);
        assert_eq!(step_slot(8, 1), 0);
        assert_eq!(step_slot(3, 20), 5);
    }
}
