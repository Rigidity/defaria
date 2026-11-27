//! Recipe machines: [`step`] on the base game's furnace and generator
//! held by hand, and a furnace smelting on an energy network.

use std::time::Duration;

use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::{
    block_entity::{BlockEntities, BlockEntityPlugin},
    connections::ConnectionsPlugin,
    machine::MachinePlugin,
    network::{NetworkClock, NetworkCorePlugin, energy::EnergyPlugin},
};
use defaria_core::{
    block_entity::BlockEntityPart,
    content::{BlockState, ItemState, base, block, block_with},
    testing::{chunk_app, insert_chunk, test_content},
    world::{Chunk, Voxels},
};

/// A machine as its block entity starts, held by hand.
struct Held {
    progress: RecipeProgress,
    holdings: Holdings,
}

impl Held {
    fn new(name: &str) -> Self {
        let content = base();
        let state = block(name);
        Self {
            progress: RecipeProgress::new(state, content).unwrap(),
            holdings: Holdings {
                slots: Slots::new(state, content),
                tanks: Tanks::new(state, content),
                energy: EnergyBuffer::new(state, content),
                ..default()
            },
        }
    }

    /// Runs `n` ticks, returning how many it was running for.
    fn run(&mut self, n: u32) -> u32 {
        (0..n)
            .filter(|_| step(base(), &mut self.progress, &mut self.holdings))
            .count() as u32
    }

    fn group(&mut self, name: &str) -> &mut defaria_core::machine::SlotGroup {
        self.holdings
            .slots
            .as_mut()
            .unwrap()
            .group_mut(name)
            .unwrap()
    }

    fn put(&mut self, group: &str, item: ItemState, count: u32) {
        assert!(
            self.group(group)
                .insert(base(), ItemStack::new(item, count))
                .is_none()
        );
    }

    fn slot(&mut self, group: &str) -> Option<ItemStack> {
        self.group(group).slots[0].clone()
    }

    fn energy(&mut self) -> &mut EnergyBuffer {
        self.holdings.energy.as_mut().unwrap()
    }
}

fn ore(rock: &str, ore: &str) -> ItemState {
    base()
        .item("defaria:stone")
        .unwrap()
        .state(&[("rock", rock), ("ore", ore)])
        .unwrap()
}

fn ingot(metal: &str) -> ItemState {
    base()
        .item("defaria:ingot")
        .unwrap()
        .state(&[("metal", metal)])
        .unwrap()
}

fn log() -> ItemState {
    base().default_item("defaria:log").unwrap()
}

const IRON: &str = "defaria:iron";
const STONE: &str = "defaria:stone";

/// A powered furnace takes an ore, smelts it over the recipe's time using
/// exactly its energy, and gives its ingot.
#[test]
fn a_furnace_smelts_an_ore_to_completion() {
    let mut furnace = Held::new("electric_furnace");
    furnace.put("input", ore(STONE, IRON), 1);
    furnace.energy().stored = 10_000;
    let ran = furnace.run(79);
    assert_eq!(ran, 79);
    assert_eq!(furnace.progress.status, RecipeStatus::Running);
    assert_eq!(
        furnace.progress.recipe.as_deref(),
        Some("defaria:smelt_ore[ore=defaria:iron]")
    );
    assert_eq!((furnace.progress.ticks, furnace.progress.total), (79, 80));
    // The ore went in when it started; nothing's out yet.
    assert_eq!(furnace.slot("input"), None);
    assert_eq!(furnace.slot("output"), None);
    // The last tick finishes it, and with nothing more to smelt it stops.
    assert!(!step(base(), &mut furnace.progress, &mut furnace.holdings));
    assert_eq!(furnace.slot("output"), Some(ItemStack::new(ingot(IRON), 1)));
    assert_eq!(furnace.progress.status, RecipeStatus::Idle);
    assert_eq!(furnace.progress.recipe, None);
    assert_eq!(furnace.energy().stored, 10_000 - 2000);
    assert_eq!(furnace.holdings.used, 2000);
}

