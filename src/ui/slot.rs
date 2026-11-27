//! The item slot widget: a framed square showing an item's icon and, for
//! more than one, how many in its corner. A fluid container's slot also
//! shows how full it is, in a gauge down its left edge in the fluid's
//! color, and says what it holds in its tooltip. The pointer can hover it (a
//! lighter frame, and the item's name as a tooltip) and click it. A
//! selected slot has a gold frame.
//!
//! Slots don't know what clicking them means: whoever spawns one observes
//! picking's events on it (such as `PointerPress`) and sets its
//! [`ItemSlot`].

use bevy::{picking::hover::Hovered, prelude::*};

use super::fluid::{fluid_color, fluid_name, format_fluid};
use super::{ItemIcon, Tooltip, UiTheme, UiWidgetSystems, icon::show_icons};
use crate::{
    content::{BlockState, Content, ItemState},
    fluid_container,
    inventory::{Inventory, ItemStack},
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
    /// For a fluid container, what it holds.
    pub fluid: Option<SlotFluid>,
}

/// What a fluid container in a slot holds, to show.
#[derive(Reflect, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlotFluid {
    /// The fluid's block, or `None` for an empty container.
    pub fluid: Option<BlockState>,
    /// In thousandths of a block, as fluid is counted.
    pub amount: u64,
    pub capacity: u64,
}

impl SlotFluid {
    /// How full, from 0 to 1.
    pub fn fraction(self) -> f32 {
        if self.capacity == 0 {
            0.0
        } else {
            (self.amount as f64 / self.capacity as f64).min(1.0) as f32
        }
    }

    /// The tooltip's line for it: the fluid and how much.
    pub fn describe(self, content: &Content) -> String {
        let name = self.fluid.map_or_else(
            || "Empty".to_owned(),
            |fluid| fluid_name(content, &content.block_of(fluid).id),
        );
        format!(
            "{name}: {} of {}",
            format_fluid(self.amount),
            format_fluid(self.capacity)
        )
    }

    /// The gauge's color: the fluid's, a little lighter, so it stands out
    /// on the gauge's dark track in a small slot.
    fn color(self, content: &Content) -> Color {
        fluid_color(content, self.fluid).lighter(0.15)
    }
}

impl ItemSlot {
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

/// The empty part of a fluid gauge: dark, so any fluid's color shows on
/// it, over the sky or the ground.
const GAUGE_TRACK: Color = Color::srgba(0.0, 0.0, 0.0, 0.6);

/// The fluid gauge down an [`ItemSlot`]'s left edge, shown for a fluid
/// container.
#[derive(Component, Debug, Clone, Copy)]
struct SlotGauge;

/// How far up a [`SlotGauge`] is filled.
#[derive(Component, Debug, Clone, Copy)]
struct SlotGaugeFill;

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
            ),
            (
                SlotGauge,
                Node {
                    display: Display::None,
                    position_type: PositionType::Absolute,
                    left: px(1),
                    top: px(4),
                    bottom: px(4),
                    width: px(4),
                    ..default()
                },
                BackgroundColor(GAUGE_TRACK),
                Pickable::IGNORE,
                children![(
                    SlotGaugeFill,
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(0),
                        bottom: px(0),
                        width: percent(100),
                        height: percent(0),
                        ..default()
                    },
                    BackgroundColor(Color::NONE),
                    Pickable::IGNORE,
                )],
            )
        ],
    )
}

