use std::{path::Path, sync::OnceLock};

use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::{
    block_entity::{BlockEntityPart, BlockEntityPlugin, Value},
    connections::ConnectionsPlugin,
    content::{
        BlockState, Direction, MODS_DIR, PortRole,
        load::{ModFiles, read_mods},
    },
    inventory::ItemStack,
    light::LightPlugin,
    network::{NetworkPlugin, Networks},
    save::{WorldFolder, tests::temp_folder},
    streaming::{ChunkLoader, StreamingPlugin, StreamingSettings, StreamingStatus},
    terrain::TerrainPlugin,
    voxel_material::BlockMaterial,
};

/// The base game, and mod `test` with an `oven`: a machine facing a way,
/// lit while it works, with power at its back, a tank and slots.
fn content() -> &'static Content {
    static CONTENT: OnceLock<Content> = OnceLock::new();
    CONTENT.get_or_init(|| {
        let (mut mods, errors) = read_mods(Path::new("assets/mods"), MODS_DIR);
        assert!(errors.is_empty(), "{errors:?}");
        mods.push(ModFiles::new(
            "mods/test",
            r#"{ id: "test", name: "Test", dependencies: ["defaria"] }"#,
            &[(
                "blocks/oven.json5",
                r#"{
                    properties: {
                        lit: ["false", "true"],
                        facing: { values: ["north", "south", "east", "west"], place: "look" },
                    },
                    model: "defaria:cube",
                    textures: { all: "defaria:machine_side" },
                    variants: [{ when: { lit: "true" }, light: [10, 6, 2] }],
                    machine: {
                        ports: [
                            { faces: ["back"], network: "defaria:electric", role: "consume" },
                            { faces: ["left"], network: "defaria:fluid", role: "input" },
                            { faces: ["right"], network: "defaria:item", role: "input", slots: "input" },
                        ],
                        energy: { capacity: 500, max_in: 50 },
                        tanks: { water: { capacity: 2, accepts: ["defaria:water"] } },
                        slots: { input: 2, output: 1 },
                        active_state: { lit: "true" },
                    },
                }"#,
            )],
        ));
        let content = Content::from_mods(mods);
        assert!(content.errors.is_empty(), "{:#?}", content.errors);
        content
    })
}

fn state(id: &str, values: &[(&str, &str)]) -> BlockState {
    content().block_state(id, values).unwrap()
}

fn oven(facing: &str) -> BlockState {
    state("test:oven", &[("facing", facing)])
}

