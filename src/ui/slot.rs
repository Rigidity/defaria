//! The item slot widget: a framed square showing an item's icon and, for
//! more than one, how many in its corner. The pointer can hover it (a
//! lighter frame, and the item's name as a tooltip) and click it. A
//! selected slot has a gold frame.
//!
//! Slots don't know what clicking them means: whoever spawns one observes
//! picking's events on it (such as `PointerPress`) and sets its
//! [`ItemSlot`].

use bevy::{picking::hover::Hovered, prelude::*};

use super::{ItemIcon, Tooltip, UiTheme, UiWidgetSystems, icon::show_icons};
use crate::{
    content::{Content, ItemState},
    inventory::ItemStack,
};

pub struct SlotPlugin;

impl Plugin for SlotPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ItemSlot>().add_systems(
            PostUpdate,
            (show_slot_items, style_slots)
                .before(show_icons)
                .in_set(UiWidgetSystems),
        );
    }
}

/// What a slot shows, and whether it's the selected one.
#[derive(Component, Reflect, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[reflect(Component)]
#[require(Hovered, Tooltip)]
pub struct ItemSlot {
    pub item: Option<ItemState>,
    /// How many, shown when more than one (or for a dimmed slot).
    pub count: u32,
    pub selected: bool,
    /// Faded: none of the item is there, such as a hotbar slot whose item
    /// ran out.
    pub dimmed: bool,
}

impl ItemSlot {
    /// A slot showing `stack`, not selected.
    pub fn of(stack: Option<&ItemStack>) -> Self {
        Self {
            item: stack.map(|stack| stack.item),
            count: stack.map_or(0, |stack| stack.count),
            ..default()
        }
    }

    /// A slot showing one `item`, with no count.
    pub fn item(item: ItemState) -> Self {
        Self {
            item: Some(item),
            count: 1,
            ..default()
        }
    }
}

/// The icon inside an [`ItemSlot`].
#[derive(Component, Debug, Clone, Copy)]
struct SlotIcon;

/// The count in an [`ItemSlot`]'s corner.
#[derive(Component, Debug, Clone, Copy)]
struct SlotCount;

/// Text showing how many: nothing for one or none, and short for big numbers
/// (`12k`, `4.1M`), to fit a slot's corner.
pub fn count_text(count: u32) -> String {
    fn short(value: f64, unit: &str) -> String {
        if value < 10.0 {
            let text = format!("{:.1}", (value * 10.0).floor() / 10.0);
            format!("{}{unit}", text.trim_end_matches(".0"))
        } else {
            format!("{}{unit}", value.floor())
        }
    }
    match count {
        0 | 1 => String::new(),
        2..10_000 => count.to_string(),
        10_000..1_000_000 => short(f64::from(count) / 1000.0, "k"),
        _ => short(f64::from(count) / 1_000_000.0, "M"),
    }
}

/// An item slot, ready to spawn.
pub fn item_slot(theme: &UiTheme, slot: ItemSlot) -> impl Bundle {
    (
        slot,
        Node {
            width: px(theme.slot_size),
            height: px(theme.slot_size),
            border: UiRect::all(px(theme.slot_border)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(theme.slot),
        BorderColor::all(theme.slot_edge),
        children![
            (
                SlotIcon,
                ItemIcon(slot.item),
                // Picking finds the slot itself.
                Pickable::IGNORE,
                Node {
                    width: px(theme.icon_size),
                    height: px(theme.icon_size),
                    ..default()
                },
            ),
            (
                SlotCount,
                Text::new(count_text(slot.count)),
                theme.text_font(),
                TextColor(theme.text),
                TextShadow {
                    offset: Vec2::splat(1.0),
                    ..default()
                },
                Node {
                    position_type: PositionType::Absolute,
                    right: px(2),
                    bottom: px(0),
                    ..default()
                },
                Pickable::IGNORE,
            )
        ],
    )
}

/// Passes each changed slot's item to its icon, count and tooltip.
fn show_slot_items(
    content: Res<Content>,
    mut slots: Query<(&ItemSlot, &Children, &mut Tooltip), Changed<ItemSlot>>,
    mut icons: Query<&mut ItemIcon, With<SlotIcon>>,
    mut counts: Query<&mut Text, With<SlotCount>>,
) {
    for (slot, children, mut tooltip) in &mut slots {
        let name = slot.item.and_then(|item| content.item_name(item));
        let name = name.unwrap_or_default();
        if tooltip.0 != name {
            tooltip.0 = name.to_owned();
        }
        for &child in children {
            if let Ok(mut icon) = icons.get_mut(child) {
                icon.set_if_neq(ItemIcon(slot.item));
            }
            if let Ok(mut text) = counts.get_mut(child) {
                // A dimmed slot shows its 0.
                let count = if slot.dimmed {
                    slot.count.to_string()
                } else {
                    count_text(slot.count)
                };
                if text.0 != count {
                    text.0 = count;
                }
            }
        }
    }
}

/// The frame color (gold when selected, light while hovered), and the
/// icon faded while dimmed.
fn style_slots(
    theme: Res<UiTheme>,
    mut slots: Query<(&ItemSlot, &Hovered, &mut BorderColor, &Children)>,
    mut icons: Query<&mut ImageNode, With<SlotIcon>>,
) {
    for (slot, hovered, mut border, children) in &mut slots {
        let tint = Color::WHITE.with_alpha(if slot.dimmed { 0.3 } else { 1.0 });
        for &child in children {
            if let Ok(mut icon) = icons.get_mut(child)
                && icon.color != tint
            {
                icon.color = tint;
            }
        }
        let color = if slot.selected {
            theme.slot_selected
        } else if hovered.get() {
            theme.slot_hovered
        } else {
            theme.slot_edge
        };
        border.set_if_neq(BorderColor::all(color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_fit_a_corner() {
        let shown: Vec<String> = [0, 1, 2, 9999, 10_000, 12_345, 999_999, 1_000_000, 4_150_000]
            .into_iter()
            .map(count_text)
            .collect();
        assert_eq!(
            shown,
            ["", "", "2", "9999", "10k", "12k", "999k", "1M", "4.1M"]
        );
    }
    use crate::content::base;

    #[test]
    fn a_slot_shows_its_item_and_names_it() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(base().clone())
            .init_resource::<UiTheme>()
            .add_plugins(SlotPlugin);
        let theme = UiTheme::default();
        let stone = base().default_item("defaria:stone");
        let slot = app
            .world_mut()
            .spawn(item_slot(&theme, ItemSlot::default()))
            .id();
        app.update();
        let icon = app.world().get::<Children>(slot).unwrap()[0];
        assert_eq!(app.world().get::<ItemIcon>(icon), Some(&ItemIcon(None)));

        app.world_mut().get_mut::<ItemSlot>(slot).unwrap().item = stone;
        app.update();
        assert_eq!(app.world().get::<ItemIcon>(icon), Some(&ItemIcon(stone)));
        assert_eq!(app.world().get::<Tooltip>(slot).unwrap().0, "Stone");

        // Selected and hovered: the selection shows.
        let mut entity = app.world_mut().entity_mut(slot);
        entity.insert(Hovered(true));
        entity.get_mut::<ItemSlot>().unwrap().selected = true;
        app.update();
        assert_eq!(
            *app.world().get::<BorderColor>(slot).unwrap(),
            BorderColor::all(theme.slot_selected)
        );
        app.world_mut().get_mut::<ItemSlot>(slot).unwrap().selected = false;
        app.update();
        assert_eq!(
            *app.world().get::<BorderColor>(slot).unwrap(),
            BorderColor::all(theme.slot_hovered)
        );
    }
}
