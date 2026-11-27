//! Fluid containers (the bucket) in the game.

use bevy::{ecs::system::RunSystemOnce, prelude::*};
use defaria_client::ui::{
    SlotFluid,
    slot::{hotbar_fluid, slot_fluid},
};
use defaria_core::{
    block_entity::BlockEntities,
    content::{Content, FULL_FLUID, ItemState, base, block},
    inventory::{Inventory, ItemStack, model::ItemData},
    machine::Tanks,
    player::LocalPlayer,
    world::Voxels,
};

use crate::headless::{Script, run_more, settle, test_game};

const WATER: &str = "defaria:water";
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

// Through the real game: clicks with a bucket in hand.

/// A stone basin at x 0 to 4, z 0 to 4, its floor at `FLOOR`, the inside
/// (x and z 1 to 3) one deep. `pond` fills it with water.
const FLOOR: i32 = 70;

fn basin(pond: bool) -> String {
    let inside = if pond { "water" } else { "air" };
    format!(
        "fill -2 {f} -2 6 {top} 6 air\n\
         fill 0 {f} 0 4 {g} 4 stone\n\
         fill 1 {g} 1 3 {g} 3 {inside}\n\
         wait 2\n",
        f = FLOOR,
        g = FLOOR + 1,
        top = FLOOR + 6,
    )
}

fn game(script: &str) -> App {
    // Loaded around the basin first.
    let start = format!("camera 2.5 {} 2.5 0 -89\nsettle\nwait 1", FLOOR + 4);
    let mut app = test_game(Script::parse(&start).unwrap());
    settle(&mut app);
    run_more(&mut app, script);
    app
}

fn player_inventory(app: &mut App) -> Inventory {
    let mut players = app
        .world_mut()
        .query_filtered::<&Inventory, With<LocalPlayer>>();
    players.single(app.world()).unwrap().clone()
}

fn give(app: &mut App, stack: ItemStack) {
    let mut players = app
        .world_mut()
        .query_filtered::<&mut Inventory, With<LocalPlayer>>();
    players
        .single_mut(app.world_mut())
        .unwrap()
        .add(base(), stack);
}

fn water_in(app: &mut App, from: IVec3, to: IVec3) -> u64 {
    app.world_mut()
        .run_system_once(move |voxels: Voxels, content: Res<Content>| {
            let mut total = 0;
            for x in from.x..=to.x {
                for y in from.y..=to.y {
                    for z in from.z..=to.z {
                        if let Some((_, Some(fluid))) = voxels.get_with_fluid(IVec3::new(x, y, z))
                            && content.block_of(fluid.kind).id == WATER
                        {
                            total += fluid.amount as u64;
                        }
                    }
                }
            }
            total
        })
        .unwrap()
}

/// The hotbar's slot naming the bucket shows what all the buckets hold.
#[test]
fn the_hotbar_shows_its_buckets_fluid() {
    let mut app = game("give bucket 1 water 1\ngive bucket 1\nhotbar 1 bucket\nwait 2\n");
    let shown: Vec<defaria_client::ui::ItemSlot> = app
        .world_mut()
        .query::<&defaria_client::ui::ItemSlot>()
        .iter(app.world())
        .copied()
        .filter(|slot| slot.item == Some(bucket()))
        .collect();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(
        shown[0].fluid,
        Some(SlotFluid {
            fluid: Some(block("water")),
            amount: FULL,
            capacity: 2 * FULL,
        })
    );
}

/// The headless `give` command puts empty and filled containers in the
/// player's inventory, and fails for fluid in something that can't hold
/// it.
#[test]
fn give_puts_buckets_in_the_inventory() {
    let mut app = game("give bucket 2\ngive bucket 3 water 0.5\nwait 1\n");
    let inventory = player_inventory(&mut app);
    assert_eq!(holding(&inventory, WATER, 0), 2);
    assert_eq!(holding(&inventory, WATER, 500), 3);
    let mut app = test_game(Script::parse("give stone 1 water 1\nwait 1").unwrap());
    let exit = (0..crate::headless::SETTLE_TIMEOUT_FRAMES)
        .find_map(|_| {
            app.update();
            app.should_exit()
        })
        .unwrap();
    assert!(exit.is_error());
}