/// One recipe after another, with no tick lost between them.
#[test]
fn a_furnace_runs_recipes_back_to_back() {
    let mut furnace = Held::new("electric_furnace");
    furnace.put("input", ore(STONE, IRON), 3);
    furnace.energy().stored = 10_000;
    assert_eq!(furnace.run(240), 239);
    assert_eq!(furnace.slot("output"), Some(ItemStack::new(ingot(IRON), 3)));
    assert_eq!(furnace.energy().stored, 10_000 - 3 * 2000);
}

/// Out of energy part way, it stops where it is and goes on once its
/// buffer has some again, using no more than the recipe's energy in all.
#[test]
fn a_furnace_waits_for_energy_and_resumes() {
    let mut furnace = Held::new("electric_furnace");
    furnace.put("input", ore(STONE, IRON), 1);
    // Enough for 40 of the 80 ticks (25 J each).
    furnace.energy().stored = 1000;
    assert_eq!(furnace.run(60), 40);
    assert_eq!(furnace.progress.status, RecipeStatus::NoEnergy);
    assert_eq!(furnace.progress.ticks, 40);
    assert_eq!(furnace.energy().stored, 0);
    furnace.energy().stored = 5000;
    assert_eq!(furnace.run(60), 39);
    assert_eq!(furnace.slot("output"), Some(ItemStack::new(ingot(IRON), 1)));
    assert_eq!(furnace.energy().stored, 4000);
    assert_eq!(furnace.holdings.used, 2000);
}

/// Without the energy for its first tick it doesn't start, so the ore
/// stays in its slot.
#[test]
fn an_unpowered_furnace_leaves_its_input_alone() {
    let mut furnace = Held::new("electric_furnace");
    furnace.put("input", ore(STONE, IRON), 1);
    assert_eq!(furnace.run(10), 0);
    assert_eq!(furnace.progress.status, RecipeStatus::NoEnergy);
    assert_eq!(furnace.progress.recipe, None);
    assert_eq!(
        furnace.slot("input"),
        Some(ItemStack::new(ore(STONE, IRON), 1))
    );
}

/// A full output slot keeps it from starting, and one that fills while
/// it runs keeps it waiting at the end until there's room.
#[test]
fn a_full_output_stops_the_furnace() {
    let mut furnace = Held::new("electric_furnace");
    furnace.energy().stored = 10_000;
    furnace.put("input", ore(STONE, IRON), 2);
    furnace.put("output", ingot(IRON), 64);
    assert_eq!(furnace.run(10), 0);
    assert_eq!(furnace.progress.status, RecipeStatus::OutputFull);
    assert_eq!(furnace.slot("input").unwrap().count, 2);
    assert_eq!(furnace.energy().stored, 10_000);

    // Room for one: it smelts one, and doesn't start the next.
    furnace.group("output").take(0, 1);
    assert_eq!(furnace.run(200), 79);
    assert_eq!(furnace.progress.status, RecipeStatus::OutputFull);
    assert_eq!(furnace.progress.recipe, None);
    assert_eq!(furnace.slot("output").unwrap().count, 64);
    assert_eq!(furnace.slot("input").unwrap().count, 1);

    // An output that fills while it runs (from a tube, say) keeps it
    // waiting at the end, done, until there's room.
    furnace.group("output").take(0, 1);
    assert_eq!(furnace.run(40), 40);
    furnace.put("output", ingot(IRON), 1);
    assert_eq!(furnace.run(100), 39);
    assert_eq!(furnace.progress.status, RecipeStatus::OutputFull);
    assert_eq!((furnace.progress.ticks, furnace.progress.total), (80, 80));
    furnace.group("output").take(0, 1);
    assert!(!step(base(), &mut furnace.progress, &mut furnace.holdings));
    assert_eq!(furnace.slot("output").unwrap().count, 64);
    assert_eq!(furnace.progress.status, RecipeStatus::Idle);
    assert_eq!(furnace.energy().stored, 10_000 - 2 * 2000);

    // An output slot holding something else is full too.
    let mut furnace = Held::new("electric_furnace");
    furnace.energy().stored = 10_000;
    furnace.put("input", ore(STONE, IRON), 1);
    furnace.put("output", ingot("defaria:gold"), 1);
    assert_eq!(furnace.run(10), 0);
    assert_eq!(furnace.progress.status, RecipeStatus::OutputFull);
}

