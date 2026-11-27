//! The machine screen in the game: using a machine, moving items, and
//! how the screen is laid out.

use bevy::prelude::*;
use defaria_client::machine_screen::*;
use defaria_client::{
    inventory_screen::{list::ListEntry, screen::INVENTORY_ROWS},
    ui::{Bar, ItemSlot, Tooltip, UiTheme},
};
use defaria_core::{
    block_entity::BlockEntities,
    content::{BlockState, Content},
    inventory::Inventory,
    machine::{Slots, has_screen},
    player::LocalPlayer,
    recipe::{RecipeProgress, RecipeStatus},
    world::Voxels,
};

use bevy::{
    ui::UiGlobalTransform,
    window::{CursorGrabMode, CursorOptions},
};

use defaria_client::inventory_screen::screen::InventoryScreen;
use defaria_core::{
    content::ItemState,
    inventory::ItemStack,
    testing::{test_content, test_mods},
};

use crate::headless::{Script, run_more, settle, test_game};

/// Where the furnace goes: three blocks in front of the camera.
const FURNACE: IVec3 = IVec3::new(64, 70, 89);

/// A game with an unpowered electric furnace in front of the camera, the
/// cursor locked, and 64 iron ore on the player as well as its stone.
fn game() -> App {
    let mut app = test_game(Script::parse("camera 64.5 70.5 92.5 0 0\nwait 2").unwrap());
    settle(&mut app);
    run_more(
        &mut app,
        "fill 60 66 84 68 74 92 air\n\
         fill 64 70 89 64 70 89 electric_furnace[facing=south]\n\
         wait 2\n\
         hold MouseLeft 1\n\
         wait 1",
    );
    assert_eq!(cursor(&mut app), CursorGrabMode::Locked);
    let ore = ore(&app);
    let content = app.world().resource::<Content>().clone();
    let server = crate::headless::server(&mut app);
    let mut players = server.query_filtered::<&mut Inventory, With<defaria_core::player::Player>>();
    let mut inventory = players.single_mut(server).unwrap();
    assert!(inventory.add(&content, ItemStack::new(ore, 64)).is_none());
    run_more(&mut app, "wait 2");
    app
}

fn open(app: &mut App) {
    run_more(app, "hold MouseRight 1\nwait 3");
    assert!(is_open(app), "the screen didn't open");
}

fn is_open(app: &mut App) -> bool {
    let mut screens = app.world_mut().query::<&MachineScreen>();
    screens.iter(app.world()).count() > 0
}

fn cursor(app: &mut App) -> CursorGrabMode {
    let mut cursors = app.world_mut().query::<&CursorOptions>();
    cursors.single(app.world()).unwrap().grab_mode
}

fn voxel(app: &mut App, pos: IVec3) -> BlockState {
    app.world_mut()
        .run_system_cached_with(|In(pos): In<IVec3>, voxels: Voxels| voxels.get(pos), pos)
        .unwrap()
        .unwrap()
}

fn stone(app: &App) -> ItemState {
    app.world()
        .resource::<Content>()
        .default_item("defaria:stone")
        .unwrap()
}

/// Iron ore in stone, which the furnace smelts.
fn ore(app: &App) -> ItemState {
    let content = app.world().resource::<Content>();
    let stone = content.item("defaria:stone").unwrap();
    stone.state(&[("ore", "defaria:iron")]).unwrap()
}

fn carried(app: &mut App, item: ItemState) -> u32 {
    let mut players = app
        .world_mut()
        .query_filtered::<&Inventory, With<LocalPlayer>>();
    players.single(app.world()).unwrap().count(item)
}

/// The furnace's `input` slot.
fn input(app: &App) -> Option<ItemStack> {
    let entity = crate::headless::server_world(app)
        .resource::<BlockEntities>()
        .get(FURNACE)
        .unwrap();
    let slots = crate::headless::server_world(app)
        .get::<Slots>(entity)
        .unwrap();
    slots.group("input").unwrap().slots[0].clone()
}

/// Where the player's inventory entry for `item` is on screen.
fn entry_at(app: &mut App, item: ItemState) -> Vec2 {
    let mut entries = app.world_mut().query::<(&ListEntry, &UiGlobalTransform)>();
    entries
        .iter(app.world())
        .find(|(entry, _)| entry.stack.item == item)
        .expect("no entry for the item")
        .1
        .translation
}

