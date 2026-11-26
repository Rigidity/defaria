use super::*;
use crate::content::{Joins, base, test_mod};

fn network(content: &Content, id: &str) -> usize {
    content.network(id).unwrap()
}

/// Faces of the ports of `state` on network kind `kind`, in order.
fn port_faces(content: &Content, state: BlockState, kind: &str) -> Vec<Direction> {
    let kind = network(content, kind);
    content
        .machine(state)
        .unwrap()
        .ports
        .iter()
        .filter(|port| port.network == kind)
        .map(|port| port.face)
        .collect()
}

#[test]
fn base_machines_have_their_ports_and_stores() {
    let content = base();
    let battery = content.default_state("defaria:battery").unwrap();
    let machine = content.machine(battery).unwrap();
    assert_eq!(
        port_faces(content, battery, "defaria:electric"),
        [Direction::North, Direction::South]
    );
    assert!(machine.ports.iter().all(|p| p.role == PortRole::Store));
    assert_eq!(
        machine.energy,
        Some(EnergySpec {
            capacity: 100_000,
            max_in: 1000,
            max_out: 1000,
            generate: 0,
            max_generate: 0,
            uses: 0,
        })
    );
    assert!(content.block_of(battery).has_block_entity());

    let tank = content.default_state("defaria:tank").unwrap();
    let machine = content.machine(tank).unwrap();
    assert_eq!(
        port_faces(content, tank, "defaria:fluid"),
        [Direction::Up, Direction::Down]
    );
    assert_eq!(
        machine.tanks,
        [TankSpec {
            name: "fluid".into(),
            capacity: 16 * FULL_FLUID as u64,
            accepts: vec!["defaria:water".into(), "defaria:lava".into()],
        }]
    );
    assert!(machine.ports.iter().all(|p| p.tank == Some(0)));

    let crate_ = content.default_state("defaria:crate").unwrap();
    let machine = content.machine(crate_).unwrap();
    assert_eq!(
        machine.slots,
        [SlotGroupSpec {
            name: "items".into(),
            count: 9
        }]
    );
    let roles: Vec<_> = machine.ports.iter().map(|p| (p.face, p.role)).collect();
    assert_eq!(
        roles,
        [
            (Direction::Up, PortRole::Input),
            (Direction::Down, PortRole::Output)
        ]
    );
    // A casing is just a block.
    let casing = content.default_state("defaria:machine_casing").unwrap();
    assert!(content.machine(casing).is_none());
    assert!(!content.block_of(casing).has_block_entity());
}

#[test]
fn relative_faces_follow_facing() {
    let content = base();
    let battery = |facing| {
        content
            .block_state("defaria:battery", &[("facing", facing)])
            .unwrap()
    };
    for (facing, faces) in [
        ("north", [Direction::North, Direction::South]),
        ("south", [Direction::South, Direction::North]),
        ("east", [Direction::East, Direction::West]),
        ("west", [Direction::West, Direction::East]),
    ] {
        assert_eq!(
            port_faces(content, battery(facing), "defaria:electric"),
            faces,
            "{facing}"
        );
    }
    use Direction::*;
    for (facing, left) in [
        (North, West),
        (West, South),
        (South, East),
        (East, North),
        (Up, West),
        (Down, West),
    ] {
        assert_eq!(resolve_face(PortFace::Left, Some(facing)), Some(left));
        assert_eq!(
            resolve_face(PortFace::Right, Some(facing)),
            Some(left.opposite())
        );
        assert_eq!(
            resolve_face(PortFace::Back, Some(facing)),
            Some(facing.opposite())
        );
    }
    assert_eq!(resolve_face(PortFace::Front, None), None);
}

/// A machine whose `rotate` turns it: absolute port faces turn with it.
const TURNED: &str = r#"{
    model: "quad",
    textures: { all: "a" },
    properties: { axis: ["z", "x", "y"] },
    variants: [
        { when: { axis: "x" }, rotate: [0, 90, 0] },
        { when: { axis: "y" }, rotate: [90, 0, 0] },
    ],
    machine: {
        ports: [{ faces: ["north"], network: "power", role: "consume" }],
        energy: { capacity: 10 },
    },
}"#;

const POWER: (&str, &str) = (
    "networks/power.json5",
    r#"{ name: "Power", solver: "energy" }"#,
);

#[test]
fn ports_turn_with_rotate() {
    let content = test_mod(&[POWER, ("blocks/m.json5", TURNED)]);
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    let state = |axis| content.block_state("test:m", &[("axis", axis)]).unwrap();
    // About Y, north goes to west, as the model's faces do; about X, north
    // goes down.
    assert_eq!(
        port_faces(&content, state("z"), "test:power"),
        [Direction::North]
    );
    assert_eq!(
        port_faces(&content, state("x"), "test:power"),
        [Direction::West]
    );
    assert_eq!(
        port_faces(&content, state("y"), "test:power"),
        [Direction::Up]
    );
    let kind = network(&content, "test:power");
    assert!(content.has_port(state("x"), kind, Direction::West));
    assert!(!content.has_port(state("x"), kind, Direction::North));
}