/// The base game and two more `smelting` recipes, after its own: planks
/// into glass for 5 kJ a tick, and iron ore into nothing but energy.
fn dear_recipes() -> &'static Content {
    static CONTENT: std::sync::OnceLock<Content> = std::sync::OnceLock::new();
    CONTENT.get_or_init(|| {
        test_content(&[
            (
                "recipes/dear.json5",
                r#"{ type: "smelting", inputs: [{ item: "defaria:planks" }], outputs: [{ item: "defaria:glass" }], energy: 100000, time: 1 }"#,
            ),
            (
                "recipes/spark.json5",
                r#"{ type: "smelting", inputs: [{ item: "defaria:stone", when: { ore: "defaria:iron" } }], outputs: [{ energy: 10 }], energy: 1000, time: 1 }"#,
            ),
        ])
    })
}

/// Waiting for energy to start, a furnace asks for the first tick of the
/// recipe it would start, not of the dearest recipe of its type; and it
/// waits for energy, asking for it, even when an earlier recipe is
/// waiting for room.
#[test]
fn a_waiting_furnace_asks_for_the_recipe_it_would_start() {
    let content = dear_recipes();
    let state = block("electric_furnace");
    let mut progress = RecipeProgress::new(state, content).unwrap();
    let mut holdings = Holdings {
        slots: Slots::new(state, content),
        energy: EnergyBuffer::new(state, content),
        ..default()
    };
    let slots = holdings.slots.as_mut().unwrap();
    let ore = ore(STONE, IRON);
    slots
        .group_mut("input")
        .unwrap()
        .insert(content, ItemStack::new(ore, 1));
    assert!(!step(content, &mut progress, &mut holdings));
    assert_eq!(progress.status, RecipeStatus::NoEnergy);
    // The iron recipe's first tick, 2 kJ over 80 ticks; not the planks'
    // 5 kJ.
    assert_eq!(progress.energy_need(content), 25);

    // An output slot full of glass: the ingot doesn't fit, but the spark
    // recipe's energy would, once it can pay 50 J.
    let glass = content.default_item("defaria:glass").unwrap();
    let slots = holdings.slots.as_mut().unwrap();
    slots.group_mut("output").unwrap().insert(
        content,
        ItemStack::new(glass, defaria_core::machine::SLOT_SIZE),
    );
    assert!(!step(content, &mut progress, &mut holdings));
    assert_eq!(progress.status, RecipeStatus::NoEnergy);
    assert_eq!(progress.energy_need(content), 50);
    holdings.energy.as_mut().unwrap().stored = 50;
    assert!(step(content, &mut progress, &mut holdings));
    assert_eq!(progress.recipe.as_deref(), Some("test:spark"));
    assert_eq!(progress.energy_need(content), 50);
}

/// The templated recipe gives each ore its own metal's ingot, whatever
/// rock it's in, and leaves what has no recipe (coal ore) alone.
#[test]
fn each_ore_smelts_into_its_own_ingot() {
    for metal in ["defaria:copper", "defaria:gold", IRON] {
        let mut furnace = Held::new("electric_furnace");
        furnace.energy().stored = 10_000;
        furnace.put("input", ore("defaria:granite", metal), 1);
        furnace.run(80);
        assert_eq!(
            furnace.slot("output"),
            Some(ItemStack::new(ingot(metal), 1)),
            "{metal}"
        );
    }
    let mut furnace = Held::new("electric_furnace");
    furnace.energy().stored = 10_000;
    furnace.put("input", ore(STONE, "defaria:coal"), 1);
    assert_eq!(furnace.run(10), 0);
    assert_eq!(furnace.progress.status, RecipeStatus::Idle);
}

