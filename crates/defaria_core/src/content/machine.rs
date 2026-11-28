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
    schema::{Attribute, FilterDef, MachineDef, MachineRecipesDef, PortFace, PortRole, Solver},
};
use crate::content::ItemState;

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
    /// The item ports' filters, which [`MachinePort::filter`] indexes.
    pub filters: Vec<ItemFilter>,
    /// For a creative machine, the item it keeps a slot group full of.
    pub emits: Option<Emits>,
    /// The recipes it runs, if it's a recipe machine.
    pub recipes: Option<MachineRecipes>,
}

/// What a recipe machine runs, and where it takes inputs from and puts
/// outputs: indices into [`Machine::slots`] and [`Machine::tanks`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MachineRecipes {
    /// The recipe type, such as `smelting`.
    pub kind: String,
    pub input_slots: Vec<usize>,
    pub input_tanks: Vec<usize>,
    pub output_slots: Vec<usize>,
    pub output_tanks: Vec<usize>,
}

/// The items an item port passes: those with one of the ids, or with a
/// material property that's one of the materials or has one of the tags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemFilter {
    /// Item ids.
    pub items: Vec<String>,
    /// Material ids.
    pub materials: Vec<String>,
    /// Material tags, without the `#`.
    pub tags: Vec<String>,
}

impl ItemFilter {
    /// Whether `item` passes.
    pub fn passes(&self, content: &Content, item: ItemState) -> bool {
        let Some(info) = content.item_state(item) else {
            return false;
        };
        let def = &content.items[info.item];
        if self.items.contains(&def.id) {
            return true;
        }
        def.properties
            .iter()
            .zip(&info.values)
            .filter(|(property, _)| property.materials)
            .any(|(_, (_, value))| {
                self.materials.contains(value)
                    || content
                        .materials
                        .get(value)
                        .is_some_and(|m| self.tags.iter().any(|tag| m.has_tag(tag)))
            })
    }
}

/// A creative machine's endless supply: a slot group kept full of one
/// item, out of nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emits {
    /// The item's id; it's emitted in its default state.
    pub item: String,
    /// The slot group, an index into [`Machine::slots`].
    pub slots: usize,
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
    /// For an item port with a filter, an index into [`Machine::filters`].
    pub filter: Option<usize>,
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
    /// The most its script may make in a second.
    pub max_generate: u64,
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
    energy: Option<[Mix; 6]>,
    tanks: Vec<(String, Mix, Vec<String>)>,
    slots: Vec<SlotGroupSpec>,
    /// Property index and value index of each `active_state` value.
    active: Vec<(usize, usize)>,
    /// The `facing` property's index, if a port needs it.
    facing: Option<usize>,
    recipes: Option<MachineRecipes>,
    filters: Vec<ItemFilter>,
    emits: Option<Emits>,
}

struct PortMix {
    faces: Vec<PortFace>,
    network: usize,
    role: PortRole,
    tank: Option<usize>,
    slots: Option<usize>,
    filter: Option<usize>,
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
                    mix("energy.max_generate", "max_generate", &energy.max_generate)?,
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
        let mut filters = Vec::new();
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
            if port.filter.is_some() && solver != Solver::Item {
                return Err(at("only a port on an item network has a `filter`".into()));
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
            let filter = match &port.filter {
                Some(def) => {
                    filters.push(item_filter(def, namespace, content).map_err(at)?);
                    Some(filters.len() - 1)
                }
                None => None,
            };
            ports.push(PortMix {
                faces: port.faces.clone(),
                network,
                role: port.role,
                tank,
                slots: group,
                filter,
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

        let recipes = def
            .recipes
            .as_ref()
            .map(|recipes| machine_recipes(recipes, def, &tanks, &slots))
            .transpose()?;

        let emits = def
            .emits
            .as_ref()
            .map(|emits| {
                let names: Vec<&str> = slots.iter().map(|group| group.name.as_str()).collect();
                Ok::<_, String>(Emits {
                    item: qualify(&emits.item, namespace)
                        .map_err(|error| format!("machine `emits`: {error}"))?,
                    slots: pick("slot group", "slots", emits.slots.as_deref(), &names)
                        .map_err(|error| format!("machine `emits` {error}"))?,
                })
            })
            .transpose()?;

        Ok(MachineMix {
            ports,
            energy,
            tanks,
            slots,
            active,
            facing,
            recipes,
            filters,
            emits,
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
                    filter: port.filter,
                });
            }
        }
        let round = |value: f32| value.round() as u64;
        let energy = self.energy.as_ref().map(
            |[capacity, max_in, max_out, generate, max_generate, uses]| {
                let value = |mix: &Mix| round(mix.value(properties, index, content));
                EnergySpec {
                    capacity: value(capacity),
                    max_in: value(max_in),
                    max_out: value(max_out),
                    generate: value(generate),
                    max_generate: value(max_generate),
                    uses: value(uses),
                }
            },
        );
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
            recipes: self.recipes.clone(),
            filters: self.filters.clone(),
            emits: self.emits.clone(),
        })
    }
}