/// Where slot `index` of the machine's slot group `group` is on screen.
fn slot_at(app: &mut App, group: usize, index: usize) -> Vec2 {
    let mut slots = app
        .world_mut()
        .query::<(&MachineSlot, &UiGlobalTransform)>();
    slots
        .iter(app.world())
        .find(|(slot, _)| **slot == MachineSlot { group, index })
        .unwrap()
        .1
        .translation
}

fn click(button: &str, at: Vec2) -> String {
    format!("cursor {} {}\nwait 1\nhold {button} 1\nwait 2", at.x, at.y)
}

fn shift_click(at: Vec2) -> String {
    format!(
        "press ShiftLeft\nwait 1\n{}\nrelease ShiftLeft\nwait 1",
        click("MouseLeft", at)
    )
}

/// Right click on a machine opens its screen and frees the cursor rather
/// than placing the held block against it; E closes it (and doesn't open
/// the inventory), and Shift with right click places as usual.
#[test]
fn using_a_machine_opens_its_screen() {
    let mut app = game();
    let front = FURNACE + IVec3::Z;
    open(&mut app);
    assert_eq!(cursor(&mut app), CursorGrabMode::None);
    assert!(voxel(&mut app, front).is_air());
    let mut screens = app.world_mut().query::<&MachineScreen>();
    let screen = screens.single(app.world()).unwrap().clone();
    assert_eq!(screen.pos, FURNACE);
    // The layout follows the definition: a slot each for input and
    // output, a progress bar and an energy bar, and no tanks.
    let world = app.world_mut();
    assert_eq!(world.query::<&MachineSlot>().iter(world).count(), 2);
    assert_eq!(world.query::<&ProgressBar>().iter(world).count(), 1);
    assert_eq!(world.query::<&EnergyBar>().iter(world).count(), 1);
    assert_eq!(world.query::<&TankBar>().iter(world).count(), 0);
    let mut lines = world.query_filtered::<&Text, With<StatusText>>();
    assert_eq!(lines.single(world).unwrap().0, "Idle");
    let mut tips = world.query_filtered::<&Tooltip, With<EnergyBar>>();
    assert_eq!(tips.single(world).unwrap().0, "Energy\n0 J of 10 kJ");
    let mut numbers = world.query_filtered::<&Text, With<EnergyText>>();
    assert_eq!(numbers.single(world).unwrap().0, "0 J\nof 10 kJ");

    run_more(&mut app, "hold KeyE 1\nwait 3");
    assert!(!is_open(&mut app));
    let mut inventories = app.world_mut().query::<&InventoryScreen>();
    assert_eq!(inventories.iter(app.world()).count(), 0);
    assert_eq!(cursor(&mut app), CursorGrabMode::Locked);

    // Escape closes it too.
    open(&mut app);
    run_more(&mut app, "hold Escape 1\nwait 3");
    assert!(!is_open(&mut app));
    // Escape closing a screen leaves the cursor locked (it frees it only
    // with nothing open to close).
    assert_eq!(cursor(&mut app), CursorGrabMode::Locked);

    run_more(
        &mut app,
        "press ShiftLeft\nhold MouseRight 1\nwait 1\nrelease ShiftLeft\nwait 2",
    );
    assert!(!is_open(&mut app));
    assert!(!voxel(&mut app, front).is_air(), "nothing was placed");
}