/// A generator burns a log into exactly the recipe's energy, stopping
/// while its buffer is full and going on once it's drawn from.
#[test]
fn a_generator_burns_fuel_into_energy() {
    let mut generator = Held::new("combustion_generator");
    generator.put("fuel", log(), 1);
    // 15 kJ over 15 s into a 10 kJ buffer: it fills two thirds of the way.
    assert_eq!(generator.run(300), 200);
    assert_eq!(generator.progress.status, RecipeStatus::OutputFull);
    assert_eq!(generator.energy().stored, 10_000);
    generator.energy().stored = 0;
    assert_eq!(generator.run(300), 99);
    assert_eq!(generator.progress.status, RecipeStatus::Idle);
    assert_eq!(generator.energy().stored, 5000);
    assert_eq!(generator.holdings.generated, 15_000);
    assert_eq!(generator.slot("fuel"), None);
}

/// Lava in its tank burns too, a tenth of a block a time.
#[test]
fn a_generator_burns_lava_from_its_tank() {
    let mut generator = Held::new("combustion_generator");
    let tank = &mut generator.holdings.tanks.as_mut().unwrap().0[0];
    assert_eq!(tank.fill("defaria:lava", 150), 150);
    generator.run(10);
    assert_eq!(
        generator.progress.recipe.as_deref(),
        Some("defaria:burn_lava")
    );
    assert_eq!(generator.holdings.tanks.as_ref().unwrap().0[0].amount, 50);
    // Its buffer drawn from as fast as it fills; not enough lava is left
    // for another.
    for _ in 0..400 {
        generator.energy().stored = 0;
        generator.run(1);
    }
    assert_eq!(generator.progress.status, RecipeStatus::Idle);
    assert_eq!(generator.holdings.generated, 20_000);
    assert_eq!(generator.holdings.tanks.as_ref().unwrap().0[0].amount, 50);
}

/// What's running and how far it's got is saved; the status isn't.
#[test]
fn progress_round_trips_through_its_saved_form() {
    let content = base();
    let state = block("electric_furnace");
    let mut furnace = Held::new("electric_furnace");
    furnace.put("input", ore(STONE, IRON), 1);
    furnace.energy().stored = 1000;
    furnace.run(50);
    let saved = furnace.progress.save(content);
    let loaded = RecipeProgress::load(&saved, 1, state, content).unwrap();
    assert_eq!(loaded.recipe, furnace.progress.recipe);
    assert_eq!((loaded.ticks, loaded.total), (40, 80));
    let idle = RecipeProgress::new(state, content).unwrap();
    assert_eq!(
        RecipeProgress::load(&idle.save(content), 1, state, content),
        Some(idle.clone())
    );
    // A recipe that's gone is dropped, inputs and all.
    let mut gone = furnace.progress.clone();
    gone.recipe = Some("defaria:nothing".into());
    let loaded = RecipeProgress::load(&gone.save(content), 1, state, content).unwrap();
    assert_eq!(loaded, idle);
    assert_eq!(RecipeProgress::load(&saved, 2, state, content), None);
}

/// The registry knows the part, so `inspect RecipeProgress` shows it.
#[test]
fn progress_is_reflected() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(base().clone())
        .add_plugins((BlockEntityPlugin, RecipePlugin));
    let registry = app.world().resource::<AppTypeRegistry>().read();
    assert!(
        registry
            .get_with_short_type_path("RecipeProgress")
            .is_some()
    );
}

// On a network.

fn app() -> App {
    let mut app = chunk_app(base().clone(), Duration::from_millis(10));
    app.add_plugins((
        ConnectionsPlugin,
        NetworkCorePlugin,
        BlockEntityPlugin,
        MachinePlugin,
        EnergyPlugin,
        RecipePlugin,
    ));
    insert_chunk(&mut app, IVec3::ZERO, Chunk::default());
    app.update();
    app
}

fn set(app: &mut App, voxels: Vec<(IVec3, BlockState)>) {
    app.world_mut()
        .run_system_once(move |mut world: Voxels| {
            for &(pos, state) in &voxels {
                assert!(world.set(pos, state), "{pos} isn't loaded");
            }
        })
        .unwrap();
    app.update();
    app.update();
}

