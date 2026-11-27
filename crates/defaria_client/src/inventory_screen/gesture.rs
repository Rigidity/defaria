//! The mouse on item lists and the hotbar, with a screen open.
//!
//! Each list says where its items go ([`ItemList::sends_to`]), such as
//! between the creative catalog and the player's inventory (there, moving
//! items into the catalog throws them away):
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
//! `PointerDragDrop`, `PointerDragEnd`). A press that moves more than
//! `ui::DRAG_DISTANCE` is a drag, not a click, even if it ends where it
//! started ([`PointerTravel`]). A dragged item's icon follows the pointer.

use bevy::{
    picking::{
        events::{PointerClick, PointerDragDrop, PointerDragEnd, PointerDragStart},
        hover::Hovered,
        pointer::PointerButton,
    },
    prelude::*,
};

use super::list::{ItemList, ListEntry, ListSource};
use crate::{
    hotbar_ui::{HOTBAR_KEYS, HotbarInput, HotbarSlot},
    ui::{ItemIcon, PointerTravel, UiPointer, UiTheme, typing},
};
use defaria_core::inventory::{
    Inventory, ItemStack,
    transfer::{half, transfer},
};
use defaria_core::{
    content::{Content, ItemState},
    hotbar::Hotbar,
    player::LocalPlayer,
};

/// How many a shift-click takes from the catalog.
pub const CATALOG_BATCH: u32 = 100;
/// How many a right click takes from the catalog.
pub const CATALOG_HALF: u32 = 10;

pub struct GesturePlugin;

impl Plugin for GesturePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Dragging>()
            .register_type::<DragIcon>()
            .add_observer(click_entry)
            .add_observer(start_drag)
            .add_observer(drop_on)
            .add_observer(end_drag)
            .add_systems(Startup, spawn_drag_icon)
            .add_systems(
                Update,
                (
                    // Before the hotbar, which takes the number keys
                    // that are left.
                    bind_with_keys.run_if(not(typing)).before(HotbarInput),
                    show_drag_icon,
                ),
            );
    }
}

/// What's being dragged.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
enum Dragging {
    #[default]
    Nothing,
    /// An entry, and the button dragging it.
    Entry(ListEntry, PointerButton),
    /// A hotbar slot, and whether it's been dropped somewhere that took it.
    HotbarSlot(usize, bool),
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
    match (button, all, entry.source == ListSource::Catalog) {
        (PointerButton::Secondary, _, true) => CATALOG_HALF,
        (_, true, true) => CATALOG_BATCH,
        (_, _, true) => 1,
        _ => amount_of(entry.stack.count, button, all),
    }
}

/// How many of `count` a click or drag moves: one, half with the right
/// button, all with `all` (Shift, or a drag with the left button).
pub fn amount_of(count: u32, button: PointerButton, all: bool) -> u32 {
    match (button, all) {
        (PointerButton::Secondary, _) => half(count),
        (_, true) => count,
        _ => 1,
    }
}

/// Where `entry`'s list sends items: its parent's [`ItemList::sends_to`].
fn sends_to(
    entry: Entity,
    parents: &Query<&ChildOf>,
    lists: &Query<&ItemList>,
) -> Option<ListSource> {
    let list = parents.get(entry).ok()?.parent();
    lists.get(list).ok()?.sends_to
}

fn click_entry(
    click: On<PointerClick>,
    keys: Res<ButtonInput<KeyCode>>,
    content: Res<Content>,
    entries: Query<&ListEntry>,
    parents: Query<&ChildOf>,
    lists: Query<&ItemList>,
    travel: Res<PointerTravel>,
    player: Option<Single<Entity, With<LocalPlayer>>>,
    mut inventories: Query<&mut Inventory>,
) {
    let Ok(entry) = entries.get(click.entity) else {
        return;
    };
    let Some(to) = sends_to(click.entity, &parents, &lists) else {
        return;
    };
    if click.button == PointerButton::Middle {
        return;
    }
    // The end of a drag that came back where it started isn't a click.
    if !travel.is_click() {
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
    } else if let Ok(&HotbarSlot(index)) = slots.get(drag.entity)
        // Hotbar slots move with the left button only.
        && drag.button == PointerButton::Primary
    {
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
        // Onto any other list (not back onto its own).
        (Dragging::Entry(entry, button), Err(list)) => {
            if list != entry.source {
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
    travel: Res<PointerTravel>,
    mut dragging: ResMut<Dragging>,
    mut players: Query<&mut Hotbar, With<LocalPlayer>>,
) {
    // Only a real drag: a click that moved a pixel as it was let go is
    // still a click (and picking may not have sent a drop onto the slot).
    if let Dragging::HotbarSlot(slot, false) = *dragging
        && !travel.is_click()
        && let Ok(mut hotbar) = players.single_mut()
    {
        hotbar.assign(slot, None);
    }
    *dragging = Dragging::Nothing;
}

/// A number key over an entry puts its item in that hotbar slot, and uses
/// the key up so the hotbar doesn't also pick that slot.
fn bind_with_keys(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    entries: Query<(&ListEntry, &Hovered)>,
    mut players: Query<&mut Hotbar, With<LocalPlayer>>,
) {
    let Some(slot) = HOTBAR_KEYS.iter().position(|&key| keys.just_pressed(key)) else {
        return;
    };
    let Some(entry) = entries
        .iter()
        .find(|(_, hovered)| hovered.get())
        .map(|(entry, _)| entry)
    else {
        return;
    };
    if let Ok(mut hotbar) = players.single_mut() {
        keys.clear_just_pressed(HOTBAR_KEYS[slot]);
        hotbar.assign(slot, Some(entry.stack.item));
    }
}

/// The icon of what's being dragged, under the pointer.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
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
