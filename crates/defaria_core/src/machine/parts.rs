//! The components a machine's block entity has: [`Ports`], an
//! [`EnergyBuffer`], [`Tanks`] and [`Slots`], each from its block's
//! `machine`, and [`MachineActive`]. All but the ports and the active flag
//! (which are worked out from the voxel's state) are saved with the block
//! entity.

use std::collections::BTreeMap;

use bevy::prelude::*;

use crate::{
    block_entity::{BlockEntityPart, Value},
    content::{
        BlockState, Content, Direction, EnergySpec, Machine, PortRole, SlotGroupSpec, TankSpec,
    },
    fluid_container::saved_data,
    inventory::{
        ItemStack,
        model::{ItemData, is_unique},
    },
    network::NetworkId,
};

/// The most of one item a slot holds. A unique item (see
/// `docs/modding.md`) takes a slot of its own, one to a slot.
pub const SLOT_SIZE: u32 = 64;

/// A machine's ports: its faces that conduits join, and the network each
/// is on now. Worked out from the voxel's state, so not saved; kept up to
/// date as the state and the networks change.
#[derive(Component, Reflect, Debug, Clone, PartialEq, Default)]
#[reflect(Component)]
pub struct Ports(pub Vec<PortLink>);

/// One of a machine's ports.
#[derive(Reflect, Debug, Clone, PartialEq)]
pub struct PortLink {
    /// The side it's on, in the world.
    pub face: Direction,
    /// The network kind's id, such as `defaria:electric`.
    pub kind: String,
    pub role: PortRole,
    /// For a fluid port, its tank: an index into [`Tanks`].
    pub tank: Option<usize>,
    /// For an item port, its slot group: an index into [`Slots`].
    pub slots: Option<usize>,
    /// The network of the conduit joined to it, if one is.
    pub network: Option<NetworkId>,
}

impl Ports {
    fn of(machine: &Machine, content: &Content) -> Self {
        Self(
            machine
                .ports
                .iter()
                .map(|port| PortLink {
                    face: port.face,
                    kind: content.networks[port.network].id.clone(),
                    role: port.role,
                    tank: port.tank,
                    slots: port.slots,
                    network: None,
                })
                .collect(),
        )
    }

    /// Takes the faces and roles of `machine`, keeping each port's network
    /// where its face and kind are the same.
    pub(super) fn follow(&mut self, machine: &Machine, content: &Content) {
        let mut new = Self::of(machine, content);
        for link in &mut new.0 {
            link.network = self
                .0
                .iter()
                .find(|old| old.face == link.face && old.kind == link.kind)
                .and_then(|old| old.network);
        }
        *self = new;
    }

    /// The port on `face` of network kind `kind`, if there's one.
    pub fn get(&self, face: Direction, kind: &str) -> Option<&PortLink> {
        self.0
            .iter()
            .find(|link| link.face == face && link.kind == kind)
    }
}

impl BlockEntityPart for Ports {
    const KEY: &'static str = "ports";
    const SAVED: bool = false;

    fn new(state: BlockState, content: &Content) -> Option<Self> {
        let machine = content.machine(state)?;
        (!machine.ports.is_empty()).then(|| Self::of(machine, content))
    }

    fn save(&self, _: &Content) -> Value {
        Value::Table(BTreeMap::new())
    }

    fn load(_: &Value, _: u32, state: BlockState, content: &Content) -> Option<Self> {
        Self::new(state, content)
    }
}

/// Whether a machine is working. Its solver or script sets it, and the
/// engine puts the voxel in the block's `active_state` while it's true,
/// and back to the defaults when it's false (see
/// [`Machine::active`](crate::content::Machine)). Not saved: it's read
/// from the voxel's state when the block entity is made.
#[derive(
    Component,
    Reflect,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
#[reflect(Component)]
pub struct MachineActive(pub bool);

impl BlockEntityPart for MachineActive {
    const KEY: &'static str = "active";
    const SAVED: bool = false;

    fn new(state: BlockState, content: &Content) -> Option<Self> {
        let machine = content.machine(state)?;
        Some(Self(
            machine.active != machine.idle && state == machine.active,
        ))
    }

    fn save(&self, _: &Content) -> Value {
        Value::Bool(self.0)
    }

    fn load(_: &Value, _: u32, state: BlockState, content: &Content) -> Option<Self> {
        Self::new(state, content)
    }
}

/// The energy a machine holds, in joules, and how fast (in watts) it may take
/// and give it, make it and use it. Only `stored` is saved; the rest comes
/// from the block.
#[derive(
    Component,
    Reflect,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