/// Clicks move items between the player and the machine as they do in the
/// inventory: one, all with Shift, half with the right button; never
/// more than a slot holds, and nothing is made or lost.
#[test]
fn clicks_move_items_between_the_player_and_the_machine() {
    let mut app = game();
    open(&mut app);
    // Plain stone, which no smelting recipe takes, stays with the player.
    let plain = stone(&app);
    let before = carried(&mut app, plain);
    let at = entry_at(&mut app, plain);
    run_more(&mut app, &shift_click(at));
    assert_eq!(input(&app), None);
    assert_eq!(carried(&mut app, plain), before);

    let stone = ore(&app);
    let start = carried(&mut app, stone);
    assert_eq!(start, 64);

    let mine = entry_at(&mut app, stone);
    run_more(&mut app, &click("MouseLeft", mine));
    assert_eq!(input(&app), Some(ItemStack::new(stone, 1)));
    assert_eq!(carried(&mut app, stone), start - 1);
    // Every frame, the screen shows what the slot holds.
    // `input` comes before `output`.
    let input_group = 0;
    let mut slots = app.world_mut().query::<(&MachineSlot, &ItemSlot)>();
    let shown = slots
        .iter(app.world())
        .find(|(slot, _)| slot.group == input_group)
        .unwrap()
        .1;
    assert_eq!((shown.item, shown.count), (Some(stone), 1));

    run_more(&mut app, &click("MouseRight", mine));
    let half = (start - 1).div_ceil(2);
    assert_eq!(input(&app).unwrap().count, 1 + half);
    run_more(&mut app, &shift_click(mine));
    assert_eq!(input(&app).unwrap().count, start);
    assert_eq!(carried(&mut app, stone), 0);

    // Back out: half, then the rest.
    let slot = slot_at(&mut app, input_group, 0);
    run_more(&mut app, &click("MouseRight", slot));
    assert_eq!(carried(&mut app, stone), start / 2);
    run_more(&mut app, &click("MouseLeft", slot));
    assert_eq!(carried(&mut app, stone), start / 2 + 1);
    run_more(&mut app, &shift_click(slot));
    assert_eq!(carried(&mut app, stone), start);
    assert_eq!(input(&app), None);
}

/// Dragging an entry of the player's inventory onto the machine moves all
/// of it into the input.
#[test]
fn dragging_onto_the_machine_moves_it_all() {
    let mut app = game();
    open(&mut app);
    let stone = ore(&app);
    let (from, to) = (entry_at(&mut app, stone), slot_at(&mut app, 0, 0));
    run_more(
        &mut app,
        &format!(
            "cursor {} {}\nwait 1\npress MouseLeft\nwait 1\ncursor {} {}\nwait 1\ncursor {} {}\nwait 1\nrelease MouseLeft\nwait 2",
            from.x,
            from.y,
            (from.x + to.x) / 2.0,
            (from.y + to.y) / 2.0,
            to.x,
            to.y
        ),
    );
    assert_eq!(input(&app), Some(ItemStack::new(stone, 64)));
    assert_eq!(carried(&mut app, stone), 0);
}

/// Dropped on a slot of a group no recipe names (one a script reads),
/// the player's items go into that group (provisional, for the user:
/// docs/decisions.md); dropped elsewhere on the machine, into its inputs,
/// which don't take them.
#[test]
fn dropping_on_a_group_puts_items_there() {
    let content = test_content(&[
        (
            "blocks/bench.json5",
            r#"{ model: "defaria:cube", textures: { all: "defaria:machine_side" }, machine: { slots: { work: 1, catalyst: 1, done: 1 }, recipes: { type: "polish", inputs: ["work"], outputs: ["done"] } } }"#,
        ),
        (
            "recipes/polish.json5",
            r#"{ type: "polish", inputs: [{ item: "defaria:planks" }], outputs: [{ item: "defaria:glass" }], time: 1 }"#,
        ),
    ]);
    let catalyst = content
        .machine(content.default_state("test:bench").unwrap())
        .unwrap()
        .slot_group("catalyst")
        .unwrap();
    let mut app = test_game(Script::parse("camera 64.5 70.5 92.5 0 0\nwait 2").unwrap());
    app.insert_resource(content);
    settle(&mut app);
    run_more(
        &mut app,
        "fill 60 66 84 68 74 92 air\n\
         fill 64 70 89 64 70 89 test:bench\n\
         wait 2\n\
         hold MouseLeft 1\n\
         wait 1",
    );
    open(&mut app);
    let stone = stone(&app);
    let before = carried(&mut app, stone);
    assert!(before > 0);
    let drag = |from: Vec2, to: Vec2| {
        format!(
            "cursor {} {}\nwait 1\npress MouseLeft\nwait 1\ncursor {} {}\nwait 1\ncursor {} {}\nwait 1\nrelease MouseLeft\nwait 2",
            from.x,
            from.y,
            (from.x + to.x) / 2.0,
            (from.y + to.y) / 2.0,
            to.x,
            to.y
        )
    };
    let held = |app: &App| {
        let entity = crate::headless::server_world(app)
            .resource::<BlockEntities>()
            .get(FURNACE)
            .unwrap();
        let slots = crate::headless::server_world(app)
            .get::<Slots>(entity)
            .unwrap();
        slots.0[catalyst].slots[0].clone()
    };
    // On the machine's name, above its slots: nothing takes stone.
    let (from, name) = (entry_at(&mut app, stone), slot_at(&mut app, catalyst, 0));
    run_more(&mut app, &drag(from, name - Vec2::new(0.0, 40.0)));
    assert_eq!(held(&app), None);
    assert_eq!(carried(&mut app, stone), before);
    run_more(&mut app, &drag(from, name));
    let moved = before.min(defaria_core::machine::SLOT_SIZE);
    assert_eq!(held(&app), Some(ItemStack::new(stone, moved)));
    assert_eq!(carried(&mut app, stone), before - moved);
}

