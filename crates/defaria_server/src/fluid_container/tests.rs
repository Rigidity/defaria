use std::time::Duration;

use bevy::{ecs::system::RunSystemOnce, time::TimeUpdateStrategy};

use super::*;
use crate::inventory::player::{load, save};
use defaria_core::{
    block_entity::BlockEntityPart,
    content::{FULL_FLUID, base, block, test_mod},
    machine::Slots,
    streaming::StreamingSettings,
    world::VoxelFluid,
};
use defaria_core::{fluid_container::saved_data, inventory::player::new_player};

const WATER: &str = "defaria:water";
const LAVA: &str = "defaria:lava";
const FULL: u64 = FULL_FLUID as u64;

fn bucket() -> ItemState {
    base().default_item("defaria:bucket").unwrap()
}

/// `count` buckets holding `amount` of `fluid` each.
fn buckets(count: u32, fluid: &str, amount: u64) -> ItemStack {
    ItemStack {
        item: bucket(),
        data: ItemData::holding(fluid, amount),
        count,
    }
}

fn empty(count: u32) -> ItemStack {
    ItemStack::new(bucket(), count)
}

fn inventory(stacks: &[ItemStack]) -> Inventory {
    let mut inventory = Inventory::new();
    for stack in stacks {
        assert_eq!(inventory.add(base(), stack.clone()), None);
    }
    inventory
}

/// How many buckets holding `amount` of `fluid` (`0` for empty ones).
fn holding(inventory: &Inventory, fluid: &str, amount: u64) -> u32 {
    inventory.count_kind(&buckets(1, fluid, amount))
}

#[test]
fn a_bucket_holds_one_block_of_fluid() {
    assert_eq!(capacity(base(), bucket()), Some(FULL));
    assert_eq!(
        capacity(base(), base().default_item("defaria:stone").unwrap()),
        None
    );
    // A container has to hold something.
    let content = test_mod(&[(
        "items/cup.json5",
        "{ texture: \"a\", fluid_container: { capacity: 0 } }",
    )]);
    assert!(
        content
            .errors
            .iter()
            .any(|error| error.contains("fluid_container")),
        "{:?}",
        content.errors
    );
    let content = test_mod(&[(
        "items/cup.json5",
        "{ texture: \"a\", fluid_container: { capacity: 0.25 } }",
    )]);
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    let cup = content.default_item("test:cup").unwrap();
    assert_eq!(capacity(&content, cup), Some(250));
}

/// Filled buckets count together only with ones holding the same fluid and
/// amount; empty ones together.
#[test]
fn buckets_stack_only_with_the_same_fluid_and_amount() {
    let inventory = inventory(&[
        empty(2),
        buckets(1, WATER, FULL),
        buckets(2, WATER, FULL),
        buckets(1, WATER, 500),
        buckets(1, LAVA, FULL),
        empty(1),
    ]);
    assert_eq!(inventory.entries().len(), 4);
    assert_eq!(holding(&inventory, WATER, 0), 3);
    assert_eq!(holding(&inventory, WATER, FULL), 3);
    assert_eq!(holding(&inventory, WATER, 500), 1);
    assert_eq!(holding(&inventory, LAVA, FULL), 1);
    assert_eq!(inventory.count(bucket()), 8);
    // Holding nothing is the same as empty.
    assert_eq!(ItemData::holding(WATER, 0), ItemData::default());
}

/// The fluid adds its weight: a block of water a tonne, of lava 3.1.
#[test]
fn the_fluid_counts_toward_the_weight() {
    let each = defaria_core::inventory::model::item_weight(base(), bucket());
    assert!(each > 0.0);
    assert_eq!(empty(2).weight(base()), 2.0 * each);
    assert_eq!(buckets(1, WATER, FULL).weight(base()), each + 1000.0);
    assert_eq!(buckets(2, WATER, 500).weight(base()), 2.0 * (each + 500.0));
    assert_eq!(buckets(1, LAVA, FULL).weight(base()), each + 3100.0);
    // A limit lets in only what it can carry.
    let mut inventory = inventory(&[empty(1)]).with_max_weight(each + 1500.0);
    assert_eq!(inventory.room_for(base(), &buckets(1, WATER, FULL)), 1);
    assert_eq!(inventory.room_for(base(), &empty(1)), 1000);
    assert_eq!(weight_room(base(), &inventory, WATER), 1500);
    assert_eq!(weight_room(base(), &Inventory::new(), WATER), u64::MAX);
    // Swapping the empty one for a full one goes over nothing.
    assert!(inventory.replace_data(base(), &empty(1), ItemData::holding(WATER, 1500)));
    assert_eq!(holding(&inventory, WATER, 1500), 1);
    assert_eq!(weight_room(base(), &inventory, WATER), 0);
}

