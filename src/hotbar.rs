//! The hotbar: [`HOTBAR_SLOTS`] quick picks along the bottom of the
//! screen, one of them active. Right click places the active slot's block
//! (see `block_edit`).
//!
//! A slot doesn't hold items: it names a kind of item, and shows how many
//! of it the player's [`Inventory`] has. Running out leaves the slot as it
//! was, at 0 and dimmed, ready for when more turn up (as in Factorio).
//!
//! - The number keys 1–9 pick the active slot, in game or in a screen
//!   (unless a text field has the keyboard).
//! - The mouse wheel steps through the slots while playing: away from you
//!   is the slot to the left, toward you the one to the right, wrapping.
//! - Switching slots shows the item's name above the hotbar for a moment.
//! - With a screen open, clicking a slot makes it active and right clicking
//!   clears it. The inventory screen puts items in slots.
//!
//! The slots ([`Hotbar`]) and [`ActiveSlot`] are components of the player
//! (the entity with [`LocalPlayer`]), so every player or mob has its own;
//! the hotbar shows the local player's.

use bevy::{
    input::mouse::{AccumulatedMouseScroll, MouseScrollUnit},
    picking::{events::PointerPress, pointer::PointerButton},
    prelude::*,
    window::PrimaryWindow,
};

use crate::{
    content::{BlockState, Content, ItemState},
    inventory::Inventory,
    player::LocalPlayer,
    ui::{ItemSlot, UiTheme, item_slot, playing, typing},
};

pub const HOTBAR_SLOTS: usize = 9;

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
        app.register_type::<Hotbar>()
            .register_type::<ActiveSlot>()
            .add_observer(click_slot)
            .add_systems(Startup, spawn_hotbar)
            .add_systems(
                Update,
                (
                    (
                        select_with_keys.run_if(not(typing)),
                        cycle_with_wheel.run_if(playing),
                    )
                        .in_set(HotbarInput),
                    (show_hotbar, show_active_name).after(HotbarInput),
                ),
            );
    }
}

/// The systems that change the active slot.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct HotbarInput;

/// The kind of item in each hotbar slot, slot 1 first.
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq, Eq)]
#[reflect(Component)]
pub struct Hotbar(pub [Option<ItemState>; HOTBAR_SLOTS]);

impl Hotbar {
    /// The item the active slot names.
    pub fn item(&self, active: ActiveSlot) -> Option<ItemState> {
        self.0.get(active.0).copied().flatten()
    }

    /// The block the active slot's item places, if it places one.
    pub fn block(&self, active: ActiveSlot, content: &Content) -> Option<BlockState> {
        content.item_state(self.item(active)?)?.block
    }

    /// Puts `item` in `slot`, taking it out of any other slot it was in,
    /// so each kind is in one slot at most.
    pub fn assign(&mut self, slot: usize, item: Option<ItemState>) {
        if let Some(item) = item {
            for other in &mut self.0 {
                if *other == Some(item) {
                    *other = None;
                }
            }
        }
        if let Some(there) = self.0.get_mut(slot) {
            *there = item;
        }
    }
}

/// The hotbar slot in hand, from 0: whose item right click uses.
#[derive(Component, Reflect, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[reflect(Component)]
pub struct ActiveSlot(pub usize);