/// A machine's `recipes`, its slot groups and tanks found by name.
fn machine_recipes(
    def: &MachineRecipesDef,
    machine: &MachineDef,
    tanks: &[(String, Mix, Vec<String>)],
    slots: &[SlotGroupSpec],
) -> Result<MachineRecipes, String> {
    check_namespace(&def.kind).map_err(|error| format!("machine recipe type {error}"))?;
    let uses = machine.energy.as_ref().map(|energy| &energy.uses);
    if uses.is_some_and(|uses| !matches!(uses, Attribute::Fixed(n) if *n == 0.0)) {
        return Err(
            "a recipe machine draws its recipes' energy itself; leave out `energy.use`".into(),
        );
    }
    let find = |names: &Option<Vec<String>>, default: &str| {
        let mut found_slots = Vec::new();
        let mut found_tanks = Vec::new();
        let (names, explicit) = match names {
            Some(names) => (names.clone(), true),
            None => (vec![default.to_string()], false),
        };
        for (i, name) in names.iter().enumerate() {
            if names[..i].contains(name) {
                return Err(format!("machine `recipes` names `{name}` twice"));
            }
            let group = slots.iter().position(|group| &group.name == name);
            let tank = tanks.iter().position(|(tank, ..)| tank == name);
            if explicit && group.is_none() && tank.is_none() {
                return Err(format!(
                    "machine `recipes` names `{name}`, which isn't one of its slot groups or tanks"
                ));
            }
            found_slots.extend(group);
            found_tanks.extend(tank);
        }
        Ok((found_slots, found_tanks))
    };
    let (input_slots, input_tanks) = find(&def.inputs, "input")?;
    let (output_slots, output_tanks) = find(&def.outputs, "output")?;
    Ok(MachineRecipes {
        kind: def.kind.clone(),
        input_slots,
        input_tanks,
        output_slots,
        output_tanks,
    })
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

/// A port's `filter`, its materials checked.
fn item_filter(def: &FilterDef, namespace: &str, content: &Content) -> Result<ItemFilter, String> {
    if def.items.is_empty() && def.materials.is_empty() {
        return Err(
            "`filter` is empty, so nothing would pass; give it `items` or `materials`".into(),
        );
    }
    let items = def
        .items
        .iter()
        .map(|item| qualify(item, namespace))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("`filter.items`: {error}"))?;
    let mut materials = Vec::new();
    let mut tags = Vec::new();
    for entry in &def.materials {
        if let Some(tag) = entry.strip_prefix('#') {
            if !content.materials.values().any(|m| m.has_tag(tag)) {
                return Err(format!("`filter.materials`: no material has tag `{tag}`"));
            }
            tags.push(tag.to_string());
        } else {
            let id = qualify(entry, namespace)
                .map_err(|error| format!("`filter.materials`: {error}"))?;
            if !content.materials.contains_key(&id) {
                return Err(format!("`filter.materials`: no material `{id}`"));
            }
            materials.push(id);
        }
    }
    Ok(ItemFilter {
        items,
        materials,
        tags,
    })
}

/// Checks that the items a machine's filters and `emits` name are items.
/// Run once every item has loaded, since items load after blocks.
pub(super) fn check_machine_items(
    def: &MachineDef,
    namespace: &str,
    content: &Content,
) -> Result<(), String> {
    let named = def
        .ports
        .iter()
        .filter_map(|port| port.filter.as_ref())
        .flat_map(|filter| filter.items.iter().map(|item| ("filter.items", item)))
        .chain(def.emits.iter().map(|emits| ("emits.item", &emits.item)));
    for (field, item) in named {
        let Ok(id) = qualify(item, namespace) else {
            continue;
        };
        if content.item(&id).is_none() {
            return Err(format!(
                "machine `{field}` names `{id}`, which isn't an item"
            ));
        }
    }
    Ok(())
}

fn solver_name(solver: Solver) -> &'static str {
    match solver {
        Solver::Energy => "energy",
        Solver::Fluid => "fluid",
        Solver::Item => "item",
    }
}

fn direction(name: &str) -> Option<Direction> {
    Direction::from_name(name)
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
