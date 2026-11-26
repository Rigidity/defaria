//! Machines as content: a block's `machine` field, baked per state. Each
//! state gets its ports (on which faces, which network kind, what role),
//! with the faces turned by the state's `rotate` and the relative ones
//! (`front`, ...) resolved from its `facing` property, and the sizes of
//! its energy buffer, tanks and slot groups, which can come from its
//! materials. The `machine` plugin gives machines' block entities the
//! components that hold what's in them. See "Machines" in
//! `docs/modding.md`.

use super::{
    BlockState, Content, Direction, FULL_FLUID, Property,
    bake::{Mix, rotation},
    check_namespace, offset_of, qualify,
    schema::{MachineDef, PortFace, PortRole, Solver},
};

/// The most slots a group may have.
pub const MAX_SLOTS: u32 = 256;

/// A machine block state, baked: what its block entity holds and where
/// conduits join it.
#[derive(Debug, Clone, PartialEq)]
pub struct Machine {
    /// At most one per face and network kind.
    pub ports: Vec<MachinePort>,
    pub energy: Option<EnergySpec>,
    /// In order of name.
    pub tanks: Vec<TankSpec>,
    /// In order of name.
    pub slots: Vec<SlotGroupSpec>,
    /// This state with the block's `active_state` values: the state it's
    /// in while it works. Itself for a block without `active_state`.
    pub active: BlockState,
    /// This state with those properties back at their defaults: the state
    /// it's in when it stops.
    pub idle: BlockState,
}

/// A face of a machine that conduits of one network kind join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MachinePort {
    /// The side, in the world: turned and resolved for this state.
    pub face: Direction,
    /// The network kind, an index into [`Content::networks`].
    pub network: usize,
    pub role: PortRole,
    /// For a fluid port, its tank, an index into [`Machine::tanks`].
    pub tank: Option<usize>,
    /// For an item port, its slot group, an index into [`Machine::slots`].
    pub slots: Option<usize>,
}

/// A machine's energy buffer, in joules, rates in watts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnergySpec {
    pub capacity: u64,
    /// The most it takes in a second.
    pub max_in: u64,
    /// The most it gives in a second.
    pub max_out: u64,
    /// What it makes on its own in a second.
    pub generate: u64,
    /// What it uses up in a second while it works.
    pub uses: u64,
}

/// One of a machine's tanks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TankSpec {
    pub name: String,
    /// In [`FULL_FLUID`] units: thousandths of a block.
    pub capacity: u64,
    /// The ids of the fluid blocks it takes; empty for any.
    pub accepts: Vec<String>,
}

/// A named group of a machine's item slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotGroupSpec {
    pub name: String,
    pub count: u32,
}

impl Machine {
    /// Its port on network kind `kind` on its `face`, if it has one.
    pub fn port(&self, kind: usize, face: Direction) -> Option<&MachinePort> {
        self.ports
            .iter()
            .find(|port| port.network == kind && port.face == face)
    }

    /// The index of its tank called `name`.
    pub fn tank(&self, name: &str) -> Option<usize> {
        self.tanks.iter().position(|tank| tank.name == name)
    }

    /// The index of its slot group called `name`.
    pub fn slot_group(&self, name: &str) -> Option<usize> {
        self.slots.iter().position(|group| group.name == name)
    }

    /// The state it should be in, working or not.
    pub fn state(&self, active: bool) -> BlockState {
        if active { self.active } else { self.idle }
    }
}

impl Content {
    /// What `state` is as a machine, if it's one.
    pub fn machine(&self, state: BlockState) -> Option<&Machine> {
        self.state(state).machine.as_deref()
    }

    /// Whether `state` has a port on network kind `kind` on its `side`, so
    /// conduits of that kind join it there and it's on their network.
    pub fn has_port(&self, state: BlockState, kind: usize, side: Direction) -> bool {
        self.machine(state)
            .is_some_and(|machine| machine.port(kind, side).is_some())
    }
}

/// The side `face` names on a block facing `facing`: `front` is the facing
/// side, and `left` and `right` are the block's own, looking out of its
/// front. A block facing up or down has west on its left, as one facing
/// north does.
pub fn resolve_face(face: PortFace, facing: Option<Direction>) -> Option<Direction> {
    let left_of = |front: Direction| match front {
        Direction::North | Direction::Up | Direction::Down => Direction::West,
        Direction::West => Direction::South,
        Direction::South => Direction::East,
        Direction::East => Direction::North,
    };
    Some(match face {
        PortFace::Up => Direction::Up,
        PortFace::Down => Direction::Down,
        PortFace::North => Direction::North,
        PortFace::South => Direction::South,
        PortFace::East => Direction::East,
        PortFace::West => Direction::West,
        PortFace::Front => facing?,
        PortFace::Back => facing?.opposite(),
        PortFace::Left => left_of(facing?),
        PortFace::Right => left_of(facing?).opposite(),
    })
}

fn relative(face: PortFace) -> bool {
    matches!(
        face,
        PortFace::Front | PortFace::Back | PortFace::Left | PortFace::Right
    )
}

