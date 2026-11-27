//! Fluid containers: items that hold some of one fluid, starting with the
//! bucket. Right click with one takes fluid from the world or a machine's
//! tank, or pours it back out.
//!
//! - An item is a container if its file has `fluid_container` (see
//!   `docs/modding.md`): its [`Item::fluid_capacity`]. What one holds is
//!   its [`ItemData::fluid`], a [`FluidAmount`], so filled containers count
//!   together only with ones holding the same fluid and amount, the fluid
//!   adds to their weight, and it's saved with the inventory.
//! - The hotbar names the container item, not one container, so a click
//!   picks which of the player's containers acts: see [`taker`] and
//!   [`pourer`]. Containers act on what the inventory holds, in creative
//!   too, so fluid is never made or lost.
//! - With a container in hand, the crosshair stops at fluid
//!   ([`fluid_target`]), at the height its surface is drawn.
//! - **Right click on fluid** takes from that voxel, if some container
//!   can take that fluid: as much as one container has room for (and the
//!   weight limit lets in), a full block's worth or what a part-full voxel
//!   holds. A part-filled container holding the same fluid is topped up
//!   before an empty one is used.
//! - **Shift and right click on fluid**, or right click on it with no
//!   container that can take it, pours a container of that fluid (never
//!   another) into the voxel, as much as fits, and the rest into the voxel
//!   in front of the face aimed at.
//! - **Right click on a block** pours the fullest container into the
//!   voxel in front of the face aimed at. On a block that does something
//!   when used (a machine with a screen, a scripted block) it only does so
//!   with Shift: without, the click goes on to the block.
//! - **Right click on a machine with tanks** takes from a tank holding
//!   fluid a container takes, or else (or with Shift) pours into a tank
//!   that takes the fluid and has room. With nothing to move, the click
//!   goes on to the machine (its screen opens).
//!
//! What doesn't fit where it's poured stays in the container; flow spreads
//! what does. Taking and pouring go through `fluid_network::world`'s
//! [`take`] and [`pour`], as pumps and outlets do, so flow wakes around
//! what changes. The click is a use ([`UseStep::Container`]), after the
//! wrench and before scripted blocks and machine screens. It's claimed
//! whenever the click was the container's, even if nothing moved (at the
//! weight limit, say), and passed on otherwise.

use bevy::prelude::*;

use crate::{
    block_edit::{Hit, fluid_target},
    block_entity::BlockEntities,
    content::{BlockState, Content, ItemState},
    fluid_network::{
        FluidAmount,
        world::{pour, take},
    },
    hotbar::{ActiveSlot, Hotbar},
    inventory::{Inventory, ItemStack, model::ItemData},
    machine::{Tank, Tanks, screen::has_screen},
    script::ScriptIndex,
    ui::SlotFluid,
    use_action::{UseAction, UseStep},
    world::Voxels,
};

pub struct FluidContainerPlugin;

impl Plugin for FluidContainerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, use_container.in_set(UseStep::Container));
    }
}

/// How much fluid one `item` holds, if it's a fluid container.
pub fn capacity(content: &Content, item: ItemState) -> Option<u64> {
    content.item_of(item)?.fluid_capacity
}

/// The data of a saved `item` holding `amount` of `fluid`, if it can: it
/// has to be a container with room for it, and `fluid` a fluid. Loading
/// checks it, since mods can change between saving and loading.
pub(crate) fn saved_data(
    content: &Content,
    item: ItemState,
    fluid: &str,
    amount: u64,
) -> Result<ItemData, String> {
    let Some(capacity) = capacity(content, item) else {
        return Err("it isn't a fluid container any more".into());
    };
    if !content
        .default_state(fluid)
        .is_some_and(|state| content.state(state).is_fluid())
    {
        return Err(format!("`{fluid}` isn't a fluid any more"));
    }
    if amount > capacity {
        return Err(format!(
            "{amount} of `{fluid}` is more than it holds ({capacity})"
        ));
    }
    Ok(ItemData::holding(fluid, amount))
}

