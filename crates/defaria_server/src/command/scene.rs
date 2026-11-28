//! The commands that set up a scene: `fill`, `pour`, `put`, `hotbar`,
//! `give` and `tp`. Each is a system run with its arguments, failing
//! with what was wrong.

use bevy::prelude::*;

use super::super::block_entity::BlockEntities;
use defaria_core::{
    content::{BlockState, Content, FULL_FLUID, ItemState, PART_SEPARATOR, StateName},
    fluid_container,
    hotbar::{ActiveSlot, Hotbar},
    inventory::{Inventory, ItemStack},
    machine::{Slots, Tanks},
    physics::Body,
    player::{EYE_HEIGHT, Player, put_eyes},
    world::{VoxelFluid, Voxels},
};

/// The state name `name` (see [`StateName`]), its id qualified with
/// `defaria:` if it has no mod.
fn split_name(name: &str) -> Result<StateName, String> {
    let mut parsed = StateName::parse(name)?;
    if !parsed.id.contains(':') {
        parsed.id = format!("defaria:{}", parsed.id);
    }
    Ok(parsed)
}

/// The item state `name` means, named as for [`fill_state`].
pub fn item_state(content: &Content, name: &str) -> Result<ItemState, String> {
    let parsed = split_name(name)?;
    let id = &parsed.id;
    let item = content.item(id).ok_or_else(|| format!("no item `{id}`"))?;
    item.state(&parsed.pairs())
        .ok_or_else(|| format!("item `{id}` has no state `{name}`"))
}

/// The state `name` means: a block id (the base game's without
/// `defaria:`), with `[property=value,…]` setting some of its properties;
/// or two halves joined by a `+`, for the voxel of both
/// (`stone_slab[half=bottom]+planks_slab[half=top]`).
pub fn fill_state(content: &Content, name: &str) -> Result<BlockState, String> {
    if let Some((low, high)) = name.split_once(PART_SEPARATOR) {
        let (low, high) = (fill_state(content, low)?, fill_state(content, high)?);
        return content
            .combine(low, high)
            .ok_or_else(|| format!("`{name}`: those two halves don't fit in one voxel"));
    }
    let parsed = split_name(name)?;
    let id = &parsed.id;
    let block = content
        .block(id)
        .ok_or_else(|| format!("no block `{id}`"))?;
    block
        .state(&parsed.pairs())
        .ok_or_else(|| format!("block `{id}` has no state `{name}`"))
}

/// The most voxels one `fill` may set: 256 a side.
pub const MAX_FILL: u64 = 256 * 256 * 256;

/// Sets every voxel between `min` and `max` to `id`. Returns how many it
/// changed: not ones already as it leaves them, nor ones in chunks that
/// aren't loaded.
pub(super) fn fill(
    In((min, max, id)): In<(IVec3, IVec3, String)>,
    content: Res<Content>,
    mut voxels: Voxels,
) -> Result<usize, String> {
    let size = max.as_i64vec3() - min.as_i64vec3() + bevy::math::I64Vec3::ONE;
    if (size.x * size.y * size.z) as u64 > MAX_FILL {
        return Err(format!("fill: that's more than {MAX_FILL} voxels"));
    }
    let state = if id == defaria_core::content::AIR {
        BlockState::AIR
    } else {
        fill_state(&content, &id).map_err(|error| format!("fill: {error}"))?
    };
    let mut set = 0;
    for x in min.x..=max.x {
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                let pos = IVec3::new(x, y, z);
                let before = voxels.get_with_fluid(pos);
                voxels.set(pos, state);
                // Air clears fluid too.
                if state.is_air() {
                    voxels.set_fluid(pos, None);
                }
                if before.is_some() && voxels.get_with_fluid(pos) != before {
                    set += 1;
                }
            }
        }
    }
    Ok(set)
}

/// Puts `amount` blocks of the fluid `id` in the voxel at `pos`.
pub(super) fn pour(
    In((pos, id, amount)): In<(IVec3, String, f32)>,
    content: Res<Content>,
    mut voxels: Voxels,
) -> Result<(), String> {
    let state = fill_state(&content, &id).map_err(|error| format!("pour: {error}"))?;
    if !content.state(state).is_fluid() {
        return Err(format!("pour: `{id}` isn't a fluid"));
    }
    let units = (amount * FULL_FLUID as f32).round();
    if !(1.0..=FULL_FLUID as f32).contains(&units) {
        return Err(format!(
            "pour: {amount} blocks is out of range; it must be over 0 and at most 1"
        ));
    }
    // Filling it first makes room in a block that leaves none, and
    // replaces any other fluid; then it holds as much as was asked, up to
    // the room its block leaves.
    voxels.set(pos, state);
    let room = voxels.fluid_amount(pos);
    voxels.set_fluid(pos, VoxelFluid::of(state, (units as u16).min(room)));
    Ok(())
}

