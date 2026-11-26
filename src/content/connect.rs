//! Connection properties: the six booleans a block's `connections`
//! property becomes, one per side, which the engine sets from the blocks
//! around it, so a pipe joins the pipes next to it. See "Connections" in
//! `docs/modding.md`; `crate::connections` keeps them up to date in the
//! world.

use super::{BlockState, Content, Direction, Property, index_of, offset_of};

/// What a block's connection properties join by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// A rule's name, as `mod:name`: blocks with the same rule join.
    Named(String),
    /// Its state's [`Conduit`](super::Conduit): conduits on the same
    /// network kind and group join, and so do ports of that kind.
    Conduit,
}

/// What a block joining its neighbour offers it, for
/// [`Content::accepts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Joins<'a> {
    /// Blocks connecting by a named rule.
    Rule(&'a str),
    /// A conduit on network kind `kind` (an index into
    /// [`Content::networks`]), in a group or none.
    Network { kind: usize, group: Option<&'a str> },
}

/// The values of each side's property: not joined, then joined.
pub const SIDE_VALUES: [&str; 2] = ["false", "true"];

/// A block's connection properties.
#[derive(Debug, Clone, PartialEq)]
pub struct Connections {
    /// What it connects by.
    pub rule: Rule,
    /// The property for each side, in [`Direction::ALL`] order, as an index
    /// into the block's properties. Value 0 is `false`, 1 is `true`.
    pub sides: [usize; 6],
}

impl Connections {
    /// The connections of a block with these properties (sorted by name, as
    /// a block's are), which include the six sides' (see
    /// [`side_properties`]). `None` if one is missing.
    pub(super) fn new(rule: Rule, properties: &[Property]) -> Option<Self> {
        let mut sides = [0; 6];
        for side in Direction::ALL {
            sides[side as usize] = properties.iter().position(|p| p.name == side.name())?;
        }
        Some(Self { rule, sides })
    }
}

/// The six properties a `connections` property becomes.
pub(super) fn side_properties() -> impl Iterator<Item = Property> {
    Direction::ALL.into_iter().map(|side| Property {
        name: side.name().into(),
        values: SIDE_VALUES.map(String::from).to_vec(),
        materials: false,
        item: false,
        place: None,
    })
}

impl Content {
    /// What `state` offers the blocks next to it: its named rule, or its
    /// conduit's network kind and group. `None` for a block that doesn't
    /// connect.
    pub fn joins(&self, state: BlockState) -> Option<Joins<'_>> {
        match &self.block_of(state).connections.as_ref()?.rule {
            Rule::Named(rule) => Some(Joins::Rule(rule)),
            Rule::Conduit => self.conduit(state).map(|conduit| Joins::Network {
                kind: conduit.network,
                group: conduit.group.as_deref(),
            }),
        }
    }

    /// Whether `state` takes a connection from a block offering `joins`
    /// on its `side`:
    ///
    /// - by a named rule, a block that connects by the same rule, on any
    ///   side;
    /// - by a network, a conduit on the same network kind and in the same
    ///   group (or both in none), on any side, or a block with a port of
    ///   that kind on that side ([`Content::has_port`]).
    ///
    /// This is the one place that decides what joins what, so the
    /// connection properties, the networks and everything else agree.
    pub fn accepts(&self, state: BlockState, joins: Joins, side: Direction) -> bool {
        match joins {
            Joins::Rule(rule) => self.block_of(state).connections.as_ref().is_some_and(
                |connections| matches!(&connections.rule, Rule::Named(own) if own == rule),
            ),
            Joins::Network { kind, group } => {
                let conduit = self.conduit(state).is_some_and(|conduit| {
                    conduit.network == kind && conduit.group.as_deref() == group
                });
                conduit || self.has_port(state, kind, side)
            }
        }
    }

    /// Whether the engine sets some of `state`'s properties from its
    /// neighbours.
    pub fn connects(&self, state: BlockState) -> bool {
        self.block_of(state).connections.is_some()
    }

    /// The state `state` should be in with these neighbours: each side
    /// joined if the block `neighbour(side)` gives there accepts its rule
    /// on the side facing it. A side whose neighbour is `None` (not loaded)
    /// keeps its value. A block without connections stays as it is.
    pub fn connected(
        &self,
        state: BlockState,
        neighbour: impl FnMut(Direction) -> Option<BlockState>,
    ) -> BlockState {
        self.connected_unless(state, |_| false, neighbour)
    }

    /// As [`Content::connected`], but a side that's `blocked` (a conduit's
    /// face closed with a wrench, on either side) is never joined, whatever
    /// is there, loaded or not.
    pub fn connected_unless(
        &self,
        state: BlockState,
        blocked: impl Fn(Direction) -> bool,
        mut neighbour: impl FnMut(Direction) -> Option<BlockState>,
    ) -> BlockState {
        let block = self.block_of(state);
        let Some(connections) = &block.connections else {
            return state;
        };
        let Some(joins) = self.joins(state) else {
            return state;
        };
        let offset = (state.0 - block.first_state) as usize;
        let mut index = index_of(&block.properties, offset);
        for side in Direction::ALL {
            let joined = if blocked(side) {
                false
            } else if let Some(next) = neighbour(side) {
                self.accepts(next, joins, side.opposite())
            } else {
                continue;
            };
            index[connections.sides[side as usize]] = joined as usize;
        }
        BlockState(block.first_state + offset_of(&block.properties, &index) as u16)
    }

    /// Whether `state` is joined on its `side`. False for a block without
    /// connections.
    pub fn joined(&self, state: BlockState, side: Direction) -> bool {
        let block = self.block_of(state);
        block.connections.as_ref().is_some_and(|connections| {
            let offset = (state.0 - block.first_state) as usize;
            index_of(&block.properties, offset)[connections.sides[side as usize]] == 1
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{Solver, base, block, test_mod};

    fn pipe(content: &Content, joined: &[Direction]) -> BlockState {
        let values: Vec<(&str, &str)> = joined.iter().map(|side| (side.name(), "true")).collect();
        content.block_state("defaria:pipe", &values).unwrap()
    }

    #[test]
    fn a_connections_property_is_six_sides() {
        let content = base();
        let block = content.block("defaria:pipe").unwrap();
        let connections = block.connections.as_ref().unwrap();
        assert_eq!(connections.rule, Rule::Conduit);
        for side in Direction::ALL {
            let property = &block.properties[connections.sides[side as usize]];
            assert_eq!(property.name, side.name());
            assert_eq!(property.values, ["false", "true"]);
            assert!(!property.item);
        }
        // Every metal times every combination of sides.
        let metals = block.properties.iter().find(|p| p.name == "metal").unwrap();
        assert_eq!(block.state_count as usize, metals.values.len() * 64);
        // Not joined anywhere by default, and the item doesn't carry it.
        for side in Direction::ALL {
            assert!(!content.joined(block.default_state(), side));
        }
        assert!(content.connects(block.default_state()));
        assert!(!content.connects(crate::content::block("stone")));
    }

    #[test]
    fn sides_join_neighbours_that_accept_the_rule() {
        let content = base();
        let lone = pipe(content, &[]);
        let wire = content.default_state("defaria:wire").unwrap();
        let stone = block("stone");
        let around = |east, west, up| {
            move |side| match side {
                Direction::East => Some(east),
                Direction::West => Some(west),
                Direction::Up => up,
                _ => Some(BlockState::AIR),
            }
        };
        // A pipe to the east joins; stone and a wire don't.
        let state = content.connected(lone, around(lone, stone, Some(wire)));
        assert_eq!(state, pipe(content, &[Direction::East]));
        // Pipes of other metals join too.
        let copper = content
            .block_state("defaria:pipe", &[("metal", "defaria:copper")])
            .unwrap();
        let state = content.connected(copper, around(lone, lone, Some(lone)));
        assert_eq!(
            state,
            content
                .block_state(
                    "defaria:pipe",
                    &[
                        ("metal", "defaria:copper"),
                        ("east", "true"),
                        ("west", "true"),
                        ("up", "true"),
                    ]
                )
                .unwrap()
        );
        // A side whose neighbour isn't loaded keeps its value, whatever it
        // is; the others still update.
        let up = pipe(content, &[Direction::Up, Direction::East]);
        let state = content.connected(up, around(stone, stone, None));
        assert_eq!(state, pipe(content, &[Direction::Up]));
        // Blocks without connections are left alone.
        assert_eq!(content.connected(stone, around(lone, lone, None)), stone);
        // Wires join wires, not pipes.
        let state = content.connected(wire, around(wire, lone, Some(wire)));
        assert_eq!(
            state,
            content
                .block_state("defaria:wire", &[("east", "true"), ("up", "true")])
                .unwrap()
        );
    }

    /// A blocked side is never joined: not to a pipe that would join it,
    /// and not when its neighbour isn't loaded, which otherwise keeps it.
    #[test]
    fn blocked_sides_never_join() {
        let content = base();
        let lone = pipe(content, &[]);
        let joined = pipe(content, &[Direction::East, Direction::Up]);
        let neighbour = |side| (side != Direction::Up).then_some(lone);
        let state = content.connected_unless(
            joined,
            |side| matches!(side, Direction::East | Direction::Up),
            neighbour,
        );
        assert_eq!(
            state,
            pipe(
                content,
                &[
                    Direction::North,
                    Direction::South,
                    Direction::West,
                    Direction::Down
                ]
            )
        );
    }

    #[test]
    fn rules_are_named_per_mod() {
        let content = test_mod(&[
            ("models/m.json5", "{ faces: [] }"),
            (
                "blocks/a.json5",
                r#"{ model: "m", properties: { connect: { connections: "duct" }, color: ["red", "blue"] } }"#,
            ),
            (
                "blocks/b.json5",
                r#"{ model: "m", properties: { connect: { connections: "test:duct" } } }"#,
            ),
            (
                "blocks/c.json5",
                r#"{ model: "m", properties: { connect: { connections: "other:duct" } } }"#,
            ),
        ]);
        assert!(content.errors.is_empty(), "{:?}", content.errors);
        let a = content.default_state("test:a").unwrap();
        let b = content.default_state("test:b").unwrap();
        let c = content.default_state("test:c").unwrap();
        assert!(content.accepts(b, Joins::Rule("test:duct"), Direction::Up));
        assert!(content.accepts(a, Joins::Rule("test:duct"), Direction::Up));
        assert!(!content.accepts(c, Joins::Rule("test:duct"), Direction::Up));
        // The other properties sort in among the sides, and keep working.
        let block = content.block("test:a").unwrap();
        let names: Vec<_> = block.properties.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["color", "down", "east", "north", "south", "up", "west"]
        );
        let blue = block.state(&[("color", "blue")]).unwrap();
        let joined = content.connected(blue, |_| Some(b));
        assert_eq!(
            content.state(joined).values[0],
            ("color".into(), "blue".into())
        );
        assert!(
            Direction::ALL
                .iter()
                .all(|&side| content.joined(joined, side))
        );
    }

    #[test]
    fn connection_mistakes_are_reported() {
        for (file, error) in [
            (
                r#"{ model: "m", properties: { connect: { connections: "duct", values: ["a"] } } }"#,
                "goes alone",
            ),
            (
                r#"{ model: "m", properties: { connect: { connections: "duct", default: "true" } } }"#,
                "goes alone",
            ),
            (
                r#"{ model: "m", properties: { connect: { connections: "duct" }, up: ["a", "b"] } }"#,
                "`up`",
            ),
            (
                r#"{ model: "m", properties: { a: { connections: "duct" }, b: { connections: "pipe" } } }"#,
                "`down`",
            ),
            (
                r#"{ model: "m", properties: { connect: { connections: "Bad Name" } } }"#,
                "connect",
            ),
        ] {
            let content = test_mod(&[
                ("models/m.json5", "{ faces: [] }"),
                ("blocks/thing.json5", file),
            ]);
            assert!(
                content.errors.iter().any(|e| e.contains(error)),
                "{file}: {:?}",
                content.errors
            );
        }
        let content = test_mod(&[(
            "items/thing.json5",
            r#"{ properties: { connect: { connections: "duct" } } }"#,
        )]);
        assert!(
            content.errors.iter().any(|e| e.contains("only for block")),
            "{:?}",
            content.errors
        );
    }

    #[test]
    fn base_conduits_carry_their_metals_stats() {
        let content = base();
        let kind = |id: &str| content.network(id).unwrap();
        let ids: Vec<&str> = content.networks.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["defaria:electric", "defaria:fluid", "defaria:item"]);
        assert_eq!(
            content.networks[kind("defaria:fluid")].solver,
            Solver::Fluid
        );
        let conduit = |id: &str, values: &[(&str, &str)]| {
            content
                .conduit(content.block_state(id, values).unwrap())
                .unwrap()
                .clone()
        };
        let iron = conduit("defaria:pipe", &[]);
        assert_eq!(iron.network, kind("defaria:fluid"));
        assert_eq!(iron.group, None);
        assert_eq!((iron.capacity, iron.throughput), (0.5, 1.0));
        let gold = conduit("defaria:pipe", &[("metal", "defaria:gold"), ("up", "true")]);
        assert_eq!((gold.capacity, gold.throughput), (0.5, 1.5));
        let copper = conduit("defaria:wire", &[]);
        assert_eq!(copper.network, kind("defaria:electric"));
        assert_eq!((copper.capacity, copper.throughput), (0.0, 1000.0));
        let iron_wire = conduit("defaria:wire", &[("metal", "defaria:iron")]);
        assert!((iron_wire.throughput - 170.0).abs() < 1e-3);
        let tube = conduit("defaria:tube", &[]);
        assert_eq!(tube.network, kind("defaria:item"));
        assert_eq!((tube.capacity, tube.throughput), (1.0, 4.0));
        assert_eq!(content.conduit(block("stone")), None);
        // Each connects by its conduit.
        let pipe = content.default_state("defaria:pipe").unwrap();
        assert_eq!(
            content.joins(pipe),
            Some(Joins::Network {
                kind: kind("defaria:fluid"),
                group: None
            })
        );
        assert!(content.accepts(pipe, content.joins(pipe).unwrap(), Direction::Up));
        let tube = content.default_state("defaria:tube").unwrap();
        assert!(!content.accepts(tube, content.joins(pipe).unwrap(), Direction::Up));
    }

    #[test]
    fn groups_are_filled_per_state_and_kept_apart() {
        let content = test_mod(&[
            ("models/m.json5", "{ faces: [] }"),
            (
                "networks/power.json5",
                r#"{ name: "Power", solver: "energy" }"#,
            ),
            (
                "blocks/cable.json5",
                r#"{ model: "m", properties: { color: ["red", "blue"], connect: { connections: true } },
                     conduit: { network: "power", group: "{color}_cable", capacity: 0, throughput: 5 } }"#,
            ),
            (
                "blocks/bare.json5",
                r#"{ model: "m", properties: { connect: { connections: true } },
                     conduit: { network: "test:power", capacity: 1, throughput: 5 } }"#,
            ),
        ]);
        assert!(content.errors.is_empty(), "{:?}", content.errors);
        let red = content
            .block_state("test:cable", &[("color", "red")])
            .unwrap();
        let blue = content
            .block_state("test:cable", &[("color", "blue")])
            .unwrap();
        let bare = content.default_state("test:bare").unwrap();
        assert_eq!(
            content.conduit(red).unwrap().group.as_deref(),
            Some("red_cable")
        );
        let joins = |state| content.joins(state).unwrap();
        assert!(content.accepts(red, joins(red), Direction::North));
        assert!(!content.accepts(blue, joins(red), Direction::North));
        assert!(!content.accepts(bare, joins(red), Direction::North));
        assert!(!content.accepts(red, joins(bare), Direction::North));
        assert!(content.accepts(bare, joins(bare), Direction::North));
        // The sides follow: a red cable joins red, not blue.
        let state = content.connected(red, |side| {
            Some(if side == Direction::East { red } else { blue })
        });
        assert!(content.joined(state, Direction::East));
        assert!(!content.joined(state, Direction::West));
    }

    #[test]
    fn conduit_mistakes_are_reported() {
        let network = (
            "networks/power.json5",
            r#"{ name: "Power", solver: "energy" }"#,
        );
        for (file, error) in [
            (
                r#"{ model: "m", properties: { c: { connections: true } },
                     conduit: { network: "steam", capacity: 1, throughput: 1 } }"#,
                "no network kind `test:steam`",
            ),
            (
                r#"{ model: "m", conduit: { network: "power", capacity: 1, throughput: 1 } }"#,
                "needs a property with `connections: true`",
            ),
            (
                r#"{ model: "m", properties: { c: { connections: true } } }"#,
                "has none",
            ),
            (
                r#"{ model: "m", properties: { c: { connections: "wire" } },
                     conduit: { network: "power", capacity: 1, throughput: 1 } }"#,
                "write `connections: true`",
            ),
            (
                r#"{ model: "m", properties: { c: { connections: false } } }"#,
                "a rule's name, or `true`",
            ),
            (
                r#"{ model: "m", properties: { c: { connections: true } },
                     conduit: { network: "power", capacity: 1 } }"#,
                "throughput",
            ),
            (
                r#"{ model: "m", properties: { c: { connections: true } },
                     conduit: { network: "power", group: "{colour}", capacity: 1, throughput: 1 } }"#,
                "{colour}",
            ),
        ] {
            let content = test_mod(&[
                ("models/m.json5", "{ faces: [] }"),
                network,
                ("blocks/thing.json5", file),
            ]);
            assert!(
                content.errors.iter().any(|e| e.contains(error)),
                "{file}: {:?}",
                content.errors
            );
        }
        let content = test_mod(&[(
            "networks/steam.json5",
            r#"{ name: "Steam", solver: "gas" }"#,
        )]);
        assert!(
            content
                .errors
                .iter()
                .any(|e| e.contains("networks/steam.json5")),
            "{:?}",
            content.errors
        );
    }
}