/// Taking tops up a part-filled bucket of the same fluid before using an
/// empty one; pouring uses the fullest that fits.
#[test]
fn which_bucket_takes_and_which_pours() {
    let mixed = inventory(&[
        empty(3),
        buckets(1, WATER, 200),
        buckets(1, WATER, 700),
        buckets(1, WATER, FULL),
        buckets(1, LAVA, 400),
    ]);
    let taker_of = |fluid| taker(&mixed, bucket(), FULL, fluid).map(|kind| kind.data);
    assert_eq!(taker_of(WATER), Some(ItemData::holding(WATER, 700)));
    assert_eq!(taker_of(LAVA), Some(ItemData::holding(LAVA, 400)));
    let full = inventory(&[buckets(2, WATER, FULL)]);
    assert_eq!(taker(&full, bucket(), FULL, WATER), None);
    assert_eq!(
        taker(&full, bucket(), FULL, LAVA),
        None,
        "nothing to take lava in"
    );
    let pourer_of = |fluid: &'static str| {
        pourer(&mixed, bucket(), |held| held.fluid == fluid).map(|kind| kind.data)
    };
    assert_eq!(pourer_of(WATER), Some(ItemData::holding(WATER, FULL)));
    assert_eq!(pourer_of(LAVA), Some(ItemData::holding(LAVA, 400)));
    assert_eq!(pourer(&inventory(&[empty(4)]), bucket(), |_| true), None);
}

/// The fluid in a bucket is saved with the inventory, and with a machine's
/// slots; fluid that can't be held any more is dropped with a warning.
#[test]
fn the_fluid_is_saved() {
    let content = base();
    let (mut inventory, hotbar) = new_player(content);
    inventory.add(content, buckets(2, WATER, 640));
    inventory.add(content, empty(1));
    let active = defaria_core::hotbar::ActiveSlot(0);
    let mode = defaria_core::game_mode::GameMode::Survival;
    let saved = save(&inventory, &hotbar, active, mode, content);
    let text = json5::to_string(&saved).unwrap();
    assert!(text.contains("defaria:water"), "{text}");
    let read: crate::save::player::PlayerFile = json5::from_str(&text).unwrap();
    assert_eq!(load(&read, content).0, inventory);

    // More than it holds, or not a fluid: an empty bucket.
    assert!(saved_data(content, bucket(), WATER, FULL + 1).is_err());
    assert!(saved_data(content, bucket(), "defaria:stone", 10).is_err());
    let stone = content.default_item("defaria:stone").unwrap();
    assert!(saved_data(content, stone, WATER, 10).is_err());
    let mut bad = saved.clone();
    for stack in &mut bad.inventory {
        if let Some(fluid) = &mut stack.fluid {
            fluid.amount = FULL * 2;
        }
    }
    let loaded = load(&bad, content).0;
    assert_eq!(holding(&loaded, WATER, 0), 3);

    // A machine's slots keep it too.
    let crate_state = block("crate");
    let mut slots = Slots::new(crate_state, content).unwrap();
    slots.0[0].slots[0] = Some(buckets(3, LAVA, FULL));
    let value = slots.save(content);
    let back = Slots::load(&value, Slots::VERSION, crate_state, content).unwrap();
    assert_eq!(back.0[0].slots[0], Some(buckets(3, LAVA, FULL)));
    // More than a bucket holds: the buckets load empty.
    let defaria_core::block_entity::Value::Table(mut groups) = value else {
        panic!("slots save as a table");
    };
    for group in groups.values_mut() {
        let defaria_core::block_entity::Value::List(slots) = group else {
            panic!("a group saves as a list");
        };
        if let defaria_core::block_entity::Value::Table(slot) = &mut slots[0] {
            slot.insert(
                "amount".into(),
                defaria_core::block_entity::Value::Int(5000),
            );
        }
    }
    let value = defaria_core::block_entity::Value::Table(groups);
    let back = Slots::load(&value, Slots::VERSION, crate_state, content).unwrap();
    assert_eq!(back.0[0].slots[0], Some(empty(3)));
}