/// Puts `amount` of `what` in the machine at `pos`: items in its slot
/// group `name`, or blocks of fluid in its tank `name`.
pub(super) fn put(
    In((pos, name, amount, what)): In<(IVec3, String, f32, String)>,
    content: Res<Content>,
    index: Res<BlockEntities>,
    mut machines: Query<(Option<&mut Slots>, Option<&mut Tanks>)>,
) -> Result<(), String> {
    let at = |error: String| format!("put at {} {} {}: {error}", pos.x, pos.y, pos.z);
    let (slots, tanks) = index
        .get(pos)
        .and_then(|entity| machines.get_mut(entity).ok())
        .ok_or_else(|| at("there's no machine here".into()))?;
    // A fluid goes in a tank, anything else in a slot group: a machine
    // may have both by one name (a generator's `fuel`).
    let fluid = fill_state(&content, &what)
        .ok()
        .filter(|&state| content.state(state).is_fluid());
    if let (Some(fluid), Some(tank)) = (
        fluid,
        tanks
            .map(|tanks| tanks.into_inner())
            .and_then(|tanks| tanks.get_mut(&name)),
    ) {
        let id = content.block_of(fluid).id.clone();
        let units = (amount * FULL_FLUID as f32).round();
        if units < 1.0 {
            return Err(at(format!("{amount} blocks is too little")));
        }
        let units = units as u64;
        if !tank.takes(&id) || tank.room() < units {
            return Err(at(format!(
                "{amount} blocks of `{id}` don't fit in `{name}`"
            )));
        }
        tank.fill(&id, units);
        return Ok(());
    }
    if let Some(group) = slots
        .map(|slots| slots.into_inner())
        .and_then(|slots| slots.group_mut(&name))
    {
        let item = item_state(&content, &what).map_err(at)?;
        if amount < 1.0 || amount.fract() != 0.0 {
            return Err(at(format!("{amount} isn't a whole number of items")));
        }
        let stack = ItemStack::new(item, amount as u32);
        let room = group.room_for(&content, &stack);
        if room < stack.count {
            return Err(at(format!("only {room} of `{what}` fit in `{name}`")));
        }
        group.insert(&content, stack);
        return Ok(());
    }
    Err(at(format!(
        "the machine has no slot group, or tank for `{what}`, called `{name}`"
    )))
}

/// Puts the item `name` in `player`'s hotbar slot `slot` and makes it the
/// one in hand.
pub(super) fn hold_item(
    In((player, slot, name)): In<(Entity, usize, String)>,
    content: Res<Content>,
    mut players: Query<(&mut Hotbar, &mut ActiveSlot)>,
) -> Result<(), String> {
    let item = item_state(&content, &name).map_err(|error| format!("hotbar: {error}"))?;
    let (mut hotbar, mut active) = players
        .get_mut(player)
        .map_err(|_| "hotbar: there's no player".to_string())?;
    hotbar.assign(slot, Some(item));
    *active = ActiveSlot(slot);
    Ok(())
}

/// Puts `count` of the item `name` in `player`'s inventory, each holding
/// `fluid` if it's given. Returns how many didn't fit.
pub(super) fn give_items(
    In((player, name, count, fluid)): In<(Entity, String, u32, Option<(String, f32)>)>,
    content: Res<Content>,
    mut players: Query<&mut Inventory>,
) -> Result<u32, String> {
    let item = item_state(&content, &name).map_err(|error| format!("give: {error}"))?;
    let data = match fluid {
        Some((fluid, amount)) => {
            let state = fill_state(&content, &fluid).map_err(|error| format!("give: {error}"))?;
            let id = &content.block_of(state).id;
            let units = (amount * FULL_FLUID as f32).round().max(0.0) as u64;
            fluid_container::saved_data(&content, item, id, units)
                .map_err(|error| format!("give: `{name}` can't hold that: {error}"))?
        }
        None => default(),
    };
    let mut inventory = players
        .get_mut(player)
        .map_err(|_| "give: there's no player".to_string())?;
    Ok(inventory
        .add(&content, ItemStack { item, data, count })
        .map_or(0, |left| left.count))
}

/// Puts `player`'s eyes at `eyes`, standing (and flying, so it stays
/// there). Its client learns it from the next move it's told of.
pub(super) fn teleport(
    In((player, eyes)): In<(Entity, Vec3)>,
    mut players: Query<(
        &mut Body,
        &mut Player,
        &mut Transform,
        Option<&mut crate::server::PlayerInputs>,
    )>,
) -> Result<(), String> {
    let (mut body, mut state, mut transform, inputs) = players
        .get_mut(player)
        .map_err(|_| "tp: there's no player".to_string())?;
    state.flying = true;
    state.eye_height = EYE_HEIGHT;
    state.previous_eye_height = EYE_HEIGHT;
    state.drawn_eye_height = EYE_HEIGHT;
    body.velocity = default();
    put_eyes(eyes.as_dvec3(), EYE_HEIGHT, &mut body, &mut transform);
    if let Some(mut inputs) = inputs {
        inputs.moved_now();
    }
    Ok(())
}