/// Looking straight down from above the middle of the basin.
fn look_down() -> String {
    format!("camera 2.5 {} 2.5 0 -89\nwait 1\n", FLOOR + 4)
}

/// Right click on a pond with a bucket in hand takes a block of it;
/// with Shift, the same click pours back.
#[test]
fn a_bucket_takes_from_a_pond_and_pours_back() {
    let mut app = game(&basin(true));
    give(&mut app, empty(1));
    let pond =
        |app: &mut App| water_in(app, IVec3::new(-2, FLOOR, -2), IVec3::new(6, FLOOR + 6, 6));
    let start = pond(&mut app);
    assert_eq!(start, 9 * FULL);
    run_more(
        &mut app,
        &format!(
            "{}hotbar 1 bucket\nhold MouseLeft 1\nwait 1\nhold MouseRight 1\nwait 1\n",
            look_down()
        ),
    );
    let inventory = player_inventory(&mut app);
    assert_eq!(holding(&inventory, WATER, FULL), 1, "{inventory:?}");
    assert_eq!(pond(&mut app), start - FULL);
    // Shift pours it back, into the voxel aimed at, which flow has been
    // filling in again: as much as fits there, the rest staying in the
    // bucket. Not a drop is made or lost.
    run_more(
        &mut app,
        "press ShiftLeft\nhold MouseRight 1\nwait 1\nrelease ShiftLeft\nwait 1\n",
    );
    let inventory = player_inventory(&mut app);
    let held: u64 = inventory
        .entries()
        .iter()
        .filter_map(|stack| stack.data.fluid.as_ref().map(|held| held.amount))
        .sum();
    assert!(held < FULL, "{inventory:?}");
    assert_eq!(pond(&mut app) + held, start);
}

/// Aimed at a dry pit's floor, a bucket pours its water onto it, though
/// there's an empty bucket to hand: there's nothing to take.
#[test]
fn a_bucket_pours_into_a_pit() {
    let mut app = game(&basin(false));
    give(&mut app, buckets(1, WATER, FULL));
    give(&mut app, empty(1));
    let pit = (IVec3::new(1, FLOOR + 1, 1), IVec3::new(3, FLOOR + 4, 3));
    assert_eq!(water_in(&mut app, pit.0, pit.1), 0);
    run_more(
        &mut app,
        &format!(
            "{}hotbar 1 bucket\nhold MouseLeft 1\nwait 1\nhold MouseRight 1\nwait 1\n",
            look_down()
        ),
    );
    let inventory = player_inventory(&mut app);
    assert_eq!(holding(&inventory, WATER, 0), 2, "{inventory:?}");
    assert_eq!(water_in(&mut app, pit.0, pit.1), FULL);
}

/// On a machine's tank, a bucket pours in what the tank takes (with
/// Shift, even when an empty bucket could take), and takes back out with
/// an empty one.
#[test]
fn a_bucket_fills_and_empties_a_machine_tank() {
    let tank = IVec3::new(2, FLOOR + 1, 2);
    let mut app = game(&format!(
        "{}fill 2 {y} 2 2 {y} 2 tank\nwait 2\n",
        basin(false),
        y = FLOOR + 1
    ));
    give(&mut app, buckets(1, WATER, FULL));
    let tank_water = |app: &mut App| {
        let entity = app.world().resource::<BlockEntities>().get(tank).unwrap();
        app.world().get::<Tanks>(entity).unwrap().total(WATER)
    };
    let click = format!(
        "{}hotbar 1 bucket\nhold MouseLeft 1\nwait 1\nhold MouseRight 1\nwait 1\n",
        look_down()
    );
    run_more(&mut app, &click);
    assert_eq!(tank_water(&mut app), FULL);
    assert_eq!(holding(&player_inventory(&mut app), WATER, 0), 1);
    // With Shift, a full bucket pours in, though an empty one could take.
    give(&mut app, buckets(1, WATER, FULL));
    run_more(
        &mut app,
        "press ShiftLeft\nhold MouseRight 1\nwait 1\nrelease ShiftLeft\nwait 1\n",
    );
    assert_eq!(tank_water(&mut app), 2 * FULL);
    assert_eq!(holding(&player_inventory(&mut app), WATER, 0), 2);
    // Without, an empty bucket takes some back out.
    run_more(&mut app, "hold MouseRight 1\nwait 1\n");
    assert_eq!(tank_water(&mut app), FULL);
    let inventory = player_inventory(&mut app);
    assert_eq!(holding(&inventory, WATER, FULL), 1);
    assert_eq!(holding(&inventory, WATER, 0), 1);
}