/// A container in hand aims at fluid as high as it is, at least a
/// sixteenth of a block.
#[test]
fn the_crosshair_picks_fluid_by_its_height() {
    let water = |amount| VoxelFluid::of(block("water"), amount);
    let height = |state, fluid| {
        defaria_core::block_edit::fluid_shape(
            base(),
            state,
            fluid,
            &defaria_core::content::Around::AIR,
        )[0]
        .max
        .y
    };
    let air = BlockState::AIR;
    assert_eq!(height(air, water(FULL_FLUID)), 1.0);
    assert_eq!(height(air, water(500)), 0.5);
    assert_eq!(height(air, water(1)), 1.0 / 16.0);
    // Up the room a slab leaves: its half full reaches the top.
    assert_eq!(height(block("stone_slab"), water(500)), 1.0);
    assert_eq!(height(block("stone_slab"), water(250)), 0.75);
    let air = &defaria_core::content::Around::AIR;
    assert!(defaria_core::block_edit::fluid_shape(base(), BlockState::AIR, None, air).is_empty());
    assert_eq!(
        defaria_core::block_edit::fluid_shape(base(), block("stone"), None, air),
        defaria_core::block_edit::target_shape(base(), block("stone"), air)
    );
}

// The world, with no flow, so what's taken and poured stays put.

const HOME: Vec3 = Vec3::new(8.0, 60.0, 8.0);
const Y: i32 = 60;

fn world_app() -> App {
    crate::testing::world_app(base().clone())
        .resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            50,
        )))
        .resource(StreamingSettings {
            radius: 1,
            above: 1,
            below: 1,
            unload_margin: 0,
            ..default()
        })
        .home(HOME)
        .build()
}

/// Sets a voxel to `state`: a fluid fills it, with `amount` of it if
/// that's given (`None` for a full block); anything else leaves no fluid.
fn put(app: &mut App, pos: IVec3, state: BlockState, amount: Option<u16>) {
    app.world_mut()
        .run_system_once(move |mut voxels: Voxels, content: Res<Content>| {
            assert!(voxels.set(pos, state));
            if !content.state(state).is_fluid() {
                assert!(voxels.set_fluid(pos, None));
            } else if let Some(amount) = amount {
                assert!(voxels.set_fluid(pos, VoxelFluid::of(state, amount)));
            }
        })
        .unwrap();
}

/// What's at a voxel as a block per voxel: its fluid where its block is
/// air, with its amount if it isn't a full block, or else its block.
fn at(app: &mut App, pos: IVec3) -> (BlockState, Option<u16>) {
    app.world_mut()
        .run_system_once(
            move |voxels: Voxels| match voxels.get_with_fluid(pos).unwrap() {
                (BlockState::AIR, Some(fluid)) => (
                    fluid.kind,
                    (fluid.amount < FULL_FLUID).then_some(fluid.amount),
                ),
                (block, _) => (block, None),
            },
        )
        .unwrap()
}

/// The block at a voxel and how much fluid it holds.
fn held_at(app: &mut App, pos: IVec3) -> (BlockState, u16) {
    app.world_mut()
        .run_system_once(move |voxels: Voxels| (voxels.get(pos).unwrap(), voxels.fluid_amount(pos)))
        .unwrap()
}

/// Runs `act` on the world and `inventory`, and returns what it returned.
fn with_world(
    app: &mut App,
    inventory: &mut Inventory,
    act: fn(&Content, &mut Voxels, &mut Inventory) -> bool,
) -> bool {
    let mut held = std::mem::take(inventory);
    let (moved, held) = app
        .world_mut()
        .run_system_once(move |content: Res<Content>, mut voxels: Voxels| {
            let moved = act(&content, &mut voxels, &mut held);
            (moved, std::mem::take(&mut held))
        })
        .unwrap();
    *inventory = held;
    moved
}

const A: IVec3 = IVec3::new(2, Y, 2);
const B: IVec3 = IVec3::new(4, Y, 2);

/// Takes the fluid at `pos` into a bucket that can take it, as a click
/// on it does.
fn take_at(content: &Content, voxels: &mut Voxels, inventory: &mut Inventory, pos: IVec3) -> bool {
    let Some((_, Some(here))) = voxels.get_with_fluid(pos) else {
        return false;
    };
    let fluid = &content.block_of(here.kind).id;
    let Some(kind) = taker(inventory, bucket(), FULL, fluid) else {
        return false;
    };
    take_from_world(content, voxels, inventory, &kind, FULL, pos) > 0
}

