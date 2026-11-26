//! The item slot widget: a framed square showing an item's icon, which the
//! pointer can hover (a lighter frame, and the item's name as a tooltip)
//! and click. A selected slot has a gold frame.
//!
//! Slots don't know what clicking them means: whoever spawns one observes
//! picking's events on it (such as `PointerPress`) and sets its
//! [`ItemSlot`].

use bevy::{picking::hover::Hovered, prelude::*};

use super::{ItemIcon, Tooltip, UiTheme, UiWidgetSystems, icon::show_icons};
use crate::content::{Content, ItemState};

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
    pub selected: bool,
}

/// The icon inside an [`ItemSlot`].
#[derive(Component, Debug, Clone, Copy)]
struct SlotIcon;

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
        children![(
            SlotIcon,
            ItemIcon(slot.item),
            // Picking finds the slot itself.
            Pickable::IGNORE,
            Node {
                width: px(theme.icon_size),
                height: px(theme.icon_size),
                ..default()
            },
        )],
    )
}

/// Passes each changed slot's item to its icon and its tooltip.
fn show_slot_items(
    content: Res<Content>,
    mut slots: Query<(&ItemSlot, &Children, &mut Tooltip), Changed<ItemSlot>>,
    mut icons: Query<&mut ItemIcon, With<SlotIcon>>,
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
        }
    }
}

/// The frame color: gold when selected, light while hovered.
fn style_slots(theme: Res<UiTheme>, mut slots: Query<(&ItemSlot, &Hovered, &mut BorderColor)>) {
    for (slot, hovered, mut border) in &mut slots {
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
