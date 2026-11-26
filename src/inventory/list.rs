//! Item lists on screen: a scrolling grid of what an inventory holds,
//! sorted, or of the creative catalog's every item. `gesture` makes their
//! entries respond to the mouse.
//!
//! There's no arranging: a list is always sorted, under a heading per
//! category in the categories' order (items in none last, under Other),
//! and in item order within one, with an entry per kind of item showing
//! how many. Only what matches its filter ([`ListFilter`], set by its
//! screen's search box) shows. When it changes, its entries and headings
//! are updated in place, spawning or despawning only the difference, so
//! the list never draws half-built.

use bevy::prelude::*;

use super::{Inventory, ItemStack, model::ItemData, search::ItemQuery};
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
            .register_type::<ListHeading>()
            .register_type::<ListFilter>()
            // In Update, not just before the UI's own systems: an entry
            // spawned in PostUpdate draws faded for its first frame.
            .add_systems(Update, refresh_lists.in_set(ListSystems));
    }
}

/// Keeps item lists in step with what they show. Screens that spawn lists
/// in Update run before it, so a new list is filled on its first frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListSystems;

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
        (list, ListFilter::default(), extra),
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

/// One heading's worth of a list: its category (`None` for items in none)
/// and its entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub category: Option<usize>,
    pub entries: Vec<ItemStack>,
}

/// `entries` (in the order they're in) under their categories, in the
/// categories' order, then those in none. Empty sections are left out.
pub fn sections(content: &Content, entries: Vec<ItemStack>) -> Vec<Section> {
    let other = content.categories.len();
    let mut sections: Vec<Section> = (0..other)
        .map(Some)
        .chain([None])
        .map(|category| Section {
            category,
            entries: Vec::new(),
        })
        .collect();
    for stack in entries {
        let category = content.item_of(stack.item).and_then(|item| item.category);
        sections[category.unwrap_or(other)].entries.push(stack);
    }
    sections.retain(|section| !section.entries.is_empty());
    sections
}

/// Whether a list shows headings: once there's a category to name.
fn headed(sections: &[Section]) -> bool {
    sections.len() > 1 || sections.iter().any(|section| section.category.is_some())
}

/// What a list shows of its items: those that match, all by default.
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq, Eq)]
#[reflect(Component)]
pub struct ListFilter(#[reflect(ignore)] pub ItemQuery);

/// A category's heading in a list (or the line saying nothing matches).
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct ListHeading;

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
    lists: Query<(Entity, Ref<ItemList>, Ref<ListFilter>, Option<&Children>)>,
    mut entries: Query<(&mut ListEntry, &mut ItemSlot)>,
    mut headings: Query<&mut Text, With<ListHeading>>,
) {
    let player = player.map(|player| *player);
    for (list, item_list, filter, children) in &lists {
        let fresh = item_list.is_added() || filter.is_changed();
        let mut wanted = match item_list.source {
            ListSource::Catalog if fresh => catalog_entries(&content),
            ListSource::Catalog => continue,
            ListSource::Inventory(owner) => {
                let Some(inventory) = owner
                    .entity(player)
                    .and_then(|entity| inventories.get(entity).ok())
                else {
                    continue;
                };
                if !inventory.is_changed() && !fresh {
                    continue;
                }
                sorted_entries(&inventory)
            }
        };
        wanted.retain(|stack| filter.0.matches(&content, stack.item));
        let grouped = sections(&content, wanted);
        let headed = headed(&grouped);
        // A filter that leaves nothing says so, in a heading.
        let nothing = grouped.is_empty() && !filter.0.is_empty();
        let children: Vec<Entity> = children.map(|c| c.iter().collect()).unwrap_or_default();
        // What's there now, to reuse in order.
        let spare: Vec<Entity> = children
            .iter()
            .copied()
            .filter(|&e| entries.contains(e))
            .collect();
        let mut spare_entries = spare.into_iter();
        let spare: Vec<Entity> = children
            .iter()
            .copied()
            .filter(|&e| headings.contains(e))
            .collect();
        let mut spare_headings = spare.into_iter();
        let source = item_list.source;
        let mut order = Vec::new();
        let mut heading = |name: &str, commands: &mut Commands| match spare_headings.next() {
            Some(heading) => {
                if let Ok(mut text) = headings.get_mut(heading)
                    && text.0 != name
                {
                    text.0 = name.to_owned();
                }
                heading
            }
            None => commands.spawn(list_heading(&theme, name)).id(),
        };
        if nothing {
            order.push(heading("No items match", &mut commands));
        }
        for section in grouped {
            if headed {
                let name = section
                    .category
                    .and_then(|index| content.categories.get(index))
                    .map_or("Other", |category| category.name.as_str());
                order.push(heading(name, &mut commands));
            }
            for stack in section.entries {
                let entry = match spare_entries.next() {
                    Some(entity) => {
                        if let Ok((mut entry, mut slot)) = entries.get_mut(entity) {
                            slot.set_if_neq(entry_slot(source, &stack));
                            entry.set_if_neq(ListEntry { source, stack });
                        }
                        entity
                    }
                    None => {
                        let slot = entry_slot(source, &stack);
                        commands
                            .spawn((ListEntry { source, stack }, item_slot(&theme, slot)))
                            .id()
                    }
                };
                order.push(entry);
            }
        }
        drop(heading);
        for unused in spare_entries.chain(spare_headings) {
            commands.entity(unused).despawn();
        }
        if order != children {
            commands.entity(list).replace_children(&order);
        }
    }
}

/// A category's heading, on a line of its own.
fn list_heading(theme: &UiTheme, name: &str) -> impl Bundle {
    (
        ListHeading,
        Text::new(name),
        theme.text_font(),
        TextColor(theme.text_dim),
        Node {
            width: percent(100),
            ..default()
        },
    )
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

    #[test]
    fn lists_are_grouped_by_category_in_order() {
        let content = base();
        let grouped = sections(content, catalog_entries(content));
        let names: Vec<&str> = grouped
            .iter()
            .map(|section| {
                section
                    .category
                    .map_or("Other", |index| content.categories[index].name.as_str())
            })
            .collect();
        assert_eq!(
            names,
            [
                "Natural",
                "Materials",
                "Building",
                "Lighting",
                "Machines",
                "Tools"
            ]
        );
        // Every item, once, in item order within each.
        let total: usize = grouped.iter().map(|section| section.entries.len()).sum();
        assert_eq!(total, content.item_states.len());
        assert!(grouped.iter().all(|section| {
            section
                .entries
                .windows(2)
                .all(|pair| pair[0].item < pair[1].item)
        }));
        let pickaxe = content.default_item("defaria:pickaxe").unwrap();
        assert!(grouped[5].entries.iter().any(|stack| stack.item == pickaxe));
        // An inventory's few: only their sections.
        let stone = content.default_item("defaria:stone").unwrap();
        let grouped = sections(content, vec![ItemStack::new(stone, 4)]);
        assert_eq!(grouped.len(), 1);
        assert_eq!(
            grouped[0].category,
            content.item("defaria:stone").unwrap().category
        );
    }
}