/// The property relative faces are resolved from.
pub const FACING: &str = "facing";

/// A block's `machine`, checked against its properties, to bake per
/// state.
pub(super) struct MachineMix {
    ports: Vec<PortMix>,
    energy: Option<[Mix; 5]>,
    tanks: Vec<(String, Mix, Vec<String>)>,
    slots: Vec<SlotGroupSpec>,
    /// Property index and value index of each `active_state` value.
    active: Vec<(usize, usize)>,
    /// The `facing` property's index, if a port needs it.
    facing: Option<usize>,
}

struct PortMix {
    faces: Vec<PortFace>,
    network: usize,
    role: PortRole,
    tank: Option<usize>,
    slots: Option<usize>,
}

impl MachineMix {
    pub(super) fn new(
        def: &MachineDef,
        namespace: &str,
        properties: &[Property],
        content: &Content,
    ) -> Result<Self, String> {
        let mix = |field: &str, stat: &str, def| {
            Mix::new(
                &format!("machine.{field}"),
                stat,
                def,
                properties,
                None,
                content,
            )
        };
        let energy = def
            .energy
            .as_ref()
            .map(|energy| {
                Ok::<_, String>([
                    mix("energy.capacity", "capacity", &energy.capacity)?,
                    mix("energy.max_in", "max_in", &energy.max_in)?,
                    mix("energy.max_out", "max_out", &energy.max_out)?,
                    mix("energy.generate", "generate", &energy.generate)?,
                    mix("energy.use", "use", &energy.uses)?,
                ])
            })
            .transpose()?;
        let mut tanks = Vec::new();
        for (name, tank) in &def.tanks {
            check_namespace(name).map_err(|error| format!("machine tank name {error}"))?;
            let capacity = mix(
                &format!("tanks.{name}.capacity"),
                "capacity",
                &tank.capacity,
            )?;
            let accepts = tank
                .accepts
                .iter()
                .map(|fluid| qualify(fluid, namespace))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("machine tank `{name}` accepts: {error}"))?;
            tanks.push((name.clone(), capacity, accepts));
        }
        let mut slots = Vec::new();
        for (name, &count) in &def.slots {
            check_namespace(name).map_err(|error| format!("machine slot group name {error}"))?;
            if count == 0 || count > MAX_SLOTS {
                return Err(format!(
                    "machine slot group `{name}` has {count} slots; give it 1 to {MAX_SLOTS}"
                ));
            }
            slots.push(SlotGroupSpec {
                name: name.clone(),
                count,
            });
        }

        let mut ports = Vec::new();
        for (i, port) in def.ports.iter().enumerate() {
            let at = |error: String| format!("machine port {}: {error}", i + 1);
            if port.faces.is_empty() {
                return Err(at("`faces` is empty".into()));
            }
            let id = qualify(&port.network, namespace).map_err(at)?;
            let network = content
                .network(&id)
                .ok_or_else(|| at(format!("no network kind `{id}`")))?;
            let solver = content.networks[network].solver;
            if !port.role.fits(solver) {
                let roles = match solver {
                    Solver::Energy => "`produce`, `consume` or `store`",
                    Solver::Fluid | Solver::Item => "`input`, `output` or `store`",
                };
                return Err(at(format!(
                    "`{id}` is a {} network, whose ports {roles}, not `{}`",
                    solver_name(solver),
                    port.role.name()
                )));
            }
            if solver == Solver::Energy && energy.is_none() {
                return Err(at(
                    "a port on an energy network needs the machine's `energy`".into(),
                ));
            }
            if port.tank.is_some() && solver != Solver::Fluid {
                return Err(at("only a port on a fluid network has a `tank`".into()));
            }
            if port.slots.is_some() && solver != Solver::Item {
                return Err(at("only a port on an item network has `slots`".into()));
            }
            let names: Vec<&str> = tanks.iter().map(|(name, ..)| name.as_str()).collect();
            let tank = (solver == Solver::Fluid)
                .then(|| pick("tank", "tanks", port.tank.as_deref(), &names))
                .transpose()
                .map_err(at)?;
            let names: Vec<&str> = slots.iter().map(|group| group.name.as_str()).collect();
            let group = (solver == Solver::Item)
                .then(|| pick("slot group", "slots", port.slots.as_deref(), &names))
                .transpose()
                .map_err(at)?;
            ports.push(PortMix {
                faces: port.faces.clone(),
                network,
                role: port.role,
                tank,
                slots: group,
            });
        }

        let facing = if def
            .ports
            .iter()
            .any(|port| port.faces.iter().any(|&f| relative(f)))
        {
            let p = properties
                .iter()
                .position(|property| property.name == FACING)
                .ok_or_else(|| {
                    "machine ports with `front`, `back`, `left` or `right` need a `facing` \
                     property"
                        .to_string()
                })?;
            if let Some(value) = properties[p]
                .values
                .iter()
                .find(|value| direction(value).is_none())
            {
                return Err(format!(
                    "`facing` has value `{value}`, which isn't a side (`up`, `north`, ...)"
                ));
            }
            Some(p)
        } else {
            None
        };