fn get(app: &mut App, pos: IVec3) -> BlockState {
    app.world_mut()
        .run_system_once(move |voxels: Voxels| voxels.get(pos).unwrap())
        .unwrap()
}

/// Runs frames until `n` more network ticks have run.
fn ticks(app: &mut App, n: u64) {
    let until = app.world().resource::<NetworkClock>().ticks + n;
    while app.world().resource::<NetworkClock>().ticks < until {
        app.update();
    }
    app.update();
}

fn entity(app: &App, pos: IVec3) -> Entity {
    app.world().resource::<BlockEntities>().get(pos).unwrap()
}

/// A creative generator powers a furnace over a wire: it lights up while
/// it smelts two ores, puts out two ingots, and goes dark again. The
/// generators' and the furnace's energy all adds up.
#[test]
fn a_powered_furnace_smelts_and_lights_up() {
    let mut app = app();
    let furnace = IVec3::new(2, 4, 4);
    // The furnace faces south, so its back (the power) is north; the
    // generator faces north, so its back is south.
    let lit = |facing: &str| block_with("electric_furnace", &[("facing", facing), ("lit", "true")]);
    set(
        &mut app,
        vec![
            (
                furnace,
                block_with("electric_furnace", &[("facing", "south")]),
            ),
            (IVec3::new(2, 4, 3), block("wire")),
            (IVec3::new(2, 4, 2), block("creative_generator")),
        ],
    );
    let e = entity(&app, furnace);
    app.world_mut()
        .get_mut::<Slots>(e)
        .unwrap()
        .group_mut("input")
        .unwrap()
        .insert(base(), ItemStack::new(ore(STONE, IRON), 2));
    ticks(&mut app, 20);
    assert_eq!(get(&mut app, furnace), lit("south"));
    assert!(app.world().get::<MachineActive>(e).unwrap().0);
    let progress = app.world().get::<RecipeProgress>(e).unwrap();
    assert_eq!(progress.status, RecipeStatus::Running);
    ticks(&mut app, 180);
    let slots = app.world().get::<Slots>(e).unwrap();
    assert_eq!(
        slots.group("output").unwrap().slots[0],
        Some(ItemStack::new(ingot(IRON), 2))
    );
    assert_eq!(slots.group("input").unwrap().total(), 0);
    assert_eq!(
        get(&mut app, furnace),
        block_with("electric_furnace", &[("facing", "south")])
    );
    // Energy in the buffers is what was generated less what was used.
    let stored: u64 = app
        .world_mut()
        .query::<&EnergyBuffer>()
        .iter(app.world())
        .map(|b| b.stored)
        .sum();
    let report = app.world().resource::<EnergyReport>();
    assert_eq!(stored + report.used, report.generated);
    assert!(report.used >= 4000);
}

/// Two furnaces on a generator that just covers both their recipes both
/// run: neither fills its buffer with the power the other needs, even
/// one that could take twenty times what the generator gives.
#[test]
fn two_furnaces_share_a_generator_that_covers_both() {
    let mut app = app();
    let generator = IVec3::new(4, 4, 2);
    let furnaces = [IVec3::new(2, 4, 4), IVec3::new(6, 4, 4)];
    let furnace = block_with("electric_furnace", &[("facing", "south")]);
    let mut voxels = vec![(generator, block("combustion_generator"))];
    voxels.extend((2..=6).map(|x| (IVec3::new(x, 4, 3), block("wire"))));
    voxels.extend(furnaces.map(|pos| (pos, furnace)));
    set(&mut app, voxels);
    let e = entity(&app, generator);
    let mut world = app.world_mut().entity_mut(e);
    let mut buffer = world.get_mut::<EnergyBuffer>().unwrap();
    buffer.stored = buffer.capacity;
    world
        .get_mut::<Slots>()
        .unwrap()
        .group_mut("fuel")
        .unwrap()
        .insert(base(), ItemStack::new(log(), 8));
    for pos in furnaces {
        let e = entity(&app, pos);
        app.world_mut()
            .get_mut::<Slots>(e)
            .unwrap()
            .group_mut("input")
            .unwrap()
            .insert(base(), ItemStack::new(ore(STONE, IRON), 8));
    }
    // Once it's lit (which sets its buffer's rates from its state), the
    // first could take twenty times what the generator gives.
    ticks(&mut app, 2);
    let greedy = entity(&app, furnaces[0]);
    assert!(app.world().get::<MachineActive>(greedy).unwrap().0);
    app.world_mut()
        .get_mut::<EnergyBuffer>(greedy)
        .unwrap()
        .max_in = 20_000;
    // Ten seconds: two ores' worth each, and a tick or two to spare.
    ticks(&mut app, 200);
    for pos in furnaces {
        let e = entity(&app, pos);
        let slots = app.world().get::<Slots>(e).unwrap();
        let done = slots.group("output").unwrap().total();
        assert!(done >= 2, "the furnace at {pos} smelted {done}");
        let progress = app.world().get::<RecipeProgress>(e).unwrap();
        assert_eq!(progress.status, RecipeStatus::Running, "at {pos}");
    }
}