fn app(folder: Option<&Path>) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, AssetPlugin::default()))
        .insert_resource(content().clone())
        .add_plugins((
            TerrainPlugin,
            LightPlugin,
            StreamingPlugin,
            BlockEntityPlugin,
            ConnectionsPlugin,
            NetworkPlugin,
            MachinePlugin,
        ))
        .init_asset::<Mesh>()
        .insert_resource(BlockMaterial::unloaded(Handle::default()))
        .insert_resource(StreamingSettings {
            radius: 2,
            above: 1,
            below: 1,
            ..default()
        });
    if let Some(folder) = folder {
        app.insert_resource(WorldFolder(folder.to_path_buf()));
    }
    app.world_mut()
        .spawn((ChunkLoader, Transform::from_translation(HOME)));
    app.update();
    app.update();
    for _ in 0..2000 {
        app.update();
        if app.world().resource::<StreamingStatus>().settled {
            return app;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("never settled");
}

const HOME: Vec3 = Vec3::new(8.0, 40.0, 8.0);
const POS: IVec3 = IVec3::new(3, 50, -5);

fn set(app: &mut App, pos: IVec3, state: BlockState) {
    app.world_mut()
        .run_system_once(move |mut voxels: Voxels| assert!(voxels.set(pos, state)))
        .unwrap();
    app.update();
    app.update();
}

fn get(app: &mut App, pos: IVec3) -> BlockState {
    app.world_mut()
        .run_system_once(move |voxels: Voxels| voxels.get(pos).unwrap())
        .unwrap()
}

fn part<P: Component + Clone>(app: &App, pos: IVec3) -> Option<P> {
    let entity = app.world().resource::<BlockEntities>().get(pos)?;
    app.world().get::<P>(entity).cloned()
}

fn edit<P: Component<Mutability = bevy::ecs::component::Mutable>>(
    app: &mut App,
    pos: IVec3,
    change: impl FnOnce(&mut P),
) {
    let entity = app.world().resource::<BlockEntities>().get(pos).unwrap();
    change(&mut app.world_mut().get_mut::<P>(entity).unwrap());
    app.update();
    app.update();
}

fn item(id: &str) -> crate::content::ItemState {
    content().default_item(id).unwrap()
}

/// Placing a machine gives its block entity its parts, from its block.
#[test]
fn placing_a_machine_gives_it_its_parts() {
    let mut app = app(None);
    set(&mut app, POS, oven("north"));
    assert_eq!(
        part::<EnergyBuffer>(&app, POS),
        Some(EnergyBuffer {
            stored: 0,
            capacity: 500,
            max_in: 50,
            max_out: 0,
            generate: 0,
            uses: 0,
        })
    );
    let tanks = part::<Tanks>(&app, POS).unwrap();
    assert_eq!(tanks.0.len(), 1);
    assert_eq!(tanks.0[0].capacity, 2000);
    let slots = part::<Slots>(&app, POS).unwrap();
    let groups: Vec<(&str, usize)> = slots
        .0
        .iter()
        .map(|g| (g.name.as_str(), g.slots.len()))
        .collect();
    assert_eq!(groups, [("input", 2), ("output", 1)]);
    let ports = part::<Ports>(&app, POS).unwrap();
    let faces: Vec<(Direction, &str, PortRole)> = ports
        .0
        .iter()
        .map(|p| (p.face, p.kind.as_str(), p.role))
        .collect();
    assert_eq!(
        faces,
        [
            (Direction::South, "defaria:electric", PortRole::Consume),
            (Direction::West, "defaria:fluid", PortRole::Input),
            (Direction::East, "defaria:item", PortRole::Input),
        ]
    );
    assert_eq!(part::<MachineActive>(&app, POS), Some(MachineActive(false)));
    // A battery has a buffer but no tanks or slots.
    let battery = IVec3::new(6, 50, -5);
    set(&mut app, battery, state("defaria:battery", &[]));
    assert!(part::<EnergyBuffer>(&app, battery).is_some());
    assert!(part::<Tanks>(&app, battery).is_none());
    assert!(part::<Slots>(&app, battery).is_none());
}

/// What a machine holds survives a restart; its ports are worked out
/// again.
#[test]
fn machines_keep_what_they_hold_through_a_restart() {
    let folder = temp_folder("machines");
    let stone = item("defaria:stone");
    let pickaxe = item("defaria:pickaxe");
    {
        let mut app = app(Some(&folder));
        set(&mut app, POS, oven("east"));
        edit::<EnergyBuffer>(&mut app, POS, |buffer| {
            assert_eq!(buffer.fill(320), 320);
        });
        edit::<Tanks>(&mut app, POS, |tanks| {
            assert_eq!(tanks.0[0].fill("defaria:water", 1500), 1500);
        });
        edit::<Slots>(&mut app, POS, |slots| {
            let input = slots.group_mut("input").unwrap();
            assert_eq!(input.insert(content(), ItemStack::new(stone, 70)), None);
            let output = slots.group_mut("output").unwrap();
            assert_eq!(output.insert(content(), ItemStack::one(pickaxe)), None);
        });
        app.world_mut().write_message(AppExit::Success);
        app.update();
    }
    let app = app(Some(&folder));
    assert_eq!(part::<EnergyBuffer>(&app, POS).unwrap().stored, 320);
    let tanks = part::<Tanks>(&app, POS).unwrap();
    assert_eq!(tanks.total("defaria:water"), 1500);
    let slots = part::<Slots>(&app, POS).unwrap();
    assert_eq!(
        slots.group("input").unwrap().slots,
        [
            Some(ItemStack::new(stone, 64)),
            Some(ItemStack::new(stone, 6))
        ]
    );
    assert_eq!(
        slots.group("output").unwrap().slots,
        [Some(ItemStack::one(pickaxe))]
    );
    assert_eq!(part::<Ports>(&app, POS).unwrap().0[0].face, Direction::West);
}

/// A machine that's working is in its block's `active_state`, and back
/// in the defaults when it stops, keeping its other properties and what
/// it holds.
#[test]
fn working_machines_take_their_active_state() {
    let mut app = app(None);
    set(&mut app, POS, oven("west"));
    edit::<EnergyBuffer>(&mut app, POS, |buffer| {
        buffer.fill(7);
    });
    edit::<MachineActive>(&mut app, POS, |active| active.0 = true);
    assert_eq!(
        get(&mut app, POS),
        state("test:oven", &[("facing", "west"), ("lit", "true")])
    );
    assert_eq!(part::<EnergyBuffer>(&app, POS).unwrap().stored, 7);
    edit::<MachineActive>(&mut app, POS, |active| active.0 = false);
    assert_eq!(get(&mut app, POS), oven("west"));
    // Placed lit, it starts out working.
    let lit = IVec3::new(6, 50, -5);
    set(
        &mut app,
        lit,
        state("test:oven", &[("facing", "west"), ("lit", "true")]),
    );
    assert_eq!(part::<MachineActive>(&app, lit), Some(MachineActive(true)));
}

/// Turning a machine turns its ports, and keeps what it holds.
#[test]
fn ports_turn_with_the_machine() {
    let mut app = app(None);
    set(&mut app, POS, oven("north"));
    edit::<EnergyBuffer>(&mut app, POS, |buffer| {
        buffer.fill(9);
    });
    set(&mut app, POS, oven("south"));
    let ports = part::<Ports>(&app, POS).unwrap();
    assert_eq!(ports.0[0].face, Direction::North, "its back");
    assert_eq!(ports.0[1].face, Direction::East, "its left");
    assert_eq!(part::<EnergyBuffer>(&app, POS).unwrap().stored, 9);
}

/// The base game's machines that face a way face whoever places them:
/// the opposite of the way they look.
#[test]
fn base_machines_face_you_as_you_place_them() {
    let content = content();
    let mut checked = 0;
    for block in content
        .blocks
        .iter()
        .filter(|block| block.id.starts_with("defaria:"))
    {
        let Some(placed) = content.default_state(&block.id) else {
            continue;
        };
        let has_facing = content
            .state(placed)
            .values
            .iter()
            .any(|(name, _)| name == "facing");
        if content.machine(placed).is_none() || !has_facing {
            continue;
        }
        for look in [
            Direction::North,
            Direction::South,
            Direction::East,
            Direction::West,
        ] {
            let state = content.placed(placed, Direction::Up, look.normal().as_vec3());
            let facing = content
                .state(state)
                .values
                .iter()
                .find(|(name, _)| name == "facing");
            assert_eq!(
                facing.map(|(_, value)| value.as_str()),
                Some(look.opposite().name()),
                "{} placed looking {}",
                block.id,
                look.name()
            );
        }
        checked += 1;
    }
    assert!(checked > 0);
}

/// Each network lists the machine ports joined to it, with their roles
/// and block entities, and each machine knows the networks on its ports.
#[test]
fn networks_list_their_machine_ports() {
    let mut app = app(None);
    let wire = state("defaria:wire", &[]);
    let pipe = state("defaria:pipe", &[]);
    set(&mut app, POS, oven("north"));
    // A wire to its back, one to its front (no port), a pipe to its left.
    let back = POS + IVec3::Z;
    set(&mut app, back, wire);
    set(&mut app, back + IVec3::Z, wire);
    set(&mut app, POS - IVec3::Z, wire);
    set(&mut app, POS - IVec3::X, pipe);
    let networks = app.world().resource::<Networks>();
    let power = networks.at(back).unwrap();
    let front = networks.at(POS - IVec3::Z).unwrap();
    let fluid = networks.at(POS - IVec3::X).unwrap();
    let entity = app.world().resource::<BlockEntities>().get(POS);
    let index = app.world().resource::<MachinePorts>();
    assert_eq!(
        index.on(power),
        [MachinePortRef {
            pos: POS,
            face: Direction::South,
            role: PortRole::Consume,
            tank: None,
            slots: None,
            filter: None,
            state: oven("north"),
            entity,
        }]
    );
    assert!(index.on(front).is_empty());
    assert_eq!(index.on(fluid)[0].role, PortRole::Input);
    assert_eq!(index.on(fluid)[0].tank, Some(0));
    let ports = part::<Ports>(&app, POS).unwrap();
    let on: Vec<_> = ports.0.iter().map(|p| p.network).collect();
    assert_eq!(on, [Some(power), Some(fluid), None]);
    // Headless `network` names each port's role.
    let line = crate::network::describe(networks, content(), back, |id, port| index.role(id, port));
    assert!(line.ends_with("ports: 3 50 -5 south (consume)"), "{line}");
    // Breaking the wire takes the port off the network.
    set(&mut app, back, BlockState::AIR);
    let index = app.world().resource::<MachinePorts>();
    let networks = app.world().resource::<Networks>();
    let rest = networks.at(back + IVec3::Z).unwrap();
    assert!(index.on(rest).is_empty());
    assert_eq!(part::<Ports>(&app, POS).unwrap().0[0].network, None);
}

/// How many frames rebuilt the machine ports index.
#[derive(Resource, Default)]
struct Rebuilds(u32);

/// The machine ports index is rebuilt when the networks or the machines
/// change, not for every voxel that does.
#[test]
fn the_port_index_is_rebuilt_only_when_what_it_reads_changes() {
    let mut app = app(None);
    app.init_resource::<Rebuilds>().add_systems(
        Update,
        (|index: Res<MachinePorts>, mut rebuilds: ResMut<Rebuilds>| {
            rebuilds.0 += index.is_changed() as u32;
        })
        .after(super::ports::index_ports),
    );
    let rebuilds = |app: &App| app.world().resource::<Rebuilds>().0;
    set(&mut app, POS, oven("north"));
    set(&mut app, POS + IVec3::Z, state("defaria:wire", &[]));
    let before = rebuilds(&app);
    set(
        &mut app,
        POS + IVec3::new(4, 0, 0),
        state("defaria:stone", &[]),
    );
    set(
        &mut app,
        POS + IVec3::new(0, 1, 0),
        state("defaria:stone", &[]),
    );
    assert_eq!(rebuilds(&app), before);
    // A second wire joins the network: rebuilt.
    set(&mut app, POS + IVec3::Z * 2, state("defaria:wire", &[]));
    assert!(rebuilds(&app) > before);
}

#[test]
fn machine_parts_are_reflected() {
    let mut app = app(None);
    set(&mut app, POS, oven("north"));
    let registry = app.world().resource::<AppTypeRegistry>().read();
    for name in ["EnergyBuffer", "Tanks", "Slots", "Ports", "MachineActive"] {
        let registration = registry
            .get_with_short_type_path(name)
            .unwrap_or_else(|| panic!("{name}"));
        assert!(registration.data::<ReflectComponent>().is_some(), "{name}");
    }
}

#[test]
fn buffers_never_make_or_lose_energy() {
    let mut buffer = EnergyBuffer {
        capacity: 100,
        ..default()
    };
    let mut outside: u64 = 1000;
    for amount in [30, 90, 5, 200, 0] {
        let taken = buffer.fill(amount.min(outside));
        outside -= taken;
        assert_eq!(outside + buffer.stored, 1000);
        let given = buffer.drain(amount / 3);
        outside += given;
        assert_eq!(outside + buffer.stored, 1000);
        assert!(buffer.stored <= buffer.capacity);
    }
    let left = buffer.stored;
    assert_eq!(buffer.drain(1000), left);
    assert_eq!(buffer.stored, 0);
}

#[test]
fn tanks_hold_one_fluid_they_accept() {
    let mut tank = Tank {
        name: "t".into(),
        capacity: 1000,
        accepts: vec!["defaria:water".into(), "defaria:lava".into()],
        ..default()
    };
    assert_eq!(tank.fill("defaria:honey", 10), 0);
    assert_eq!(tank.fill("defaria:water", 700), 700);
    assert_eq!(tank.fill("defaria:lava", 10), 0, "it holds water");
    assert_eq!(tank.fill("defaria:water", 700), 300);
    assert_eq!(tank.drain(400), 400);
    assert_eq!(tank.fluid.as_deref(), Some("defaria:water"));
    assert_eq!(tank.drain(1000), 600);
    assert_eq!(tank.fluid, None);
    assert_eq!(tank.fill("defaria:lava", 10), 10, "empty, it takes lava");
}

#[test]
fn slots_hold_items_without_losing_any() {
    let content = content();
    let stone = item("defaria:stone");
    let dirt = crate::content::dirt_item();
    let pickaxe = item("defaria:pickaxe");
    let mut group = SlotGroup {
        name: "g".into(),
        slots: vec![None; 3],
    };
    assert_eq!(
        group.room_for(content, &ItemStack::one(stone)),
        3 * SLOT_SIZE
    );
    assert_eq!(group.insert(content, ItemStack::new(stone, 100)), None);
    assert_eq!(
        group.insert(content, ItemStack::new(dirt, 50)),
        None,
        "the third slot"
    );
    // Only the second stone slot has room: 28 more.
    assert_eq!(
        group.insert(content, ItemStack::new(stone, 40)),
        Some(ItemStack::new(stone, 12))
    );
    assert_eq!(group.total(), 128 + 50);
    assert_eq!(
        group.insert(content, ItemStack::one(pickaxe)),
        Some(ItemStack::one(pickaxe))
    );
    assert_eq!(group.take(2, 60), Some(ItemStack::new(dirt, 50)));
    assert_eq!(group.slots[2], None);
    assert_eq!(group.take(2, 1), None);
    // A unique item takes a slot of its own.
    assert_eq!(
        group.insert(content, ItemStack::new(pickaxe, 2)),
        Some(ItemStack::one(pickaxe))
    );
    assert_eq!(group.slots[2], Some(ItemStack::one(pickaxe)));
}

#[test]
fn parts_round_trip_through_their_saved_form() {
    let content = content();
    let state = oven("north");
    let mut slots = Slots::new(state, content).unwrap();
    slots
        .group_mut("output")
        .unwrap()
        .insert(content, ItemStack::new(item("defaria:planks"), 3));
    let saved = slots.save(content);
    assert_eq!(Slots::load(&saved, 1, state, content), Some(slots));
    let mut tanks = Tanks::new(state, content).unwrap();
    tanks.0[0].fill("defaria:water", 12);
    let saved = tanks.save(content);
    assert_eq!(Tanks::load(&saved, 1, state, content), Some(tanks));
    // A slot with an item that's gone comes back empty.
    let saved = Value::Table(
        [(
            "input".to_string(),
            Value::List(vec![Value::Table(
                [
                    ("item".to_string(), Value::Text("defaria:nothing".into())),
                    ("count".to_string(), Value::Int(4)),
                ]
                .into(),
            )]),
        )]
        .into(),
    );
    let loaded = Slots::load(&saved, 1, state, content).unwrap();
    assert_eq!(loaded.total(), 0);
    // Ports and the active flag aren't saved, but worked out again.
    const { assert!(!<Ports as BlockEntityPart>::SAVED) };
    const { assert!(!<MachineActive as BlockEntityPart>::SAVED) };
}