        let mut active = Vec::new();
        for (name, value) in &def.active_state {
            let p = properties
                .iter()
                .position(|property| &property.name == name)
                .ok_or_else(|| {
                    format!("machine `active_state` sets `{name}`, which isn't a property")
                })?;
            let v = properties[p]
                .values
                .iter()
                .position(|v| v == value)
                .ok_or_else(|| {
                    format!("machine `active_state` sets `{name}` to `{value}`, which isn't one of its values")
                })?;
            // The idle state has every `active_state` property at its
            // default, its first value.
            if v == 0 {
                return Err(format!(
                    "machine `active_state` sets `{name}` to `{value}`, its default, so it would \
                     look the same idle"
                ));
            }
            active.push((p, v));
        }

        Ok(MachineMix {
            ports,
            energy,
            tanks,
            slots,
            active,
            facing,
        })
    }

    /// The machine of the state with these value indices, turned by
    /// `rotate`, among the block's states from `first_state`.
    pub(super) fn value(
        &self,
        properties: &[Property],
        index: &[usize],
        rotate: Option<[i32; 3]>,
        first_state: u16,
        content: &Content,
    ) -> Result<Machine, String> {
        let rotation = rotation(rotate.unwrap_or([0; 3]))?;
        let turn = |side: Direction| {
            Direction::from_normal((rotation * side.normal().as_vec3()).round().as_ivec3())
                .expect("a quarter turn keeps a side a side")
        };
        let facing = self
            .facing
            .map(|p| direction(&properties[p].values[index[p]]).expect("checked in new"));
        let mut ports: Vec<MachinePort> = Vec::new();
        for port in &self.ports {
            for &face in &port.faces {
                let side = if relative(face) {
                    resolve_face(face, facing).expect("facing checked in new")
                } else {
                    turn(resolve_face(face, None).expect("an absolute side"))
                };
                if ports
                    .iter()
                    .any(|other| other.network == port.network && other.face == side)
                {
                    return Err(format!(
                        "machine has two ports on `{}` on its {} face",
                        content.networks[port.network].id,
                        side.name()
                    ));
                }
                ports.push(MachinePort {
                    face: side,
                    network: port.network,
                    role: port.role,
                    tank: port.tank,
                    slots: port.slots,
                });
            }
        }
        let round = |value: f32| value.round() as u64;
        let energy = self
            .energy
            .as_ref()
            .map(|[capacity, max_in, max_out, generate, uses]| {
                let value = |mix: &Mix| round(mix.value(properties, index, content));
                EnergySpec {
                    capacity: value(capacity),
                    max_in: value(max_in),
                    max_out: value(max_out),
                    generate: value(generate),
                    uses: value(uses),
                }
            });
        let tanks = self
            .tanks
            .iter()
            .map(|(name, capacity, accepts)| TankSpec {
                name: name.clone(),
                capacity: round(capacity.value(properties, index, content) * FULL_FLUID as f32),
                accepts: accepts.clone(),
            })
            .collect();
        let state = |index: &[usize]| BlockState(first_state + offset_of(properties, index) as u16);
        let mut active = index.to_vec();
        let mut idle = index.to_vec();
        for &(p, v) in &self.active {
            active[p] = v;
            // A property's default is its first value.
            idle[p] = 0;
        }
        Ok(Machine {
            ports,
            energy,
            tanks,
            slots: self.slots.clone(),
            active: state(&active),
            idle: state(&idle),
        })
    }
}

/// The index of the tank or slot group a port names in `names`, or the
/// only one.
fn pick(what: &str, field: &str, named: Option<&str>, names: &[&str]) -> Result<usize, String> {
    match named {
        Some(name) => names
            .iter()
            .position(|n| *n == name)
            .ok_or_else(|| format!("names {what} `{name}`, which the machine doesn't have")),
        None if names.len() == 1 => Ok(0),
        None if names.is_empty() => Err(format!(
            "the machine has no {field} for it to use; give it `{field}`"
        )),
        None => Err(format!(
            "the machine has {} {field}; say which with `{}`",
            names.len(),
            if field == "tanks" { "tank" } else { "slots" }
        )),
    }
}

fn solver_name(solver: Solver) -> &'static str {
    match solver {
        Solver::Energy => "energy",
        Solver::Fluid => "fluid",
        Solver::Item => "item",
    }
}

fn direction(name: &str) -> Option<Direction> {
    Direction::ALL.into_iter().find(|side| side.name() == name)
}

/// Checks that the fluids a machine's tanks accept are blocks with
/// `fluid`. Run once every block has loaded, since a tank can name a
/// fluid from a file that loads after it.
pub(super) fn check_tank_fluids(
    def: &MachineDef,
    namespace: &str,
    content: &Content,
) -> Result<(), String> {
    for (name, tank) in &def.tanks {
        for fluid in &tank.accepts {
            let Ok(id) = qualify(fluid, namespace) else {
                continue;
            };
            let is_fluid = content
                .block(&id)
                .is_some_and(|block| content.state(block.default_state()).is_fluid());
            if !is_fluid {
                return Err(format!(
                    "machine tank `{name}` accepts `{id}`, which isn't a fluid"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
