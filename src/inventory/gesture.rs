//! The mouse on item lists and the hotbar, with a screen open.
//!
//! A screen with two lists ([`ScreenLists`]) moves items between them, such
//! as the creative catalog and the player's inventory (there, moving items
//! into the catalog throws them away):
//!
//! - **Left click** an entry moves one to the other list; **Shift** moves
//!   them all (from the catalog, [`CATALOG_BATCH`]); **right click** moves
//!   half (from the catalog, [`CATALOG_HALF`]).
//! - **Dragging** an entry onto the other list moves them all (half with
//!   the right button).
//! - **Dragging** an entry onto a hotbar slot puts its item there, as do the
//!   **number keys** with the pointer over an entry.
//! - **Dragging** a hotbar slot onto another swaps them; dropping it
//!   anywhere else clears it.
//!
//! Clicks and drags are picking's (`PointerClick`, `PointerDragStart`,
//! `PointerDragDrop`, `PointerDragEnd`). A dragged item's icon follows the
//! pointer.

use bevy::{
    picking::{
        events::{PointerClick, PointerDragDrop, PointerDragEnd, PointerDragStart},
        hover::Hovered,
        pointer::PointerButton,
    },
    prelude::*,
};

use super::{
    Inventory, ItemStack,
    list::{ItemList, ListEntry, ListSource},
    transfer::{half, transfer},
};
use crate::{
    content::{Content, ItemState},
    hotbar::{HOTBAR_KEYS, Hotbar, HotbarSlot},
    player::LocalPlayer,
    ui::{ItemIcon, UiPointer, UiTheme, typing},
};

/// How many a shift-click takes from the catalog.
pub const CATALOG_BATCH: u32 = 100;
/// How many a right click takes from the catalog.
pub const CATALOG_HALF: u32 = 10;

pub struct GesturePlugin;

impl Plugin for GesturePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HoveredEntry>()
            .init_resource::<Dragging>()
            .add_observer(click_entry)
            .add_observer(start_drag)
            .add_observer(drop_on)
            .add_observer(end_drag)
            .add_systems(Startup, spawn_drag_icon)
            .add_systems(
                Update,
                (
                    find_hovered_entry,
                    bind_with_keys.run_if(not(typing)),
                    show_drag_icon,
                )
                    .chain(),
            );
    }
}

/// The two lists a screen moves items between. On the screen's root.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenLists(pub ListSource, pub ListSource);

impl ScreenLists {
    /// Where items from `source` go.
    pub fn other(&self, source: ListSource) -> Option<ListSource> {
        if source == self.0 {
            Some(self.1)
        } else if source == self.1 {
            Some(self.0)
        } else {
            None
        }
    }
}

/// The list entry under the pointer, if any.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct HoveredEntry(pub Option<ListEntry>);

/// A run condition: the pointer is over a list entry, so the number keys
/// put its item in the hotbar rather than picking a slot.
pub fn over_entry(hovered: Option<Res<HoveredEntry>>) -> bool {
    hovered.is_some_and(|hovered| hovered.0.is_some())
}

/// What's being dragged.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
enum Dragging {
    #[default]
    Nothing,
    Entry(ListEntry, PointerButton),
    /// A hotbar slot, and whether it's been dropped somewhere that took it.
    HotbarSlot(usize, bool),
}

fn find_hovered_entry(entries: Query<(&ListEntry, &Hovered)>, mut hovered: ResMut<HoveredEntry>) {
    let over = entries
        .iter()
        .find(|(_, hovered)| hovered.get())
        .map(|(entry, _)| entry.clone());
    if hovered.0 != over {
        hovered.0 = over;
    }
}