#[test]
fn conduits_join_only_port_faces_of_their_kind() {
    let content = base();
    let battery = content.default_state("defaria:battery").unwrap();
    let wire = content.default_state("defaria:wire").unwrap();
    let pipe = content.default_state("defaria:pipe").unwrap();
    let joins = |state| content.joins(state).unwrap();
    assert!(content.accepts(battery, joins(wire), Direction::North));
    assert!(content.accepts(battery, joins(wire), Direction::South));
    assert!(!content.accepts(battery, joins(wire), Direction::East));
    assert!(!content.accepts(battery, joins(wire), Direction::Up));
    assert!(!content.accepts(battery, joins(pipe), Direction::North));
    // A wire north of the battery joins it on its south side.
    let state = content.connected(wire, |side| {
        Some(if side == Direction::South {
            battery
        } else {
            BlockState::AIR
        })
    });
    assert!(content.joined(state, Direction::South));
    let state = content.connected(wire, |side| {
        Some(if side == Direction::West {
            battery
        } else {
            BlockState::AIR
        })
    });
    assert!(!content.joined(state, Direction::West));
    // The machine itself doesn't connect: it has no connection properties.
    assert!(!content.connects(battery));
    assert!(!content.accepts(battery, Joins::Rule("defaria:anything"), Direction::North));
}

#[test]
fn numbers_come_from_materials() {
    let content = test_mod(&[
        POWER,
        (
            "materials/copper.json5",
            r##"{ tags: ["metal"], palette: ["#000000", "#ffffff"], hardness: 1, weight: 1, stats: { conductivity: 1 } }"##,
        ),
        (
            "materials/iron.json5",
            r##"{ tags: ["metal"], palette: ["#000000", "#ffffff"], hardness: 1, weight: 1, stats: { conductivity: 0.25 } }"##,
        ),
        (
            "blocks/cell.json5",
            r##"{
                model: "quad",
                textures: { all: "a" },
                properties: { metal: { materials: "#metal" } },
                machine: {
                    energy: {
                        capacity: { base: 1000, stat: "conductivity", materials: { metal: 1 } },
                        max_in: 5,
                    },
                    tanks: { t: { capacity: { base: 2, stat: "conductivity", materials: { metal: 1 } } } },
                },
            }"##,
        ),
    ]);
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    let cell = |metal| {
        content
            .machine(
                content
                    .block_state("test:cell", &[("metal", metal)])
                    .unwrap(),
            )
            .unwrap()
            .clone()
    };
    let copper = cell("test:copper");
    assert_eq!(
        copper.energy,
        Some(EnergySpec {
            capacity: 1000,
            max_in: 5,
            max_out: 0,
            generate: 0,
            max_generate: 0,
            uses: 0,
        })
    );
    assert_eq!(copper.tanks[0].capacity, 2000);
    let iron = cell("test:iron");
    assert_eq!(iron.energy.unwrap().capacity, 250);
    assert_eq!(iron.tanks[0].capacity, 500);
}

#[test]
fn active_state_names_the_working_and_idle_states() {
    let content = test_mod(&[
        POWER,
        (
            "blocks/oven.json5",
            r#"{
                model: "quad",
                textures: { all: "a" },
                properties: {
                    lit: ["false", "true"],
                    facing: ["north", "south", "east", "west"],
                },
                machine: {
                    ports: [{ faces: ["back"], network: "power", role: "consume" }],
                    energy: { capacity: 10, max_in: 1 },
                    active_state: { lit: "true" },
                },
            }"#,
        ),
        (
            "blocks/box.json5",
            r#"{ model: "quad", textures: { all: "a" }, machine: { slots: { a: 1 } } }"#,
        ),
    ]);
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    let oven = |lit, facing| {
        content
            .block_state("test:oven", &[("lit", lit), ("facing", facing)])
            .unwrap()
    };
    for state in [oven("false", "east"), oven("true", "east")] {
        let machine = content.machine(state).unwrap();
        assert_eq!(machine.active, oven("true", "east"));
        assert_eq!(machine.idle, oven("false", "east"));
        assert_eq!(machine.state(true), oven("true", "east"));
        assert_eq!(port_faces(&content, state, "test:power"), [Direction::West]);
    }
    // Without `active_state`, both are itself.
    let block = content.default_state("test:box").unwrap();
    let machine = content.machine(block).unwrap();
    assert_eq!((machine.active, machine.idle), (block, block));
    assert!(machine.ports.is_empty());
}