/// What a slot showing `stack` shows of its fluid: `None` if it isn't a
/// fluid container.
pub(crate) fn slot_fluid(content: &Content, stack: &ItemStack) -> Option<SlotFluid> {
    let capacity = capacity(content, stack.item)?;
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
pub(crate) fn hotbar_fluid(
    content: &Content,
    inventory: &Inventory,
    item: ItemState,
) -> Option<SlotFluid> {
    let capacity = capacity(content, item)?;
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

/// One container like `stack` as a tank, to take and pour with.
fn as_tank(stack: &ItemStack, capacity: u64) -> Tank {
    let held = stack.data.fluid.as_ref();
    Tank {
        name: String::new(),
        fluid: held.map(|held| held.fluid.clone()),
        amount: held.map_or(0, |held| held.amount),
        capacity,
        accepts: Vec::new(),
    }
}

/// The data of a container holding what `tank` does.
fn data_of(tank: &Tank) -> ItemData {
    match &tank.fluid {
        Some(fluid) => ItemData::holding(fluid, tank.amount),
        None => ItemData::default(),
    }
}

/// The player's containers of `item` that can take some of `fluid`: one
/// part-filled with it (the fullest first), so it's topped up, or else an
/// empty one. `None` if every one is full or holds another fluid.
fn taker(inventory: &Inventory, item: ItemState, capacity: u64, fluid: &str) -> Option<ItemStack> {
    let mine = || {
        inventory
            .entries()
            .iter()
            .filter(move |stack| stack.item == item)
    };
    let part_filled = mine()
        .filter(|stack| {
            stack
                .data
                .fluid
                .as_ref()
                .is_some_and(|held| held.fluid == fluid && held.amount < capacity)
        })
        .max_by_key(|stack| stack.data.fluid.as_ref().map_or(0, |held| held.amount));
    part_filled
        .or_else(|| mine().find(|stack| stack.data.fluid.is_none()))
        .map(|stack| ItemStack {
            count: 1,
            ..stack.clone()
        })
}

/// The player's container of `item` to pour from: the fullest holding a
/// fluid `accepts` takes. `None` if none does.
fn pourer(
    inventory: &Inventory,
    item: ItemState,
    accepts: impl Fn(&FluidAmount) -> bool,
) -> Option<ItemStack> {
    inventory
        .entries()
        .iter()
        .filter(|stack| stack.item == item)
        .filter(|stack| stack.data.fluid.as_ref().is_some_and(&accepts))
        .max_by_key(|stack| stack.data.fluid.as_ref().map_or(0, |held| held.amount))
        .map(|stack| ItemStack {
            count: 1,
            ..stack.clone()
        })
}

/// How much more `fluid` the inventory's weight limit lets one container
/// take on: unlimited without a limit or for a weightless fluid.
fn weight_room(content: &Content, inventory: &Inventory, fluid: &str) -> u64 {
    let per_unit = ItemData::holding(fluid, 1).weight(content);
    match inventory.spare_weight(content) {
        Some(spare) if per_unit > 0.0 => (spare / per_unit).floor() as u64,
        _ => u64::MAX,
    }
}

/// Moves fluid between one container like `kind` and `tank` (a machine's,
/// or the world through `act`), then swaps the container in the
/// inventory for one holding what's left. `act` moves fluid between the
/// container's tank and the other side, and returns how much moved.
fn with_container(
    content: &Content,
    inventory: &mut Inventory,
    kind: &ItemStack,
    capacity: u64,
    act: impl FnOnce(&mut Tank) -> u64,
) -> u64 {
    let mut tank = as_tank(kind, capacity);
    let moved = act(&mut tank);
    // Its fluid was kept within the weight limit (`weight_room`).
    if moved > 0 {
        inventory.replace_data(content, kind, data_of(&tank));
    }
    moved
}

/// Takes fluid from a machine's tanks into one container, unless
/// `pour_only`, or else pours one into them. Returns whether the click was
/// the container's: fluid moved, or some container could take a tank's.
fn use_on_tanks(
    content: &Content,
    inventory: &mut Inventory,
    item: ItemState,
    capacity: u64,
    tanks: &mut Tanks,
    pour_only: bool,
) -> bool {
    (!pour_only && take_from_tanks(content, inventory, item, capacity, tanks))
        || pour_into_tanks(content, inventory, item, capacity, tanks)
}

/// Takes from the first tank holding a fluid a container can take, as
/// much as the weight limit lets in. Returns whether there was one, even
/// if none was let in.
fn take_from_tanks(
    content: &Content,
    inventory: &mut Inventory,
    item: ItemState,
    capacity: u64,
    tanks: &mut Tanks,
) -> bool {
    for tank in &mut tanks.0 {
        let Some(fluid) = tank.fluid.clone() else {
            continue;
        };
        let Some(kind) = taker(inventory, item, capacity, &fluid) else {
            continue;
        };
        let room = weight_room(content, inventory, &fluid);
        with_container(content, inventory, &kind, capacity, |bucket| {
            let given = tank.drain(tank.amount.min(bucket.room()).min(room));
            bucket.fill(&fluid, given)
        });
        return true;
    }
    false
}

/// Pours the fullest container whose fluid a tank takes into the first
/// tank that takes it and has room.
fn pour_into_tanks(
    content: &Content,
    inventory: &mut Inventory,
    item: ItemState,
    capacity: u64,
    tanks: &mut Tanks,
) -> bool {
    let takes = |held: &FluidAmount| {
        tanks
            .0
            .iter()
            .any(|tank| tank.takes(&held.fluid) && tank.room() > 0)
    };
    let Some(kind) = pourer(inventory, item, takes) else {
        return false;
    };
    let moved = with_container(content, inventory, &kind, capacity, |bucket| {
        let Some(fluid) = bucket.fluid.clone() else {
            return 0;
        };
        let Some(tank) = tanks
            .0
            .iter_mut()
            .find(|tank| tank.takes(&fluid) && tank.room() > 0)
        else {
            return 0;
        };
        let put = tank.fill(&fluid, bucket.amount);
        bucket.drain(put)
    });
    moved > 0
}

/// Takes the fluid at `pos` into one container, if it's `fluid`. Returns
/// how much moved.
fn take_from_world(
    content: &Content,
    voxels: &mut Voxels,
    inventory: &mut Inventory,
    kind: &ItemStack,
    capacity: u64,
    pos: IVec3,
) -> u64 {
    let room = voxels
        .get_with_fluid(pos)
        .and_then(|(_, fluid)| fluid)
        .map(|fluid| weight_room(content, inventory, &content.block_of(fluid.kind).id))
        .unwrap_or(0);
    with_container(content, inventory, kind, capacity, |bucket| {
        take(voxels, content, pos, room, bucket)
    })
}

/// Pours one container into the voxels at `places`, in order, as much as
/// fits in each, until it's empty: the fullest container holding `only`,
/// or any fluid without it, that fits in one of them. Returns whether any
/// moved.
fn pour_into_world(
    content: &Content,
    voxels: &mut Voxels,
    inventory: &mut Inventory,
    item: ItemState,
    capacity: u64,
    places: &[IVec3],
    only: Option<&str>,
) -> bool {
    // Into the room a voxel's block leaves (all of air's, a slab's
    // half), empty or holding the same fluid.
    let fits_at = |voxels: &Voxels, pos: IVec3, held: &FluidAmount| {
        content
            .default_state(&held.fluid)
            .and_then(|kind| voxels.room_for(pos, kind))
            .is_some_and(|room| room > 0)
    };
    let fits = |held: &FluidAmount| {
        only.is_none_or(|only| held.fluid == only)
            && places.iter().any(|&pos| fits_at(voxels, pos, held))
    };
    let Some(kind) = pourer(inventory, item, fits) else {
        return false;
    };
    with_container(content, inventory, &kind, capacity, |bucket| {
        places
            .iter()
            .map(|&pos| pour(voxels, content, pos, u64::MAX, bucket))
            .sum()
    }) > 0
}

/// Right click with a container of `item` at `hit` in the world (not a
/// machine's tanks; see the module docs). Returns whether the click was
/// the container's: `false` passes it on, to use the block.
fn use_on_world(
    content: &Content,
    voxels: &mut Voxels,
    inventory: &mut Inventory,
    item: ItemState,
    capacity: u64,
    hit: Hit,
    pour_only: bool,
    usable: impl Fn(BlockState) -> bool,
) -> bool {
    let Some((state, here)) = voxels.get_with_fluid(hit.block) else {
        return false;
    };
    let in_front = hit.place_at().map(|(pos, _)| pos);
    // A voxel's fluid, beside its block (a slab in a pond has water too).
    let fluid = here.map(|here| content.block_of(here.kind).id.clone());
    if let Some(fluid) = &fluid
        && !pour_only
        && let Some(kind) = taker(inventory, item, capacity, fluid)
    {
        // Some container can take it, so the click takes, even if the
        // weight limit lets none in.
        take_from_world(content, voxels, inventory, &kind, capacity, hit.block);
        return true;
    }
    if fluid.is_none() && !pour_only && usable(state) {
        return false;
    }
    // Onto fluid, only that fluid: into it, and what doesn't fit in front
    // of it.
    let only = fluid.as_deref();
    let places: Vec<IVec3> = fluid
        .is_some()
        .then_some(hit.block)
        .into_iter()
        .chain(in_front)
        .collect();
    pour_into_world(content, voxels, inventory, item, capacity, &places, only)
}

/// The fluid container in hand, and what one holds.
fn in_hand(content: &Content, hotbar: &Hotbar, active: ActiveSlot) -> Option<(ItemState, u64)> {
    let item = hotbar.item(active)?;
    Some((item, capacity(content, item)?))
}

/// A use with a fluid container in hand: takes or pours (see the module
/// docs), claiming the use when it's the container's.
fn use_container(
    mut players: Query<(&mut UseAction, &Hotbar, &ActiveSlot, &mut Inventory)>,
    content: Res<Content>,
    scripts: Option<Res<ScriptIndex>>,
    mut voxels: Voxels,
    index: Res<BlockEntities>,
    mut tanks: Query<&mut Tanks>,
) {
    for (mut action, hotbar, active, mut inventory) in &mut players {
        let Some(using) = action.pending() else {
            continue;
        };
        let Some((item, capacity)) = in_hand(&content, hotbar, *active) else {
            continue;
        };
        let Some(hit) = fluid_target(&using.eyes, &content, &voxels) else {
            continue;
        };
        let used = use_at(
            &content,
            scripts.as_deref(),
            &mut voxels,
            &index,
            &mut tanks,
            inventory.as_mut(),
            (item, capacity),
            hit,
            using.sneak,
        );
        if used {
            action.claim();
        }
    }
}

/// Takes or pours with `item` (holding `capacity`) at `hit`, pouring only
/// if `pour_only`: whether the use was the container's.
fn use_at(
    content: &Content,
    scripts: Option<&ScriptIndex>,
    voxels: &mut Voxels,
    index: &BlockEntities,
    tanks: &mut Query<&mut Tanks>,
    inventory: &mut Inventory,
    (item, capacity): (ItemState, u64),
    hit: Hit,
    pour_only: bool,
) -> bool {
    if let Some(mut tanks) = index.get(hit.block).and_then(|e| tanks.get_mut(e).ok()) {
        return use_on_tanks(content, inventory, item, capacity, &mut tanks, pour_only);
    }
    let usable = |state| {
        content.machine(state).is_some_and(has_screen)
            || scripts.is_some_and(|scripts| scripts.is_usable(content, state))
    };
    use_on_world(
        content, voxels, inventory, item, capacity, hit, pour_only, usable,
    )
}

#[cfg(test)]
mod tests;