/// Moves up to `count` of `kind` from `from` to `to`, finding their
/// inventories. Returns how many moved.
fn move_items(
    content: &Content,
    player: Option<Entity>,
    inventories: &mut Query<&mut Inventory>,
    from: ListSource,
    to: ListSource,
    kind: &ItemStack,
    count: u32,
) -> u32 {
    let entity = |source| match source {
        ListSource::Catalog => None,
        ListSource::Inventory(owner) => owner.entity(player),
    };
    match (entity(from), entity(to)) {
        (Some(a), Some(b)) if a == b => 0,
        (Some(a), Some(b)) => {
            let Ok([mut a, mut b]) = inventories.get_many_mut([a, b]) else {
                return 0;
            };
            transfer(content, Some(&mut a), Some(&mut b), kind, count)
        }
        (Some(a), None) => inventories.get_mut(a).map_or(0, |mut a| {
            transfer(content, Some(&mut a), None, kind, count)
        }),
        (None, Some(b)) => inventories.get_mut(b).map_or(0, |mut b| {
            transfer(content, None, Some(&mut b), kind, count)
        }),
        (None, None) => 0,
    }
}

/// How many a click or drag moves of `entry`.
fn amount(entry: &ListEntry, button: PointerButton, all: bool) -> u32 {
    let catalog = entry.source == ListSource::Catalog;
    match (button, all, catalog) {
        (PointerButton::Secondary, _, true) => CATALOG_HALF,
        (PointerButton::Secondary, _, false) => half(entry.stack.count),
        (_, true, true) => CATALOG_BATCH,
        (_, true, false) => entry.stack.count,
        _ => 1,
    }
}

fn click_entry(
    click: On<PointerClick>,
    keys: Res<ButtonInput<KeyCode>>,
    content: Res<Content>,
    entries: Query<&ListEntry>,
    screen: Option<Single<&ScreenLists>>,
    player: Option<Single<Entity, With<LocalPlayer>>>,
    mut inventories: Query<&mut Inventory>,
) {
    let (Ok(entry), Some(screen)) = (entries.get(click.entity), screen) else {
        return;
    };
    let Some(to) = screen.other(entry.source) else {
        return;
    };
    if click.button == PointerButton::Middle {
        return;
    }
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let count = amount(entry, click.button, shift);
    let player = player.map(|player| *player);
    move_items(
        &content,
        player,
        &mut inventories,
        entry.source,
        to,
        &entry.stack,
        count,
    );
}

fn start_drag(
    drag: On<PointerDragStart>,
    entries: Query<&ListEntry>,
    slots: Query<&HotbarSlot>,
    mut dragging: ResMut<Dragging>,
) {
    if let Ok(entry) = entries.get(drag.entity) {
        *dragging = Dragging::Entry(entry.clone(), drag.button);
    } else if let Ok(&HotbarSlot(index)) = slots.get(drag.entity) {
        *dragging = Dragging::HotbarSlot(index, false);
    }
}

/// Where a drop landed: a hotbar slot, or a list (the entity or one of its
/// ancestors).
fn drop_target(
    entity: Entity,
    slots: &Query<&HotbarSlot>,
    lists: &Query<&ItemList>,
    parents: &Query<&ChildOf>,
) -> Option<Result<usize, ListSource>> {
    if let Ok(&HotbarSlot(index)) = slots.get(entity) {
        return Some(Ok(index));
    }
    std::iter::once(entity)
        .chain(parents.iter_ancestors(entity))
        .find_map(|entity| lists.get(entity).ok())
        .map(|list| Err(list.source))
}