/// Breaking the machine closes its screen, and so does its chunk
/// unloading.
#[test]
fn the_screen_closes_when_the_machine_goes() {
    let mut app = game();
    open(&mut app);
    run_more(&mut app, "fill 64 70 89 64 70 89 air\nwait 2");
    assert!(!is_open(&mut app));
    assert_eq!(cursor(&mut app), CursorGrabMode::Locked);

    run_more(
        &mut app,
        "fill 64 70 89 64 70 89 electric_furnace[facing=south]\nwait 2",
    );
    open(&mut app);
    // Somewhere far away: the furnace's chunk unloads.
    run_more(
        &mut app,
        "camera 64.5 70.5 2092.5 0 0\nwait 2\nsettle\nwait 2",
    );
    assert!(
        crate::headless::server_world(&app)
            .resource::<BlockEntities>()
            .get(FURNACE)
            .is_none()
    );
    assert!(!is_open(&mut app));
}

/// Mid-recipe, the screen shows the progress, and the status says when
/// it's waiting.
#[test]
fn the_screen_shows_progress_and_status() {
    let mut app = game();
    run_more(
        &mut app,
        "fill 64 70 88 64 70 88 wire\n\
         fill 64 70 87 64 70 87 creative_generator\n\
         put 64 70 89 input 2 stone[ore=defaria:iron]\n\
         wait 30",
    );
    open(&mut app);
    let world = app.world_mut();
    let mut bars = world.query_filtered::<&Bar, With<ProgressBar>>();
    let fraction = bars.single(world).unwrap().fraction;
    assert!(fraction > 0.0 && fraction < 1.0, "{fraction}");
    let mut lines = world.query_filtered::<&Text, With<StatusText>>();
    assert!(lines.single(world).unwrap().0.ends_with('%'));
}

#[test]
fn numbers_for_people() {
    assert_eq!(
        [0, 999, 1000, 4300, 10_000, 2_500_000].map(format_energy),
        ["0 J", "999 J", "1 kJ", "4.3 kJ", "10 kJ", "2.5 MJ"]
    );
    assert_eq!(title_case("input"), "Input");
    let progress = |status, ticks| RecipeProgress {
        status,
        ticks,
        total: 80,
        ..default()
    };
    assert_eq!(status_text(&progress(RecipeStatus::Running, 20)), "25%");
    assert_eq!(status_text(&progress(RecipeStatus::Idle, 0)), "Idle");
    assert_eq!(
        status_text(&progress(RecipeStatus::NoEnergy, 3)),
        "Needs energy"
    );
    assert_eq!(
        status_text(&progress(RecipeStatus::OutputFull, 80)),
        "Output full"
    );
}

/// Machines with slots, tanks or recipes have a screen; a lamp or battery,
/// with only energy, has none, so right click places against it.
#[test]
fn only_machines_with_something_to_handle_have_screens() {
    let content = defaria_core::content::base();
    let screen =
        |name: &str| has_screen(content.machine(defaria_core::content::block(name)).unwrap());
    for name in ["electric_furnace", "combustion_generator", "crate", "tank"] {
        assert!(screen(name), "{name}");
    }
    for name in ["powered_lamp", "battery", "creative_generator"] {
        assert!(!screen(name), "{name}");
    }
}