fn take_a(content: &Content, voxels: &mut Voxels, inventory: &mut Inventory) -> bool {
    take_at(content, voxels, inventory, A)
}

fn take_b(content: &Content, voxels: &mut Voxels, inventory: &mut Inventory) -> bool {
    take_at(content, voxels, inventory, B)
}

fn pour_a(content: &Content, voxels: &mut Voxels, inventory: &mut Inventory) -> bool {
    pour_into_world(content, voxels, inventory, bucket(), FULL, &[A], None)
}

fn pour_b(content: &Content, voxels: &mut Voxels, inventory: &mut Inventory) -> bool {
    pour_into_world(content, voxels, inventory, bucket(), FULL, &[B], None)
}

/// From a full voxel, a bucket takes it all and leaves air.
#[test]
fn taking_a_full_block() {
    let mut app = world_app();
    put(&mut app, A, block("water"), None);
    let mut inventory = inventory(&[empty(2)]);
    assert!(with_world(&mut app, &mut inventory, take_a));
    assert_eq!(at(&mut app, A), (BlockState::AIR, None));
    assert_eq!(holding(&inventory, WATER, FULL), 1);
    assert_eq!(holding(&inventory, WATER, 0), 1);
    // Nothing left to take.
    assert!(!with_world(&mut app, &mut inventory, take_a));
}

/// From part-full voxels, a bucket takes what's there, and tops up to full
/// from the next, leaving the rest.
#[test]
fn taking_from_part_full_voxels() {
    let mut app = world_app();
    let water = block("water");
    put(&mut app, A, water, Some(300));
    put(&mut app, B, water, Some(900));
    let mut inventory = inventory(&[empty(1)]);
    assert!(with_world(&mut app, &mut inventory, take_a));
    assert_eq!(at(&mut app, A), (BlockState::AIR, None));
    assert_eq!(holding(&inventory, WATER, 300), 1);
    assert!(with_world(&mut app, &mut inventory, take_b));
    assert_eq!(at(&mut app, B), (water, Some(200)));
    assert_eq!(holding(&inventory, WATER, FULL), 1);
    // Full, and no other bucket: it takes no more.
    assert!(!with_world(&mut app, &mut inventory, take_b));
    // Nor lava, into a bucket of water.
    put(&mut app, A, block("lava"), None);
    assert!(!with_world(&mut app, &mut inventory, take_a));
}

/// Into air a whole bucket pours; onto a part-full voxel only what fits,
/// the rest staying in the bucket; onto a full one or another fluid,
/// nothing.
#[test]
fn pouring_with_and_without_room() {
    let mut app = world_app();
    let water = block("water");
    put(&mut app, A, BlockState::AIR, None);
    put(&mut app, B, water, Some(600));
    let mut inventory = inventory(&[buckets(2, WATER, FULL)]);
    assert!(with_world(&mut app, &mut inventory, pour_a));
    // A full voxel keeps no amount apart.
    assert_eq!(at(&mut app, A), (water, None));
    assert_eq!(holding(&inventory, WATER, 0), 1);
    assert!(with_world(&mut app, &mut inventory, pour_b));
    assert_eq!(at(&mut app, B), (water, None));
    assert_eq!(holding(&inventory, WATER, 600), 1);
    // No room left: nothing moves.
    assert!(!with_world(&mut app, &mut inventory, pour_b));
    assert_eq!(holding(&inventory, WATER, 600), 1);
    put(&mut app, B, block("lava"), Some(500));
    assert!(!with_world(&mut app, &mut inventory, pour_b));
    put(&mut app, B, block("stone"), None);
    assert!(!with_world(&mut app, &mut inventory, pour_b));
    assert_eq!(holding(&inventory, WATER, 600), 1);
}