#[reflect(Component)]
pub struct EnergyBuffer {
    pub stored: u64,
    pub capacity: u64,
    /// The most it takes in a second.
    pub max_in: u64,
    /// The most it gives in a second.
    pub max_out: u64,
    /// What it makes on its own in a second (a creative generator).
    pub generate: u64,
    /// The most its script may make in a second (see
    /// `docs/scripting.md`).
    pub max_generate: u64,
    /// What it uses up in a second while it works (a powered lamp).
    pub uses: u64,
}

impl EnergyBuffer {
    fn of(spec: &EnergySpec) -> Self {
        Self {
            stored: 0,
            capacity: spec.capacity,
            max_in: spec.max_in,
            max_out: spec.max_out,
            generate: spec.generate,
            max_generate: spec.max_generate,
            uses: spec.uses,
        }
    }

    /// How much more it holds.
    pub fn room(&self) -> u64 {
        self.capacity.saturating_sub(self.stored)
    }

    /// Puts up to `amount` in, as much as there's room for, and returns how
    /// much went in. Rates are the caller's to apply.
    pub fn fill(&mut self, amount: u64) -> u64 {
        let taken = amount.min(self.room());
        self.stored += taken;
        taken
    }

    /// Takes up to `amount` out, and returns how much came out.
    pub fn drain(&mut self, amount: u64) -> u64 {
        let given = amount.min(self.stored);
        self.stored -= given;
        given
    }

    /// Takes the sizes of `spec`, keeping what's stored (even over a
    /// smaller capacity: nothing's lost, it just takes no more).
    pub(super) fn follow(&mut self, spec: &EnergySpec) {
        let new = Self {
            stored: self.stored,
            ..Self::of(spec)
        };
        *self = new;
    }
}

impl BlockEntityPart for EnergyBuffer {
    const KEY: &'static str = "energy";

    fn new(state: BlockState, content: &Content) -> Option<Self> {
        content.machine(state)?.energy.as_ref().map(Self::of)
    }

    fn save(&self, _: &Content) -> Value {
        Value::Int(self.stored.min(i64::MAX as u64) as i64)
    }

    fn load(value: &Value, version: u32, state: BlockState, content: &Content) -> Option<Self> {
        if version != 1 {
            return None;
        }
        let stored = u64::try_from(value.as_int()?).ok()?;
        Some(Self {
            stored,
            ..Self::new(state, content)?
        })
    }
}

/// A machine's fluid tanks, in order of name.
#[derive(
    Component, Reflect, Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize,
)]
#[reflect(Component)]
pub struct Tanks(pub Vec<Tank>);

/// One tank: some amount of one fluid, in [`FULL_FLUID`] units
/// (thousandths of a block, as the world counts fluid).
///
/// [`FULL_FLUID`]: crate::content::FULL_FLUID
#[derive(Reflect, Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Tank {
    pub name: String,
    /// The fluid's block id, or `None` while it's empty.
    pub fluid: Option<String>,
    pub amount: u64,
    pub capacity: u64,
    /// The fluids it takes, by block id; empty for any.
    pub accepts: Vec<String>,
}

impl Tank {
    fn of(spec: &TankSpec) -> Self {
        Self {
            name: spec.name.clone(),
            fluid: None,
            amount: 0,
            capacity: spec.capacity,
            accepts: spec.accepts.clone(),
        }
    }

    /// Whether it takes `fluid` now: one it accepts, and the one it holds
    /// if it isn't empty.
    pub fn takes(&self, fluid: &str) -> bool {
        (self.accepts.is_empty() || self.accepts.iter().any(|f| f == fluid))
            && self.fluid.as_deref().is_none_or(|own| own == fluid)
    }

    /// How much more it holds.
    pub fn room(&self) -> u64 {
        self.capacity.saturating_sub(self.amount)
    }

    /// Puts up to `amount` of `fluid` in, if it takes it, and returns how
    /// much went in.
    pub fn fill(&mut self, fluid: &str, amount: u64) -> u64 {
        if !self.takes(fluid) {
            return 0;
        }
        let taken = amount.min(self.room());
        if taken > 0 {
            self.amount += taken;
            self.fluid = Some(fluid.to_string());
        }
        taken
    }

    /// Takes up to `amount` out, and returns how much came out (of the
    /// fluid it held). Empty, it holds no fluid.
    pub fn drain(&mut self, amount: u64) -> u64 {
        let given = amount.min(self.amount);
        self.amount -= given;
        if self.amount == 0 {
            self.fluid = None;
        }
        given
    }
}