/// Slot groups up to four are a row; bigger ones are square-ish, so a
/// crate's nine are three by three.
#[test]
fn slot_groups_are_square_ish() {
    assert_eq!(
        [0, 1, 2, 4, 5, 6, 9, 10, 16, 17, 200].map(group_columns),
        [1, 1, 2, 4, 3, 3, 3, 4, 4, 5, 9]
    );
}

/// A slot group that's both a recipe's input and its output is drawn
/// once, with the inputs.
#[test]
fn a_group_both_in_and_out_is_drawn_once() {
    let content = test_content(&[
        (
            "blocks/bench.json5",
            r#"{ model: "defaria:cube", textures: { all: "defaria:machine_side" }, machine: { slots: { work: 2, spare: 1 }, recipes: { type: "polish", inputs: ["work"], outputs: ["work"] } } }"#,
        ),
        (
            "recipes/polish.json5",
            r#"{ type: "polish", inputs: [{ item: "defaria:planks" }], outputs: [{ item: "defaria:glass" }], time: 1 }"#,
        ),
    ]);
    let state = content.default_state("test:bench").unwrap();
    let mut world = World::new();
    let screen = MachineScreen {
        machine: Entity::PLACEHOLDER,
        pos: IVec3::ZERO,
        state,
    };
    spawn_machine_screen(&mut world.commands(), &UiTheme::default(), &content, screen);
    world.flush();
    let mut slots: Vec<(usize, usize)> = world
        .query::<&MachineSlot>()
        .iter(&world)
        .map(|slot| (slot.group, slot.index))
        .collect();
    slots.sort();
    let work = content
        .machine(state)
        .unwrap()
        .recipes
        .as_ref()
        .unwrap()
        .input_slots[0];
    let spare = 1 - work;
    let mut expected = vec![(work, 0), (work, 1), (spare, 0)];
    expected.sort();
    assert_eq!(slots, expected);
}

/// The panel's machine part starts at the same height for a furnace, a
/// crate (nine slots, three by three) and a generator, so the screen
/// doesn't jump between machines.
#[test]
fn the_panel_stays_put_between_machines() {
    let mut app = game();
    let mut tops = Vec::new();
    for block in [
        "electric_furnace[facing=south]",
        "crate",
        "combustion_generator[facing=south]",
    ] {
        run_more(&mut app, &format!("fill 64 70 89 64 70 89 {block}\nwait 2"));
        open(&mut app);
        let world = app.world_mut();
        let mut panels =
            world.query_filtered::<(&ComputedNode, &UiGlobalTransform), With<MachinePanel>>();
        let (node, at) = panels.single(world).unwrap();
        tops.push(at.translation.y - node.size.y / 2.0);
        run_more(
            &mut app,
            "hold KeyE 1\nwait 3\nfill 64 70 89 64 70 89 air\nwait 2",
        );
    }
    assert_eq!(tops[0], tops[1], "{tops:?}");
    assert_eq!(tops[0], tops[2], "{tops:?}");
}

