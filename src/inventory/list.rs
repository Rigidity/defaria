//! Item lists on screen: a scrolling grid of what an inventory holds,
//! sorted, or of the creative catalog's every item. `gesture` makes their
//! entries respond to the mouse.
//!
//! There's no arranging: an inventory's list is always sorted (in item
//! order for now), with an entry per kind of item showing how many. When
//! the inventory changes its entries are updated in place, spawning or
//! despawning only as many as the count of kinds changes by, so the list
//! never draws half-built.

use bevy::prelude::*;

use super::{Inventory, ItemStack, model::ItemData};
use crate::{
    content::{Content, ItemState},
    player::LocalPlayer,
    ui::{ItemSlot, UiTheme, item_slot, scroll_panel},
};

pub struct ListPlugin;

impl Plugin for ListPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ItemList>()
            .register_type::<ListEntry>()
            // In Update, not just before the UI's own systems: an entry
            // spawned in PostUpdate draws faded for its first frame.
            .add_systems(Update, refresh_lists);
    }
}

/// Whose inventory something on screen shows.
#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Owner {
    /// The local player's, whichever entity that is.
    LocalPlayer,
    /// The inventory on this entity, such as a chest.
    Entity(Entity),
}

impl Owner {
    /// The entity, with the local player being `player`.
    pub fn entity(self, player: Option<Entity>) -> Option<Entity> {
        match self {
            Self::LocalPlayer => player,
            Self::Entity(entity) => Some(entity),
        }
    }
}

/// What a list shows: an inventory, or the catalog of every item.
#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListSource {
    Catalog,
    Inventory(Owner),
}

/// A scrolling grid of items; its children are its [`ListEntry`]s.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq, Eq)]
#[reflect(Component)]
pub struct ItemList {
    pub source: ListSource,
    /// Where its items go when clicked (the screen's other list), if
    /// anywhere. Dragged, they go to whichever other list they're dropped
    /// on.
    pub sends_to: Option<ListSource>,
}

/// One entry of an [`ItemList`]: a kind of item, and how many (for the
/// catalog, one, standing for as many as you like).
#[derive(Component, Reflect, Debug, Clone, PartialEq, Eq)]
#[reflect(Component)]
pub struct ListEntry {
    pub source: ListSource,
    pub stack: ItemStack,
}

/// Slots in a row of a list.
pub const COLUMNS: usize = 9;

/// `list`, `rows` rows of slots tall (more scroll), spawned under
/// `parent`. `extra` goes on the list's node. Returns the list.
pub fn item_list(
    parent: &mut ChildSpawnerCommands,
    theme: &UiTheme,
    list: ItemList,
    rows: usize,
    extra: impl Bundle,
) -> Entity {
    let (slot, gap) = (theme.slot_size, theme.slot_gap);
    let width = COLUMNS as f32 * slot + (COLUMNS - 1) as f32 * gap;
    let height = rows as f32 * slot + (rows.max(1) - 1) as f32 * gap;
    scroll_panel(
        parent,
        theme,
        Node {
            width: px(width),
            height: px(height),
            flex_wrap: FlexWrap::Wrap,
            align_content: AlignContent::FlexStart,
            column_gap: px(gap),
            row_gap: px(gap),
            ..default()
        },
        slot + gap,
        (list, extra),
        |_| {},
    )
}

/// What an inventory's list shows: an entry per kind, sorted by item
/// (unique items, one per copy, next to each other).
pub fn sorted_entries(inventory: &Inventory) -> Vec<ItemStack> {
    let mut entries = inventory.entries().to_vec();
    entries.sort_by_key(|stack| stack.item);
    entries
}

/// Every item state, each as a catalog entry, in item order.
pub fn catalog_entries(content: &Content) -> Vec<ItemStack> {
    (0..content.item_states.len() as u32)
        .map(|index| ItemStack {
            item: ItemState(index),
            data: ItemData::default(),
            count: 1,
        })
        .collect()
}

/// Fills new lists, and keeps inventories' lists in step with them.
fn refresh_lists(
    mut commands: Commands,
    theme: Res<UiTheme>,
    content: Res<Content>,
    player: Option<Single<Entity, With<LocalPlayer>>>,
    inventories: Query<Ref<Inventory>>,
    lists: Query<(Entity, Ref<ItemList>, Option<&Children>)>,
    mut entries: Query<(&mut ListEntry, &mut ItemSlot)>,
) {
    let player = player.map(|player| *player);
    for (list, item_list, children) in &lists {
        let wanted = match item_list.source {
            ListSource::Catalog if item_list.is_added() => catalog_entries(&content),
            ListSource::Catalog => continue,
            ListSource::Inventory(owner) => {
                let Some(inventory) = owner
                    .entity(player)
                    .and_then(|entity| inventories.get(entity).ok())
                else {
                    continue;
                };
                if !inventory.is_changed() && !item_list.is_added() {
                    continue;
                }
                sorted_entries(&inventory)
            }
        };
        let shown: Vec<Entity> = children
            .map(|children| children.iter().filter(|e| entries.contains(*e)).collect())
            .unwrap_or_default();
        let source = item_list.source;
        // Reuse what's there, in order; spawn or despawn only the
        // difference.
        for (&entity, stack) in shown.iter().zip(&wanted) {
            if let Ok((mut entry, mut slot)) = entries.get_mut(entity) {
                entry.set_if_neq(ListEntry {
                    source,
                    stack: stack.clone(),
                });
                slot.set_if_neq(entry_slot(source, stack));
            }
        }
        for &extra in shown.iter().skip(wanted.len()) {
            commands.entity(extra).despawn();
        }
        if wanted.len() > shown.len() {
            commands.entity(list).with_children(|list| {
                for stack in wanted.into_iter().skip(shown.len()) {
                    let slot = entry_slot(source, &stack);
                    list.spawn((ListEntry { source, stack }, item_slot(&theme, slot)));
                }
            });
        }
    }
}

/// What an entry's slot shows.
fn entry_slot(source: ListSource, stack: &ItemStack) -> ItemSlot {
    ItemSlot {
        count: shown_count(source, stack),
        ..ItemSlot::item(stack.item)
    }
}

/// The count an entry shows: none for the catalog's.
fn shown_count(source: ListSource, stack: &ItemStack) -> u32 {
    match source {
        ListSource::Catalog => 1,
        ListSource::Inventory(_) => stack.count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::base;

    #[test]
    fn an_inventory_lists_sorted_by_item() {
        let content = base();
        let item = |id| content.default_item(id).unwrap();
        let mut inventory = Inventory::new();
        for (id, count) in [
            ("defaria:lamp", 3),
            ("defaria:glass", 5),
            ("defaria:pickaxe", 2),
        ] {
            inventory.add(content, ItemStack::new(item(id), count));
        }
        let sorted: Vec<(ItemState, u32)> = sorted_entries(&inventory)
            .iter()
            .map(|stack| (stack.item, stack.count))
            .collect();
        let mut expected = vec![
            (item("defaria:lamp"), 3),
            (item("defaria:glass"), 5),
            (item("defaria:pickaxe"), 1),
            (item("defaria:pickaxe"), 1),
        ];
        expected.sort();
        assert_eq!(sorted, expected);
        assert_eq!(catalog_entries(content).len(), content.item_states.len());
    }
}