/// Passes each changed slot's item to its icon, count, gauge and tooltip.
fn show_slot_items(
    content: Res<Content>,
    mut slots: Query<(&ItemSlot, &Children, &mut Tooltip), Changed<ItemSlot>>,
    mut icons: Query<&mut ItemIcon, With<SlotIcon>>,
    mut counts: Query<&mut Text, With<SlotCount>>,
    mut gauges: Query<(&mut Node, &Children), (With<SlotGauge>, Without<SlotGaugeFill>)>,
    mut fills: Query<(&mut Node, &mut BackgroundColor), With<SlotGaugeFill>>,
) {
    for (slot, children, mut tooltip) in &mut slots {
        let name = slot.item.and_then(|item| content.item_name(item));
        let mut text = name.unwrap_or_default().to_owned();
        if let Some(fluid) = slot.fluid.filter(|_| slot.item.is_some()) {
            text = format!("{text}\n{}", fluid.describe(&content));
        }
        if tooltip.0 != text {
            tooltip.0 = text;
        }
        for &child in children {
            if let Ok((mut node, gauge)) = gauges.get_mut(child) {
                let fluid = slot.fluid.filter(|_| slot.item.is_some());
                let display = if fluid.is_some() {
                    Display::Flex
                } else {
                    Display::None
                };
                if node.display != display {
                    node.display = display;
                }
                for &fill in gauge {
                    if let (Some(fluid), Ok((mut node, mut color))) = (fluid, fills.get_mut(fill)) {
                        node.height = percent(fluid.fraction() * 100.0);
                        color.set_if_neq(BackgroundColor(fluid.color(&content)));
                    }
                }
            }
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
/// icon: faded while dimmed, and invisible with no item.
fn style_slots(
    theme: Res<UiTheme>,
    mut slots: Query<(&ItemSlot, &Hovered, &mut BorderColor, &Children)>,
    mut icons: Query<&mut ImageNode, With<SlotIcon>>,
) {
    for (slot, hovered, mut border, children) in &mut slots {
        // No item: nothing at all. An image node without an image draws a
        // white box, so it has to be see-through.
        let tint = match (slot.item, slot.dimmed) {
            (None, _) => Color::NONE,
            (Some(_), true) => Color::WHITE.with_alpha(0.3),
            (Some(_), false) => Color::WHITE,
        };
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

/// What a slot showing `stack` shows of its fluid: `None` if it isn't a
/// fluid container.
pub fn slot_fluid(content: &Content, stack: &ItemStack) -> Option<SlotFluid> {
    let capacity = fluid_container::capacity(content, stack.item)?;
    let held = stack.data.fluid.as_ref();
    Some(SlotFluid {
        fluid: held.and_then(|held| content.default_state(&held.fluid)),
        amount: held.map_or(0, |held| held.amount),
        capacity,
    })
}

/// What a hotbar slot naming the container `item` shows of its fluid: all
/// the inventory's containers of it together, how much they hold of how
/// much they could, in the color of the first fluid among them. `None` if
/// it isn't a fluid container.
pub fn hotbar_fluid(
    content: &Content,
    inventory: &Inventory,
    item: ItemState,
) -> Option<SlotFluid> {
    let capacity = fluid_container::capacity(content, item)?;
    let mine = inventory
        .entries()
        .iter()
        .filter(|stack| stack.item == item);
    let mut shown = SlotFluid::default();
    for stack in mine {
        let count = stack.count as u64;
        shown.capacity = shown
            .capacity
            .saturating_add(capacity.saturating_mul(count));
        if let Some(held) = &stack.data.fluid {
            shown.amount = shown
                .amount
                .saturating_add(held.amount.saturating_mul(count));
            shown.fluid = shown.fluid.or_else(|| content.default_state(&held.fluid));
        }
    }
    Some(shown)
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
        let tint = |app: &App| app.world().get::<ImageNode>(icon).unwrap().color;
        assert_eq!(app.world().get::<ItemIcon>(icon), Some(&ItemIcon(None)));
        // Empty: nothing drawn, not a white box.
        assert_eq!(tint(&app), Color::NONE);

        app.world_mut().get_mut::<ItemSlot>(slot).unwrap().item = stone;
        app.update();
        assert_eq!(app.world().get::<ItemIcon>(icon), Some(&ItemIcon(stone)));
        assert_eq!(app.world().get::<Tooltip>(slot).unwrap().0, "Stone");
        assert_eq!(tint(&app), Color::WHITE);
        app.world_mut().get_mut::<ItemSlot>(slot).unwrap().dimmed = true;
        app.update();
        assert_eq!(tint(&app), Color::WHITE.with_alpha(0.3));
        app.world_mut().get_mut::<ItemSlot>(slot).unwrap().dimmed = false;
        // Emptied again: hidden again.
        app.world_mut().get_mut::<ItemSlot>(slot).unwrap().item = None;
        app.update();
        assert_eq!(tint(&app), Color::NONE);
        app.world_mut().get_mut::<ItemSlot>(slot).unwrap().item = stone;

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

#[cfg(test)]
mod gauge_tests {
    use super::*;
    use crate::content::{base, block};

    /// A fluid container's slot shows a gauge filled as far as it's full,
    /// and says what it holds in its tooltip; other items show none.
    #[test]
    fn a_containers_slot_shows_its_fluid() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(base().clone())
            .init_resource::<UiTheme>()
            .add_plugins(SlotPlugin);
        let theme = UiTheme::default();
        let bucket = base().default_item("defaria:bucket").unwrap();
        let slot = app
            .world_mut()
            .spawn(item_slot(&theme, ItemSlot::item(bucket)))
            .id();
        app.update();
        let gauge = app.world().get::<Children>(slot).unwrap()[2];
        let fill = app.world().get::<Children>(gauge).unwrap()[0];
        let shown = |app: &App| app.world().get::<Node>(gauge).unwrap().display;
        assert_eq!(shown(&app), Display::None);
        assert_eq!(app.world().get::<Tooltip>(slot).unwrap().0, "Bucket");

        app.world_mut().get_mut::<ItemSlot>(slot).unwrap().fluid = Some(SlotFluid {
            fluid: Some(block("water")),
            amount: 500,
            capacity: 1000,
        });
        app.update();
        assert_eq!(shown(&app), Display::Flex);
        assert_eq!(app.world().get::<Node>(fill).unwrap().height, percent(50));
        assert_eq!(
            app.world().get::<Tooltip>(slot).unwrap().0,
            "Bucket\nWater: 500 L of 1,000 L"
        );

        app.world_mut().get_mut::<ItemSlot>(slot).unwrap().fluid = None;
        app.update();
        assert_eq!(shown(&app), Display::None);
        assert_eq!(app.world().get::<Tooltip>(slot).unwrap().0, "Bucket");
    }
}