/// A bucket pours into the room a slab in the world leaves it, no more,
/// and takes the water back out, the slab staying put: none made or lost.
#[test]
fn buckets_fill_and_empty_a_slabs_room() {
    let mut app = world_app();
    let slab = block("stone_slab");
    put(&mut app, A, slab, None);
    let mut inventory = inventory(&[buckets(1, WATER, FULL)]);
    assert!(with_world(&mut app, &mut inventory, pour_a));
    assert_eq!(held_at(&mut app, A), (slab, 500));
    assert_eq!(holding(&inventory, WATER, 500), 1);
    // Full: nothing more goes in.
    assert!(!with_world(&mut app, &mut inventory, pour_a));
    assert!(with_world(&mut app, &mut inventory, take_a));
    assert_eq!(held_at(&mut app, A), (slab, 0));
    assert_eq!(holding(&inventory, WATER, FULL), 1);
}

/// Taking and pouring back, over and over, keeps every drop: what's in
/// the voxels and the buckets adds up the same.
#[test]
fn a_round_trip_keeps_the_total() {
    let mut app = world_app();
    let water = block("water");
    put(&mut app, A, water, Some(700));
    put(&mut app, B, water, Some(450));
    let mut inventory = inventory(&[empty(1), buckets(1, WATER, 250)]);
    let total = |app: &mut App, inventory: &Inventory| {
        let in_world: u64 = [A, B]
            .into_iter()
            .map(|pos| match at(app, pos) {
                (state, amount) if state == water => amount.unwrap_or(FULL_FLUID) as u64,
                _ => 0,
            })
            .sum();
        let held: u64 = inventory
            .entries()
            .iter()
            .filter_map(|stack| Some(stack.data.fluid.as_ref()?.amount * stack.count as u64))
            .sum();
        in_world + held
    };
    let start = total(&mut app, &inventory);
    assert_eq!(start, 1400);
    let steps = [
        take_a, pour_b, take_b, take_a, pour_a, take_b, pour_b, pour_a, take_a,
    ];
    for step in steps {
        with_world(&mut app, &mut inventory, step);
        assert_eq!(total(&mut app, &inventory), start);
    }
    assert_eq!(inventory.count(bucket()), 2);
}

/// A click (no Shift) at a voxel from above, as `use_container` makes it
/// in the world.
fn click(app: &mut App, inventory: &mut Inventory, pos: IVec3, pour_only: bool) -> bool {
    let mut held = std::mem::take(inventory);
    let hit = Hit {
        block: pos,
        normal: IVec3::Y,
        shape_box: 0,
        at: Vec3::new(0.5, 1.0, 0.5),
    };
    let (taken, held) = app
        .world_mut()
        .run_system_once(move |content: Res<Content>, mut voxels: Voxels| {
            let taken = use_on_world(
                &content,
                &mut voxels,
                &mut held,
                bucket(),
                FULL,
                hit,
                pour_only,
                |state| content.machine(state).is_some_and(has_screen),
            );
            (taken, std::mem::take(&mut held))
        })
        .unwrap();
    *inventory = held;
    taken
}

/// Under a weight limit, a bucket takes only what the limit lets in, from
/// the world and from a tank; at the limit, a click on fluid it could take
/// is still the bucket's, and pours nothing out instead.
#[test]
fn taking_stops_at_the_weight_limit() {
    let mut app = world_app();
    let water = block("water");
    put(&mut app, A, water, None);
    let each = defaria_core::inventory::model::item_weight(base(), bucket());
    let mut inventory = inventory(&[empty(1), buckets(1, WATER, FULL)])
        .with_max_weight(2.0 * each + 1000.0 + 400.0);
    assert!(click(&mut app, &mut inventory, A, false));
    assert_eq!(at(&mut app, A), (water, Some(600)));
    assert_eq!(holding(&inventory, WATER, 400), 1);
    // At the limit: taken, but nothing moves, and nothing is poured.
    assert!(click(&mut app, &mut inventory, A, false));
    assert_eq!(at(&mut app, A), (water, Some(600)));
    assert_eq!(holding(&inventory, WATER, 400), 1);
    assert_eq!(holding(&inventory, WATER, FULL), 1);

    // A tank: the same.
    let mut tanks = Tanks(vec![Tank {
        name: "fluid".into(),
        fluid: Some(WATER.into()),
        amount: 3000,
        capacity: 4000,
        accepts: Vec::new(),
    }]);
    let mut held = self::inventory(&[empty(1)]).with_max_weight(each + 250.0);
    assert!(use_on_tanks(
        base(),
        &mut held,
        bucket(),
        FULL,
        &mut tanks,
        false
    ));
    assert_eq!(tanks.0[0].amount, 2750);
    assert_eq!(holding(&held, WATER, 250), 1);
    assert!(use_on_tanks(
        base(),
        &mut held,
        bucket(),
        FULL,
        &mut tanks,
        false
    ));
    assert_eq!(tanks.0[0].amount, 2750);
}