/// A machine with three big slot groups, wider together than the window,
/// wraps them onto more lines: the whole screen stays in the window.
#[test]
fn wide_machines_wrap_to_fit_the_window() {
    let content = test_content(&[(
        "blocks/wide.json5",
        r#"{ model: "defaria:cube", textures: { all: "defaria:machine_side" }, machine: { slots: { a: 40, b: 40, c: 40 } } }"#,
    )]);
    let mut app = test_game(Script::parse("camera 64.5 70.5 92.5 0 0\nwait 2").unwrap());
    app.insert_resource(content);
    settle(&mut app);
    run_more(
        &mut app,
        "fill 60 66 84 68 74 92 air\n\
         fill 64 70 89 64 70 89 test:wide\n\
         wait 2\n\
         hold MouseLeft 1\n\
         wait 1",
    );
    open(&mut app);
    let world = app.world_mut();
    let width = world
        .query::<&Window>()
        .single(world)
        .unwrap()
        .physical_width() as f32;
    let mut panels = world.query_filtered::<&ChildOf, With<MachinePanel>>();
    let outer = panels.single(world).unwrap().parent();
    let (node, at) = world
        .query::<(&ComputedNode, &UiGlobalTransform)>()
        .get(world, outer)
        .unwrap();
    let (min, max) = (
        at.translation - node.size / 2.0,
        at.translation + node.size / 2.0,
    );
    assert!(min.x >= 0.0 && max.x <= width, "{min} {max} in {width}");
    assert!(min.y >= 0.0, "the screen's top is above the window: {min}");
    // The player's inventory keeps its size.
    let mut lists = world.query::<(&Name, &ComputedNode)>();
    let list = lists
        .iter(world)
        .find(|(name, _)| name.as_str() == "Inventory list")
        .unwrap()
        .1
        .size
        .y;
    let (theme, rows) = (UiTheme::default(), INVENTORY_ROWS as f32);
    assert_eq!(list, rows * theme.slot_size + (rows - 1.0) * theme.slot_gap);
    // The machine's part scrolls, since its parts don't fit even wrapped.
    let mut machine = world.query_filtered::<&ComputedNode, With<MachinePanel>>();
    let node = machine.single(world).unwrap();
    assert!(crate::ui::scroll::max_scroll(node) > 0.0);
}

/// The base game and a test mod with `big_crate`: 54 slots, six rows.
fn big_crate_content() -> Content {
    test_content(&[(
        "blocks/big_crate.json5",
        r#"{ model: "defaria:cube", textures: { all: "defaria:machine_side" }, machine: { slots: { items: 54 } } }"#,
    )])
}

/// A slot group too tall for the screen scrolls, so the screen fits in
/// the window, at the same height as a furnace's.
#[test]
fn a_big_slot_group_scrolls() {
    let mut app = test_game(Script::parse("camera 64.5 70.5 92.5 0 0\nwait 2").unwrap());
    app.insert_resource(big_crate_content());
    settle(&mut app);
    run_more(
        &mut app,
        "fill 60 66 84 68 74 92 air\n\
         fill 64 70 89 64 70 89 test:big_crate\n\
         wait 2\n\
         hold MouseLeft 1\n\
         wait 1",
    );
    open(&mut app);
    let world = app.world_mut();
    let mut panels =
        world.query_filtered::<(&ComputedNode, &UiGlobalTransform, &ChildOf), With<MachinePanel>>();
    let (node, at, parent) = panels.single(world).unwrap();
    let (machine_top, parent) = (at.translation.y - node.size.y / 2.0, parent.parent());
    let (outer, outer_at) = world
        .query::<(&ComputedNode, &UiGlobalTransform)>()
        .get(world, parent)
        .unwrap();
    assert!(
        outer_at.translation.y - outer.size.y / 2.0 >= 0.0,
        "the screen's top is above the window"
    );
    let mut scrolled =
        world.query_filtered::<(&Name, &ComputedNode), With<crate::ui::scroll::Scrollable>>();
    let grids: Vec<f32> = scrolled
        .iter(world)
        .filter(|(name, _)| name.as_str() == "Slot group")
        .map(|(_, node)| crate::ui::scroll::max_scroll(node))
        .collect();
    assert_eq!(grids.len(), 1);
    assert!(grids[0] > 0.0, "{grids:?}");

    // The wheel over it scrolls it; the last row comes into view.
    let first = slot_at(&mut app, 0, 0);
    run_more(
        &mut app,
        &format!("cursor {} {}\nwait 1\nscroll -10\nwait 3", first.x, first.y),
    );
    let last = slot_at(&mut app, 0, 53);
    assert!(last.y < first.y + 3.0 * 60.0, "{last} {first}");
    assert!(slot_at(&mut app, 0, 0).y < first.y);

    // The same height as a furnace's.
    run_more(
        &mut app,
        "hold KeyE 1\nwait 3\nfill 64 70 89 64 70 89 electric_furnace[facing=south]\nwait 2",
    );
    open(&mut app);
    let world = app.world_mut();
    let mut panels =
        world.query_filtered::<(&ComputedNode, &UiGlobalTransform), With<MachinePanel>>();
    let (node, at) = panels.single(world).unwrap();
    assert_eq!(at.translation.y - node.size.y / 2.0, machine_top);
}