impl Tanks {
    fn of(specs: &[TankSpec]) -> Self {
        Self(specs.iter().map(Tank::of).collect())
    }

    pub fn get(&self, name: &str) -> Option<&Tank> {
        self.0.iter().find(|tank| tank.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Tank> {
        self.0.iter_mut().find(|tank| tank.name == name)
    }

    /// How much of `fluid` it holds in all its tanks.
    pub fn total(&self, fluid: &str) -> u64 {
        self.0
            .iter()
            .filter(|tank| tank.fluid.as_deref() == Some(fluid))
            .map(|tank| tank.amount)
            .sum()
    }

    /// Takes the sizes and fluids of `specs`, keeping what's in each tank.
    pub(super) fn follow(&mut self, specs: &[TankSpec]) {
        let mut new = Self::of(specs);
        for tank in &mut new.0 {
            if let Some(old) = self.get(&tank.name) {
                tank.fluid = old.fluid.clone();
                tank.amount = old.amount;
            }
        }
        *self = new;
    }
}

impl BlockEntityPart for Tanks {
    const KEY: &'static str = "tanks";

    fn new(state: BlockState, content: &Content) -> Option<Self> {
        let machine = content.machine(state)?;
        (!machine.tanks.is_empty()).then(|| Self::of(&machine.tanks))
    }

    /// Each tank that isn't empty, by name: `{ fluid, amount }`.
    fn save(&self, _: &Content) -> Value {
        Value::Table(
            self.0
                .iter()
                .filter_map(|tank| {
                    let fluid = tank.fluid.clone()?;
                    let value = Value::Table(BTreeMap::from([
                        ("fluid".to_string(), Value::Text(fluid)),
                        (
                            "amount".to_string(),
                            Value::Int(tank.amount.min(i64::MAX as u64) as i64),
                        ),
                    ]));
                    Some((tank.name.clone(), value))
                })
                .collect(),
        )
    }

    fn load(value: &Value, version: u32, state: BlockState, content: &Content) -> Option<Self> {
        if version != 1 {
            return None;
        }
        let saved = value.as_table()?;
        let mut tanks = Self::new(state, content)?;
        for (name, value) in saved {
            let Some(tank) = tanks.get_mut(name) else {
                warn!("a machine's tank `{name}` isn't there any more; its fluid is lost");
                continue;
            };
            let value = value.as_table()?;
            let Some(Value::Text(fluid)) = value.get("fluid") else {
                return None;
            };
            tank.fluid = Some(fluid.clone());
            tank.amount = u64::try_from(value.get("amount")?.as_int()?).ok()?;
        }
        Some(tanks)
    }
}

/// A machine's item slots, in groups, in order of name.
#[derive(
    Component, Reflect, Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize,
)]
#[reflect(Component)]
pub struct Slots(pub Vec<SlotGroup>);

/// A named group of slots, each empty or holding some of one item.
#[derive(Reflect, Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct SlotGroup {
    pub name: String,
    pub slots: Vec<Option<ItemStack>>,
}

/// How many of `stack`'s item one slot holds.
pub fn slot_size(content: &Content, stack: &ItemStack) -> u32 {
    if is_unique(content, stack.item) {
        1
    } else {
        SLOT_SIZE
    }
}

impl SlotGroup {
    fn of(spec: &SlotGroupSpec) -> Self {
        Self {
            name: spec.name.clone(),
            slots: vec![None; spec.count as usize],
        }
    }

    /// Puts as much of `stack` in as fits: onto slots holding the same
    /// kind first, then into empty ones, in order. Returns what didn't
    /// fit, if anything.
    pub fn insert(&mut self, content: &Content, mut stack: ItemStack) -> Option<ItemStack> {
        let size = slot_size(content, &stack);
        for slot in self.slots.iter_mut().flatten() {
            if stack.count == 0 {
                break;
            }
            if slot.same_kind(&stack) {
                let moved = stack.count.min(size.saturating_sub(slot.count));
                slot.count += moved;
                stack.count -= moved;
            }
        }
        for slot in &mut self.slots {
            if stack.count == 0 {
                break;
            }
            if slot.is_none() {
                let moved = stack.count.min(size);
                *slot = Some(ItemStack {
                    count: moved,
                    ..stack.clone()
                });
                stack.count -= moved;
            }
        }
        (stack.count > 0).then_some(stack)
    }