/// A tank that's full, or doesn't take the fluid, moves nothing either
/// way, so the click isn't the bucket's.
#[test]
fn a_tank_that_takes_nothing_moves_nothing() {
    let tank = |fluid: Option<&str>, amount, accepts: &[&str]| Tank {
        name: "fluid".into(),
        fluid: fluid.map(str::to_owned),
        amount,
        capacity: 4000,
        accepts: accepts.iter().map(|id| id.to_string()).collect(),
    };
    for (mut tanks, shift) in [
        (Tanks(vec![tank(Some(WATER), 4000, &[])]), true),
        (Tanks(vec![tank(None, 0, &[LAVA])]), false),
    ] {
        let before = tanks.clone();
        let mut held = inventory(&[buckets(2, WATER, FULL)]);
        let start = held.clone();
        assert!(!use_on_tanks(
            base(),
            &mut held,
            bucket(),
            FULL,
            &mut tanks,
            shift
        ));
        assert_eq!((tanks, held), (before, start));
    }
}

/// Aimed at a pond with buckets of water and of lava, pouring pours water
/// (into the voxel, or in front of it when it's full), never lava.
#[test]
fn pouring_onto_fluid_pours_only_that_fluid() {
    let mut app = world_app();
    let water = block("water");
    let above = A + IVec3::Y;
    put(&mut app, A, water, None);
    put(&mut app, above, BlockState::AIR, None);
    let mut inventory = inventory(&[buckets(1, LAVA, FULL), buckets(1, WATER, 500)]);
    // Shift: a pour, into the air above the full voxel.
    assert!(click(&mut app, &mut inventory, A, true));
    assert_eq!(at(&mut app, above), (water, Some(500)));
    assert_eq!(holding(&inventory, LAVA, FULL), 1);
    assert_eq!(holding(&inventory, WATER, 0), 1);
    // No water left to pour: nothing, not lava.
    assert!(!click(&mut app, &mut inventory, A, true));
    assert_eq!(holding(&inventory, LAVA, FULL), 1);

    // Onto a nearly full voxel, what doesn't fit goes in front of it, in
    // the same click.
    put(&mut app, B, water, Some(900));
    put(&mut app, B + IVec3::Y, BlockState::AIR, None);
    let mut inventory = self::inventory(&[buckets(1, WATER, FULL)]);
    assert!(click(&mut app, &mut inventory, B, true));
    assert_eq!(at(&mut app, B), (water, None));
    assert_eq!(at(&mut app, B + IVec3::Y), (water, Some(900)));
    assert_eq!(holding(&inventory, WATER, 0), 1);
}

/// On a block that does something when used, a click with a full bucket
/// is passed on, unless Shift is held; on a plain block it pours in front.
#[test]
fn a_usable_block_gets_the_click() {
    let mut app = world_app();
    let crate_pos = A;
    put(&mut app, crate_pos, block("crate"), None);
    put(&mut app, crate_pos + IVec3::Y, BlockState::AIR, None);
    put(&mut app, B, block("stone"), None);
    put(&mut app, B + IVec3::Y, BlockState::AIR, None);
    let mut inventory = inventory(&[buckets(2, WATER, FULL)]);
    assert!(!click(&mut app, &mut inventory, crate_pos, false));
    assert_eq!(holding(&inventory, WATER, FULL), 2);
    assert!(click(&mut app, &mut inventory, crate_pos, true));
    assert_eq!(at(&mut app, crate_pos + IVec3::Y), (block("water"), None));
    assert!(click(&mut app, &mut inventory, B, false));
    assert_eq!(at(&mut app, B + IVec3::Y), (block("water"), None));
    assert_eq!(holding(&inventory, WATER, 0), 2);
    // A machine with no screen (a battery) is a plain block to pour on.
    let battery = IVec3::new(6, Y, 2);
    put(&mut app, battery, block("battery"), None);
    put(&mut app, battery + IVec3::Y, BlockState::AIR, None);
    let mut inventory = self::inventory(&[buckets(1, WATER, FULL)]);
    assert!(click(&mut app, &mut inventory, battery, false));
    assert_eq!(at(&mut app, battery + IVec3::Y), (block("water"), None));
}