/// The base game and a test mod with a machine that has slots (so a
/// screen) and a script counting its uses.
fn scripted_machine_mods() -> Vec<defaria_core::content::load::ModFiles> {
    test_mods(&[
        (
            "blocks/box.json5",
            r#"{ model: "defaria:cube", textures: { all: "defaria:machine_side" }, script: "box", machine: { slots: { items: 1 } } }"#,
        ),
        (
            "scripts/box.luau",
            "function on_used(block, player)\n\tblock.data.used = (block.data.used or 0) + 1\nend\n",
        ),
    ])
}

/// Right click on a machine whose script has `on_used` runs the script
/// instead of opening its screen; with a wrench in hand, the wrench takes
/// the click and neither happens.
#[test]
fn a_scripted_machine_is_used_rather_than_opened() {
    use defaria_core::{
        block_entity::{BlockData, Value},
        content::load::order,
        script::{ScriptIndex, ScriptRuntime, ScriptSettings, ScriptSources},
    };
    let content = Content::from_mods(scripted_machine_mods());
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    let runtime = || {
        let sources = ScriptSources::from_mods(&order(scripted_machine_mods(), &mut Vec::new()));
        ScriptRuntime::new(sources, ScriptSettings::untimed())
    };
    let mut app = test_game(Script::parse("camera 64.5 70.5 92.5 0 0\nwait 2").unwrap());
    // The server runs the scripts (it starts on the first frame); both
    // sides have the content.
    app.update();
    let server = crate::headless::server(&mut app);
    let runtime = runtime();
    server.insert_resource(ScriptIndex::new(&content, &runtime));
    server.insert_resource(runtime);
    server.insert_resource(content.clone());
    app.insert_resource(content);
    settle(&mut app);
    let used = |app: &mut App| {
        let entity = crate::headless::server_world(app)
            .resource::<BlockEntities>()
            .get(FURNACE)
            .unwrap();
        crate::headless::server_world(app)
            .get::<BlockData>(entity)
            .and_then(|data| data.0.get("used").cloned())
    };
    run_more(
        &mut app,
        "fill 60 66 84 68 74 92 air\n\
         fill 64 70 89 64 70 89 test:box\n\
         wait 2\n\
         hold MouseLeft 1\n\
         wait 1\n\
         hold MouseRight 1\n\
         wait 5",
    );
    assert!(!is_open(&mut app));
    assert_eq!(used(&mut app), Some(Value::Int(1)));

    run_more(
        &mut app,
        "hotbar 1 wrench\nwait 1\nhold MouseRight 1\nwait 5",
    );
    assert!(!is_open(&mut app));
    assert_eq!(used(&mut app), Some(Value::Int(1)));
}