    /// How many of `stack`'s kind would go in.
    pub fn room_for(&self, content: &Content, stack: &ItemStack) -> u32 {
        let size = slot_size(content, stack);
        self.slots
            .iter()
            .map(|slot| match slot {
                None => size,
                Some(there) if there.same_kind(stack) => size.saturating_sub(there.count),
                Some(_) => 0,
            })
            .fold(0u32, u32::saturating_add)
    }

    /// Takes up to `count` out of slot `index`, and returns what came out.
    pub fn take(&mut self, index: usize, count: u32) -> Option<ItemStack> {
        let slot = self.slots.get_mut(index)?;
        let there = slot.as_mut()?;
        let taken = count.min(there.count);
        if taken == 0 {
            return None;
        }
        there.count -= taken;
        let out = ItemStack {
            count: taken,
            ..there.clone()
        };
        if there.count == 0 {
            *slot = None;
        }
        Some(out)
    }

    /// How many items of every kind it holds, in all.
    pub fn total(&self) -> u64 {
        self.slots.iter().flatten().map(|s| s.count as u64).sum()
    }
}

impl Slots {
    fn of(specs: &[SlotGroupSpec]) -> Self {
        Self(specs.iter().map(SlotGroup::of).collect())
    }

    pub fn group(&self, name: &str) -> Option<&SlotGroup> {
        self.0.iter().find(|group| group.name == name)
    }

    pub fn group_mut(&mut self, name: &str) -> Option<&mut SlotGroup> {
        self.0.iter_mut().find(|group| group.name == name)
    }

    /// How many items of every kind it holds, in all.
    pub fn total(&self) -> u64 {
        self.0.iter().map(SlotGroup::total).sum()
    }
}

impl BlockEntityPart for Slots {
    const KEY: &'static str = "slots";
    /// 2 added the fluid a container in a slot holds.
    const VERSION: u32 = 2;

    fn new(state: BlockState, content: &Content) -> Option<Self> {
        let machine = content.machine(state)?;
        (!machine.slots.is_empty()).then(|| Self::of(&machine.slots))
    }

    /// Each group by name: a list of its slots, each `{}` when empty or
    /// `{ item, count }`, the item by its saved name, with `fluid` and
    /// `amount` for a fluid container holding some.
    fn save(&self, content: &Content) -> Value {
        let slot = |slot: &Option<ItemStack>| {
            let mut table = BTreeMap::new();
            if let Some(stack) = slot
                && let Some(name) = content.saved_item_name(stack.item)
            {
                table.insert("item".to_string(), Value::Text(name));
                table.insert("count".to_string(), Value::Int(stack.count as i64));
                if let Some(held) = &stack.data.fluid {
                    table.insert("fluid".to_string(), Value::Text(held.fluid.clone()));
                    table.insert("amount".to_string(), Value::Int(held.amount as i64));
                }
            }
            Value::Table(table)
        };
        Value::Table(
            self.0
                .iter()
                .map(|group| {
                    let slots = group.slots.iter().map(slot).collect();
                    (group.name.clone(), Value::List(slots))
                })
                .collect(),
        )
    }

    fn load(value: &Value, version: u32, state: BlockState, content: &Content) -> Option<Self> {
        if version != Self::VERSION {
            return None;
        }
        let mut slots = Self::new(state, content)?;
        for (name, value) in value.as_table()? {
            let Value::List(saved) = value else {
                return None;
            };
            let Some(group) = slots.group_mut(name) else {
                warn!("a machine's slot group `{name}` isn't there any more; its items are lost");
                continue;
            };
            for (slot, saved) in group.slots.iter_mut().zip(saved) {
                let saved = saved.as_table()?;
                let Some(Value::Text(item)) = saved.get("item") else {
                    continue;
                };
                let count = u32::try_from(saved.get("count")?.as_int()?).ok()?;
                let Some(state) = content.saved_item(item) else {
                    warn!("a machine's slot held `{item}`, which isn't there any more");
                    continue;
                };
                let data = match saved.get("fluid") {
                    None => ItemData::default(),
                    Some(Value::Text(fluid)) => {
                        let amount = u64::try_from(saved.get("amount")?.as_int()?).ok()?;
                        saved_data(content, state, fluid, amount).unwrap_or_else(|error| {
                            warn!(
                                "a machine's slot held `{item}` with fluid, but {error}; it's lost"
                            );
                            ItemData::default()
                        })
                    }
                    Some(_) => return None,
                };
                if count > 0 {
                    *slot = Some(ItemStack {
                        item: state,
                        data,
                        count,
                    });
                }
            }
            if saved.len() > group.slots.len() {
                warn!("a machine's slot group `{name}` has fewer slots now; the rest are lost");
            }
        }
        Some(slots)
    }
}