fn drop_on(
    drop: On<PointerDragDrop>,
    content: Res<Content>,
    slots: Query<&HotbarSlot>,
    lists: Query<&ItemList>,
    parents: Query<&ChildOf>,
    screen: Option<Single<&ScreenLists>>,
    mut players: Query<&mut Hotbar, With<LocalPlayer>>,
    player: Option<Single<Entity, With<LocalPlayer>>>,
    mut inventories: Query<&mut Inventory>,
    mut dragging: ResMut<Dragging>,
) {
    // A drop bubbles up; only the node it landed on counts.
    if drop.entity != drop.original_event_target() {
        return;
    }
    let Some(target) = drop_target(drop.entity, &slots, &lists, &parents) else {
        return;
    };
    let player = player.map(|player| *player);
    match (&mut *dragging, target) {
        (Dragging::Entry(entry, _), Ok(slot)) => {
            if let Ok(mut hotbar) = players.single_mut() {
                hotbar.assign(slot, Some(entry.stack.item));
            }
        }
        (Dragging::Entry(entry, button), Err(list)) => {
            let to_other = screen.is_some_and(|screen| screen.other(entry.source) == Some(list));
            if to_other {
                let count = amount(entry, *button, *button == PointerButton::Primary);
                move_items(
                    &content,
                    player,
                    &mut inventories,
                    entry.source,
                    list,
                    &entry.stack,
                    count,
                );
            }
        }
        (Dragging::HotbarSlot(from, taken), Ok(to)) => {
            if let Ok(mut hotbar) = players.single_mut() {
                hotbar.0.swap(*from, to);
            }
            *taken = true;
        }
        (Dragging::HotbarSlot(..), Err(_)) | (Dragging::Nothing, _) => {}
    }
}

/// The drag is over: a hotbar slot dropped anywhere but another hotbar slot
/// is cleared.
fn end_drag(
    _: On<PointerDragEnd>,
    mut dragging: ResMut<Dragging>,
    mut players: Query<&mut Hotbar, With<LocalPlayer>>,
) {
    if let Dragging::HotbarSlot(slot, false) = *dragging
        && let Ok(mut hotbar) = players.single_mut()
    {
        hotbar.assign(slot, None);
    }
    *dragging = Dragging::Nothing;
}

/// A number key over an entry puts its item in that hotbar slot.
fn bind_with_keys(
    keys: Res<ButtonInput<KeyCode>>,
    hovered: Res<HoveredEntry>,
    mut players: Query<&mut Hotbar, With<LocalPlayer>>,
) {
    let Some(entry) = &hovered.0 else {
        return;
    };
    if let Some(slot) = HOTBAR_KEYS.iter().position(|&key| keys.just_pressed(key))
        && let Ok(mut hotbar) = players.single_mut()
    {
        hotbar.assign(slot, Some(entry.stack.item));
    }
}

/// The icon of what's being dragged, under the pointer.
#[derive(Component, Debug, Clone, Copy)]
struct DragIcon;

fn spawn_drag_icon(mut commands: Commands, theme: Res<UiTheme>) {
    commands.spawn((
        Name::new("Dragged item"),
        DragIcon,
        Node {
            position_type: PositionType::Absolute,
            width: px(theme.icon_size),
            height: px(theme.icon_size),
            ..default()
        },
        ItemIcon(None),
        // Above the screens, below the tooltip.
        GlobalZIndex(i32::MAX - 1),
        Pickable::IGNORE,
        Visibility::Hidden,
    ));
}

fn show_drag_icon(
    pointer: UiPointer,
    theme: Res<UiTheme>,
    dragging: Res<Dragging>,
    players: Query<&Hotbar, With<LocalPlayer>>,
    mut icons: Query<(&mut Node, &mut ItemIcon, &mut Visibility), With<DragIcon>>,
) {
    let item: Option<ItemState> = match &*dragging {
        Dragging::Entry(entry, _) => Some(entry.stack.item),
        Dragging::HotbarSlot(slot, _) => players.single().ok().and_then(|hotbar| hotbar.0[*slot]),
        Dragging::Nothing => None,
    };
    for (mut node, mut icon, mut visibility) in &mut icons {
        let (Some(item), Some(at)) = (item, pointer.position()) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        visibility.set_if_neq(Visibility::Inherited);
        icon.set_if_neq(ItemIcon(Some(item)));
        let corner = at - Vec2::splat(theme.icon_size / 2.0);
        let (left, top) = (px(corner.x), px(corner.y));
        if node.left != left || node.top != top {
            node.left = left;
            node.top = top;
        }
    }
}