/// A full generator feeding a 20 W lamp burns one tick in fifty, waiting
/// for room in between: it stays lit the whole time rather than blinking,
/// and goes dark once its fuel has burnt.
#[test]
fn a_waiting_generator_stays_lit() {
    let mut app = app();
    let generator = IVec3::new(2, 4, 2);
    // The generator faces north, so its back (the power) is south.
    set(
        &mut app,
        vec![
            (generator, block("combustion_generator")),
            (IVec3::new(2, 4, 3), block("wire")),
            (IVec3::new(2, 4, 4), block("powered_lamp")),
        ],
    );
    let e = entity(&app, generator);
    let mut world = app.world_mut().entity_mut(e);
    let mut buffer = world.get_mut::<EnergyBuffer>().unwrap();
    // Room for two ticks of burning: it starts, then waits for room.
    buffer.stored = buffer.capacity - 100;
    world
        .get_mut::<Slots>()
        .unwrap()
        .group_mut("fuel")
        .unwrap()
        .insert(base(), ItemStack::new(log(), 1));
    ticks(&mut app, 2);
    for _ in 0..200 {
        ticks(&mut app, 1);
        assert!(app.world().get::<MachineActive>(e).unwrap().0);
    }
    let progress = app.world().get::<RecipeProgress>(e).unwrap();
    assert_eq!(progress.status, RecipeStatus::OutputFull);
    assert!(progress.ticks > 0);
}

/// An unpowered furnace with an ore waits for energy; once the ore is
/// taken out it's idle, through the shortcut for empty machines, which
/// copies nothing.
#[test]
fn an_emptied_furnace_goes_idle() {
    let mut app = app();
    let furnace = IVec3::new(2, 4, 4);
    set(&mut app, vec![(furnace, block("electric_furnace"))]);
    let e = entity(&app, furnace);
    let input = |app: &mut App| {
        let mut slots = app.world_mut().get_mut::<Slots>(e).unwrap();
        slots.group_mut("input").unwrap().slots[0].take()
    };
    app.world_mut()
        .get_mut::<Slots>(e)
        .unwrap()
        .group_mut("input")
        .unwrap()
        .insert(base(), ItemStack::new(ore(STONE, IRON), 1));
    ticks(&mut app, 2);
    let progress = app.world().get::<RecipeProgress>(e).unwrap();
    assert_eq!(progress.status, RecipeStatus::NoEnergy);
    assert_eq!(progress.recipe, None);
    assert!(input(&mut app).is_some());
    ticks(&mut app, 2);
    let progress = app.world().get::<RecipeProgress>(e).unwrap();
    assert_eq!(progress.status, RecipeStatus::Idle);
    assert!(!app.world().get::<MachineActive>(e).unwrap().0);
    // Idle ticks after that change nothing.
    let changed = |app: &App| {
        let entity = app.world().entity(e);
        (
            entity.get_ref::<RecipeProgress>().unwrap().last_changed(),
            entity.get_ref::<Slots>().unwrap().last_changed(),
        )
    };
    let before = changed(&app);
    ticks(&mut app, 5);
    assert_eq!(changed(&app), before);
}