#[test]
fn machine_mistakes_are_reported() {
    let fluid_net = (
        "networks/pipes.json5",
        r#"{ name: "Pipes", solver: "fluid" }"#,
    );
    let items_net = (
        "networks/tubes.json5",
        r#"{ name: "Tubes", solver: "item" }"#,
    );
    let water = (
        "blocks/water.json5",
        r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 12 } } }"##,
    );
    for (machine, error) in [
        (
            r#"{ ports: [{ faces: ["up"], network: "pipes", role: "store", tank: "out" }], tanks: { in: { capacity: 1 } } }"#,
            "names tank `out`",
        ),
        (
            r#"{ ports: [{ faces: ["up"], network: "steam", role: "store" }] }"#,
            "no network kind `test:steam`",
        ),
        (
            r#"{ ports: [{ faces: ["up"], network: "power", role: "input" }], energy: { capacity: 1 } }"#,
            "not `input`",
        ),
        (
            r#"{ ports: [{ faces: ["up"], network: "pipes", role: "consume" }], tanks: { a: { capacity: 1 } } }"#,
            "not `consume`",
        ),
        (
            r#"{ ports: [{ faces: ["up"], network: "power", role: "consume" }] }"#,
            "needs the machine's `energy`",
        ),
        (
            r#"{ ports: [{ faces: ["up"], network: "pipes", role: "store" }] }"#,
            "has no tanks",
        ),
        (
            r#"{ ports: [{ faces: ["up"], network: "pipes", role: "store" }], tanks: { a: { capacity: 1 }, b: { capacity: 1 } } }"#,
            "say which with `tank`",
        ),
        (
            r#"{ ports: [{ faces: ["up"], network: "tubes", role: "input", slots: "x" }], slots: { a: 1 } }"#,
            "names slot group `x`",
        ),
        (
            r#"{ ports: [{ faces: ["up"], network: "tubes", role: "input", tank: "a" }], tanks: { a: { capacity: 1 } }, slots: { s: 1 } }"#,
            "only a port on a fluid network has a `tank`",
        ),
        (
            r#"{ ports: [{ faces: ["front"], network: "power", role: "consume" }], energy: { capacity: 1 } }"#,
            "need a `facing` property",
        ),
        (
            r#"{ ports: [{ faces: ["up", "up"], network: "power", role: "consume" }], energy: { capacity: 1 } }"#,
            "two ports",
        ),
        (
            r#"{ ports: [{ faces: [], network: "power", role: "consume" }], energy: { capacity: 1 } }"#,
            "`faces` is empty",
        ),
        (
            r#"{ ports: [{ faces: ["sideways"], network: "power", role: "consume" }] }"#,
            "sideways",
        ),
        (r#"{ slots: { a: 0 } }"#, "has 0 slots"),
        (
            r#"{ active_state: { lit: "true" } }"#,
            "`lit`, which isn't a property",
        ),
        (r#"{ active_state: { mode: "on" } }"#, "`mode` to `on`"),
        (
            r#"{ active_state: { mode: "off" } }"#,
            "`mode` to `off`, its default",
        ),
        (r#"{ energy: { capacity: -5 } }"#, "at least 0"),
        (
            r#"{ tanks: { a: { capacity: 1, accepts: ["a_block"] } } }"#,
            "`test:a_block`, which isn't a fluid",
        ),
        (
            r#"{ tanks: { a: { capacity: 1, accepts: ["gone"] } } }"#,
            "`test:gone`, which isn't a fluid",
        ),
    ] {
        let block = format!(
            r#"{{ model: "quad", textures: {{ all: "a" }}, properties: {{ mode: ["off", "idle"] }}, machine: {machine} }}"#
        );
        let content = test_mod(&[
            POWER,
            fluid_net,
            items_net,
            water,
            (
                "blocks/a_block.json5",
                r#"{ model: "quad", textures: { all: "a" } }"#,
            ),
            ("blocks/thing.json5", &block),
        ]);
        assert!(
            content
                .errors
                .iter()
                .any(|e| e.contains("thing.json5") && e.contains(error)),
            "{machine}: {:?}",
            content.errors
        );
    }
    // `facing` must name sides.
    let content = test_mod(&[
        POWER,
        (
            "blocks/thing.json5",
            r#"{ model: "quad", textures: { all: "a" }, properties: { facing: ["in", "out"] },
                 machine: { ports: [{ faces: ["front"], network: "power", role: "consume" }], energy: { capacity: 1 } } }"#,
        ),
    ]);
    assert!(
        content.errors.iter().any(|e| e.contains("value `in`")),
        "{:?}",
        content.errors
    );
    // A fluid from a file after the machine's is fine.
    let content = test_mod(&[
        (
            "blocks/a_tank.json5",
            r#"{ model: "quad", textures: { all: "a" }, machine: { tanks: { a: { capacity: 1, accepts: ["water"] } } } }"#,
        ),
        water,
    ]);
    assert!(content.errors.is_empty(), "{:?}", content.errors);
}

#[test]
fn roles_say_which_way_things_go() {
    use PortRole::*;
    for (role, gives, takes) in [
        (Produce, true, false),
        (Consume, false, true),
        (Store, true, true),
        (Input, false, true),
        (Output, true, false),
    ] {
        assert_eq!((role.gives(), role.takes()), (gives, takes), "{role:?}");
    }
    assert!(Store.fits(Solver::Energy) && Store.fits(Solver::Fluid) && Store.fits(Solver::Item));
    assert!(!Produce.fits(Solver::Item) && !Input.fits(Solver::Energy));
}