pub fn select_with_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut active: Option<Single<&mut ActiveSlot, With<LocalPlayer>>>,
) {
    if let Some(index) = HOTBAR_KEYS.iter().position(|&key| keys.just_pressed(key))
        && let Some(active) = &mut active
    {
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
    active: Option<Single<&mut ActiveSlot, With<LocalPlayer>>>,
    // Wheel movement not yet a whole step, in steps.
    mut partial: Local<f32>,
) {
    let Some(mut active) = active else {
        return;
    };
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

/// One slot of the hotbar, showing the local player's inventory slot
/// `index`.
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
                theme.title_font(),
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

/// Left click makes the slot active; right click clears it.
fn click_slot(
    click: On<PointerPress>,
    slots: Query<&HotbarSlot>,
    player: Option<Single<(&mut Hotbar, &mut ActiveSlot), With<LocalPlayer>>>,
) {
    let (Ok(&HotbarSlot(index)), Some(player)) = (slots.get(click.entity), player) else {
        return;
    };
    let (mut hotbar, mut active) = player.into_inner();
    if click.button == PointerButton::Primary {
        active.set_if_neq(ActiveSlot(index));
    } else if click.button == PointerButton::Secondary {
        hotbar.assign(index, None);
    }
}

/// Each slot's item, how many the player has (dimmed at none), and which
/// is active.
fn show_hotbar(
    player: Option<Single<(Ref<Hotbar>, Ref<Inventory>, Ref<ActiveSlot>), With<LocalPlayer>>>,
    mut slots: Query<(&HotbarSlot, &mut ItemSlot)>,
) {
    let Some(player) = player else {
        return;
    };
    let (hotbar, inventory, active) = player.into_inner();
    let changed = hotbar.is_changed() || inventory.is_changed() || active.is_changed();
    for (&HotbarSlot(index), mut slot) in &mut slots {
        if !changed && !slot.is_added() {
            continue;
        }
        let item = hotbar.0[index];
        let count = item.map_or(0, |item| inventory.count(item));
        slot.set_if_neq(ItemSlot {
            item,
            count,
            selected: index == active.0,
            dimmed: item.is_some() && count == 0,
        });
    }
}

/// Shows the active item's name when the slot or its item changes, then
/// fades it out.
fn show_active_name(
    time: Res<Time>,
    player: Option<Single<(Ref<Hotbar>, Ref<ActiveSlot>), With<LocalPlayer>>>,
    content: Res<Content>,
    theme: Res<UiTheme>,
    mut names: Query<(
        &mut ActiveItemName,
        &mut Text,
        &mut TextColor,
        &mut TextShadow,
    )>,
    // The active slot's item last frame, so the name shows again when the
    // item changes as well as the slot.
    mut last_item: Local<Option<Option<ItemState>>>,
) {
    let Some(player) = player else {
        return;
    };
    let (hotbar, active) = player.into_inner();
    // Not for the starting slot, only when the slot or its item changes
    // after.
    let item = hotbar.item(*active);
    let switched =
        (active.is_changed() || last_item.is_some_and(|last| last != item)) && !active.is_added();
    *last_item = Some(item);
    for (mut left, mut text, mut color, mut shadow) in &mut names {
        if switched {
            let name = item.and_then(|item| content.item_name(item));
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

/// A shadow one pixel of the pixel font down and right.
fn text_shadow() -> TextShadow {
    TextShadow {
        offset: Vec2::splat(1.0),
        ..default()
    }
}

/// The name's shadow, as visible as the text at `alpha`.
fn name_shadow(alpha: f32) -> TextShadow {
    let shadow = text_shadow();
    TextShadow {
        color: shadow.color.with_alpha(shadow.color.alpha() * alpha),
        ..shadow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        content::base,
        inventory::{ItemStack, player::new_player},
    };
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    /// The hotbar, with a local player who's a new player.
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
        let (inventory, hotbar) = new_player(base());
        app.world_mut()
            .spawn((LocalPlayer, inventory, hotbar, ActiveSlot::default()));
        app.update();
        app
    }

    fn player(app: &mut App) -> (Inventory, ActiveSlot) {
        let mut players = app
            .world_mut()
            .query_filtered::<(&Inventory, &ActiveSlot), With<LocalPlayer>>();
        let (inventory, active) = players.single(app.world()).unwrap();
        (inventory.clone(), *active)
    }

    fn player_mut(app: &mut App) -> (Mut<'_, Inventory>, Mut<'_, Hotbar>) {
        let mut players = app
            .world_mut()
            .query_filtered::<(&mut Inventory, &mut Hotbar), With<LocalPlayer>>();
        players.single_mut(app.world_mut()).unwrap()
    }

    fn set_active(app: &mut App, slot: usize) {
        let mut players = app
            .world_mut()
            .query_filtered::<&mut ActiveSlot, With<LocalPlayer>>();
        players.single_mut(app.world_mut()).unwrap().0 = slot;
    }

    fn active(app: &mut App) -> usize {
        player(app).1.0
    }

    const STARTING_COUNT_FOR_TESTS: u32 = crate::inventory::player::STARTING_COUNT;

    fn turn(app: &mut App, unit: MouseScrollUnit, y: f32) {
        app.insert_resource(AccumulatedMouseScroll {
            unit,
            delta: Vec2::new(0.0, y),
        });
        app.update();
        app.insert_resource(AccumulatedMouseScroll::default());
    }

    #[test]
    fn number_keys_pick_the_slot() {
        let mut app = app();
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::Digit7);
        app.update();
        assert_eq!(active(&mut app), 6);
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release_all();
        keys.clear();
        keys.press(KeyCode::Digit1);
        app.update();
        assert_eq!(active(&mut app), 0);
    }

    #[test]
    fn the_wheel_steps_through_the_slots_and_wraps() {
        let mut app = app();
        turn(&mut app, MouseScrollUnit::Line, -1.0);
        assert_eq!(active(&mut app), 1);
        turn(&mut app, MouseScrollUnit::Line, 3.0);
        assert_eq!(active(&mut app), 7);
        // A touchpad's pixels add up to whole steps.
        for _ in 0..3 {
            turn(&mut app, MouseScrollUnit::Pixel, -15.0);
        }
        assert_eq!(active(&mut app), 8);
        turn(&mut app, MouseScrollUnit::Line, -1.0);
        assert_eq!(active(&mut app), 0);
    }

    #[test]
    fn the_slots_show_the_hotbar_and_the_active_slot() {
        let mut app = app();
        let content = base();
        let (dirt, lamp) = (
            crate::content::dirt_item(),
            content.default_item("defaria:lamp").unwrap(),
        );
        {
            let (mut inventory, mut hotbar) = player_mut(&mut app);
            // Dirt moves to slot 5, from slot 2.
            hotbar.assign(4, Some(dirt));
            inventory.add(content, ItemStack::new(dirt, 7));
            // The lamps run out.
            inventory.remove(lamp, STARTING_COUNT_FOR_TESTS);
        }
        set_active(&mut app, 4);
        app.update();
        let mut slots = app.world_mut().query::<(&HotbarSlot, &ItemSlot)>();
        let shown: Vec<_> = slots
            .iter(app.world())
            .filter(|(slot, _)| [0, 1, 4, 7].contains(&slot.0))
            .map(|(slot, item)| (slot.0, *item))
            .collect();
        assert!(shown.contains(&(
            4,
            ItemSlot {
                item: Some(dirt),
                count: STARTING_COUNT_FOR_TESTS + 7,
                selected: true,
                dimmed: false,
            }
        )));
        let stone = content.default_item("defaria:stone").unwrap();
        assert!(shown.contains(&(
            0,
            ItemSlot {
                item: Some(stone),
                count: STARTING_COUNT_FOR_TESTS,
                selected: false,
                dimmed: false,
            }
        )));
        // Each kind is in one slot at most: slot 2 lost the dirt.
        assert!(shown.contains(&(1, ItemSlot::default())));
        // Out of lamps: still there, dimmed at 0.
        assert!(shown.contains(&(
            7,
            ItemSlot {
                item: Some(lamp),
                count: 0,
                selected: false,
                dimmed: true,
            }
        )));
    }

    /// Two entities carry different hotbars; the hotbar shows the local
    /// player's, and the number keys pick the local player's slot.
    #[test]
    fn the_hotbar_is_the_local_players() {
        let mut app = app();
        let lamp = base().default_item("defaria:lamp").unwrap();
        let mut other = Hotbar::default();
        other.assign(0, Some(lamp));
        let mob = app
            .world_mut()
            .spawn((Inventory::new(), other, ActiveSlot(3)))
            .id();
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::Digit2);
        app.update();
        assert_eq!(active(&mut app), 1);
        assert_eq!(app.world().get::<ActiveSlot>(mob), Some(&ActiveSlot(3)));
        let mut slots = app.world_mut().query::<(&HotbarSlot, &ItemSlot)>();
        let first = slots
            .iter(app.world())
            .find(|(slot, _)| slot.0 == 0)
            .unwrap()
            .1
            .item;
        assert_eq!(first, base().default_item("defaria:stone"));
    }

    #[test]
    fn switching_shows_the_item_name_then_fades_it() {
        let mut app = app();
        set_active(&mut app, 5);
        app.update();
        let mut names = app
            .world_mut()
            .query::<(&Text, &TextColor, &TextShadow, &ActiveItemName)>();
        let (text, color, shadow, _) = names.single(app.world()).unwrap();
        assert_eq!(text.0, "Oak Log");
        assert_eq!(color.0.alpha(), 1.0);
        assert_eq!(shadow.color, text_shadow().color);
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
        let full = text_shadow().color.alpha();
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
        assert_eq!(active(&mut app), 0);
        turn(&mut app, MouseScrollUnit::Pixel, -PIXELS_PER_STEP);
        assert_eq!(active(&mut app), 1);
    }

    #[test]
    fn step_slot_wraps_both_ways() {
        assert_eq!(step_slot(0, -1), 8);
        assert_eq!(step_slot(8, 1), 0);
        assert_eq!(step_slot(3, 20), 5);
    }
}