/// The base game and a test mod with a block whose item's script has
/// `on_use` (turning the block aimed at into a marker), a machine with a
/// screen and no script, and the scripted machine of
/// [`scripted_machine_mods`].
fn scripted_item_mods() -> Vec<defaria_core::content::load::ModFiles> {
    test_mods(&[
        (
            "blocks/box.json5",
            r#"{ model: "defaria:cube", textures: { all: "defaria:machine_side" }, script: "box", machine: { slots: { items: 1 } } }"#,
        ),
        (
            "scripts/box.luau",
            "function on_used(block, player)\n\tblock.data.used = (block.data.used or 0) + 1\nend\n",
        ),
        (
            "blocks/crate.json5",
            r#"{ model: "defaria:cube", textures: { all: "defaria:machine_side" }, machine: { slots: { items: 1 } } }"#,
        ),
        (
            "blocks/marker.json5",
            r#"{ model: "defaria:cube", textures: { all: "defaria:machine_side" } }"#,
        ),
        (
            "blocks/wand.json5",
            r#"{ model: "defaria:cube", textures: { all: "defaria:machine_side" } }"#,
        ),
        ("items/wand.json5", r#"{ script: "wand" }"#),
        (
            "scripts/wand.luau",
            "function on_use(item, player)\n\treturn player.target:set(\"test:marker\")\nend\n",
        ),
    ])
}

/// An item's `on_use` comes after what the block it's used on does: with
/// a scripted item that would also place a block, right click on stone
/// uses the item (and places nothing), on a machine opens its screen, and
/// on a scripted machine runs the machine's `on_used`.
#[test]
fn a_scripted_item_comes_after_the_block_it_is_used_on() {
    use defaria_core::{
        block_entity::{BlockData, Value},
        content::load::order,
        script::{ScriptIndex, ScriptRuntime, ScriptSettings, ScriptSources},
    };
    let content = Content::from_mods(scripted_item_mods());
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    let state = |id: &str| content.default_state(id).unwrap();
    let [stone, crate_, scripted, marker] =
        ["defaria:stone", "test:crate", "test:box", "test:marker"].map(state);
    let runtime = || {
        let sources = ScriptSources::from_mods(&order(scripted_item_mods(), &mut Vec::new()));
        ScriptRuntime::new(sources, ScriptSettings::untimed())
    };
    let mut app = test_game(Script::parse("camera 64.5 70.5 92.5 0 0\nwait 2").unwrap());
    // The server runs the scripts (it starts on the first frame); both
    // sides have the content.
    app.update();
    let server = crate::headless::server(&mut app);
    let runtime = runtime();
    server.insert_resource(ScriptIndex::new(&content, &runtime));
    server.insert_resource(runtime);
    server.insert_resource(content.clone());
    app.insert_resource(content);
    settle(&mut app);
    let front = FURNACE + IVec3::Z;
    run_more(
        &mut app,
        "fill 60 66 84 68 74 92 air\n\
         fill 64 70 89 64 70 89 stone\n\
         wait 2\n\
         hold MouseLeft 1\n\
         wait 1\n\
         hotbar 1 test:wand\n\
         wait 1",
    );
    assert_eq!(voxel(&mut app, FURNACE), stone);

    run_more(&mut app, "hold MouseRight 1\nwait 5");
    assert_eq!(voxel(&mut app, FURNACE), marker, "the item was used");
    assert!(voxel(&mut app, front).is_air(), "and nothing was placed");

    run_more(
        &mut app,
        "fill 64 70 89 64 70 89 test:crate\nwait 2\nhold MouseRight 1\nwait 5",
    );
    assert!(is_open(&mut app), "the machine's screen opened");
    assert_eq!(voxel(&mut app, FURNACE), crate_, "and the item wasn't used");

    run_more(
        &mut app,
        "hold KeyE 1\nwait 3\n\
         fill 64 70 89 64 70 89 test:box\n\
         wait 2\n\
         hold MouseRight 1\n\
         wait 5",
    );
    assert!(!is_open(&mut app));
    assert_eq!(voxel(&mut app, FURNACE), scripted, "the item wasn't used");
    let entity = crate::headless::server_world(&app)
        .resource::<BlockEntities>()
        .get(FURNACE)
        .unwrap();
    let used = crate::headless::server_world(&app)
        .get::<BlockData>(entity)
        .and_then(|data| data.0.get("used").cloned());
    assert_eq!(used, Some(Value::Int(1)), "the machine was used");
    assert!(voxel(&mut app, front).is_air(), "nothing was placed");
}

/// Once a use has opened a machine's screen the mouse doesn't turn the
/// view, and the use is gone by the end of the frame it was made; a right
/// click on air leaves no use behind either. (The screen opens when the
/// server says so, a frame after the click, so the mouse can still turn
/// the view on the click's own frame.)
#[test]
fn opening_a_screen_stops_the_view_turning() {
    use defaria_core::{player::Look, use_action::UseAction};
    let mut app = game();
    let look = |app: &mut App| {
        let mut looks = app
            .world_mut()
            .query_filtered::<(&Look, &UseAction), With<LocalPlayer>>();
        let (look, action) = looks.single(app.world()).unwrap();
        (look.yaw, action.pending())
    };
    run_more(&mut app, "hold MouseRight 1\nwait 3");
    assert!(is_open(&mut app));
    let (before, _) = look(&mut app);
    run_more(&mut app, "mouse 200 0\nwait 2");
    assert_eq!(look(&mut app), (before, None));

    run_more(
        &mut app,
        "hold KeyE 1\nwait 3\nfill 64 70 89 64 70 89 air\nwait 2\nhold MouseRight 1\nwait 1",
    );
    assert_eq!(look(&mut app).1, None, "no use outlasts its frame");
}