/// With nothing to move (an empty tank, empty buckets), the click goes on
/// to the machine and its screen opens; with the cursor free, a click
/// does nothing.
#[test]
fn a_click_that_moves_nothing_opens_the_machine() {
    let mut app = game(&format!(
        "{}fill 2 {y} 2 2 {y} 2 tank\nwait 2\n",
        basin(false),
        y = FLOOR + 1
    ));
    give(&mut app, empty(1));
    let screens = |app: &mut App| {
        app.world_mut()
            .query::<&defaria_client::machine_screen::MachineScreen>()
            .iter(app.world())
            .count()
    };
    // The cursor isn't locked: nothing happens.
    run_more(
        &mut app,
        &format!(
            "{}hotbar 1 bucket\nhold MouseRight 1\nwait 2\n",
            look_down()
        ),
    );
    assert_eq!(screens(&mut app), 0);
    run_more(
        &mut app,
        "hold MouseLeft 1\nwait 1\nhold MouseRight 1\nwait 2\n",
    );
    assert_eq!(screens(&mut app), 1);
    assert_eq!(holding(&player_inventory(&mut app), WATER, 0), 1);
}

/// A water block's item places only in creative: in survival it would
/// make water from nothing.
#[test]
fn fluid_blocks_place_only_in_creative() {
    let mut app = game(&basin(false));
    let pit = (IVec3::new(1, FLOOR + 1, 1), IVec3::new(3, FLOOR + 4, 3));
    let water_item = base().default_item(WATER).unwrap();
    give(&mut app, ItemStack::new(water_item, 1));
    // Survival (C, with the cursor locked to play).
    run_more(
        &mut app,
        &format!(
            "{}hotbar 1 water\nhold MouseLeft 1\nwait 1\nhold KeyC 1\nwait 1\nhold MouseRight 1\nwait 2\n",
            look_down()
        ),
    );
    assert_eq!(water_in(&mut app, pit.0, pit.1), 0);
    assert_eq!(player_inventory(&mut app).count(water_item), 1);
    // Back in creative, it places.
    run_more(&mut app, "hold KeyC 1\nwait 1\nhold MouseRight 1\nwait 2\n");
    assert_eq!(water_in(&mut app, pit.0, pit.1), FULL);
}

/// What slots show of buckets: each stack's own fluid, and the hotbar's
/// all its buckets together.
#[test]
fn slots_show_what_buckets_hold() {
    let content = base();
    let water = block("water");
    assert_eq!(
        slot_fluid(content, &buckets(4, WATER, 300)),
        Some(SlotFluid {
            fluid: Some(water),
            amount: 300,
            capacity: FULL,
        })
    );
    assert_eq!(
        slot_fluid(content, &empty(2)),
        Some(SlotFluid {
            fluid: None,
            amount: 0,
            capacity: FULL,
        })
    );
    let stone = content.default_item("defaria:stone").unwrap();
    assert_eq!(slot_fluid(content, &ItemStack::one(stone)), None);
    let mixed = inventory(&[empty(1), buckets(2, WATER, 500), ItemStack::one(stone)]);
    assert_eq!(
        hotbar_fluid(content, &mixed, bucket()),
        Some(SlotFluid {
            fluid: Some(water),
            amount: 1000,
            capacity: 3 * FULL,
        })
    );
    assert_eq!(hotbar_fluid(content, &mixed, stone), None);
    assert_eq!(
        hotbar_fluid(content, &Inventory::new(), bucket()),
        Some(SlotFluid {
            capacity: 0,
            ..default()
        })
    );
}
