//! [`MachineRef`]: a machine's handle on itself in its own hooks, a block
//! handle with its energy buffer, tanks, slots, ports and active flag.
//!
//! Like block handles, the handles here are plain values that reach the
//! machine through the call's world function (`world.rs`), and only for
//! the machine the call is about. Every change goes through the methods
//! the engine's solvers use ([`EnergyBuffer::fill`], [`Tank::drain`],
//! [`SlotGroup::insert`], ...), and moves things rather than making them:
//! fluid and items only between the machine's own tanks and slots, or
//! between a tank and the world next to it, and energy is made only by
//! `generate`, up to the definition's `max_generate`, in `on_tick`.

use bevy::prelude::*;
use mlua::{
    AnyUserData, Lua, MetaMethod, MultiValue, UserData, UserDataFields, UserDataMethods, Value,
};

use super::{
    item_table,
    world::{Call, check_writable},
};
use crate::fluid_network::world::{pour, take};
use defaria_core::{
    inventory::ItemStack,
    machine::{SlotGroup, Tank},
    script::api::{
        Located, Op, add_block_fields, add_block_methods, current_serial, face_named, handle_pos,
        position, world_op,
    },
};

/// A machine, as its own hooks see it: everything a [`BlockRef`] has, and
///
/// - `m.energy`: its energy buffer ([`EnergyRef`]), or nil;
/// - `m.tanks`: its tanks by name ([`TankRef`]);
/// - `m.slots`: its slot groups by name ([`SlotsRef`]);
/// - `m.ports`: its ports, a list of `{ face, kind, role, network,
///   frozen }`;
/// - `m.active`, `m:set_active(on)`: whether it's working, which puts it
///   in its `active_state`.
///
/// [`BlockRef`]: defaria_core::script::api::BlockRef
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MachineRef {
    pos: IVec3,
    serial: u64,
}

impl MachineRef {
    /// A handle on the machine at `pos`, for the call running in `lua`,
    /// whose origin it must be.
    pub fn create(lua: &Lua, pos: IVec3) -> mlua::Result<AnyUserData> {
        lua.create_userdata(MachineRef {
            pos,
            serial: current_serial(lua),
        })
    }
}

impl Located for MachineRef {
    fn pos(&self) -> IVec3 {
        self.pos
    }

    fn serial(&self) -> u64 {
        self.serial
    }
}

/// `m.energy`: a machine's energy buffer, in joules.
///
/// - `stored`, `capacity`, `room`, and its rates in watts: `max_in`,
///   `max_out`, `max_generate`;
/// - `energy:take(n)`: uses up to `n` of what it holds (it's spent, as a
///   machine's work spends it), returning how much;
/// - `energy:generate(n)`: makes up to `n`, as much as its
///   `max_generate` allows this tick and there's room for, returning how
///   much. Only in `on_tick`; anywhere else it makes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnergyRef {
    pos: IVec3,
    serial: u64,
}

/// `m.tanks[name]`: one of a machine's tanks, in fluid units (1000 is a
/// block of fluid).
///
/// - `name`, `fluid` (its block id, or nil while empty), `amount`,
///   `capacity`, `room`;
/// - `tank:move_to(other, n?)`: moves up to `n` (all it can if nil) into
///   another of the machine's tanks;
/// - `tank:pour(target, n?)`: pours up to `n` into the voxel `target` (a
///   face name or a block handle) within one step of the machine: into
///   air, or onto the same fluid, as much as the voxel holds;
/// - `tank:draw(target, n?)`: takes up to `n` of the fluid in the voxel
///   `target`, if the tank takes it.
///
/// Each returns how much moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TankRef {
    pos: IVec3,
    serial: u64,
    name: String,
}

/// `m.slots[name]`: one of a machine's slot groups.
///
/// - `name`, `#slots` (how many slots);
/// - `slots:get(i)`: what slot `i` (from 1) holds, `{ id, name, state,
///   count }`, or nil;
/// - `slots:count(id?)`: how many items it holds in all, or of item `id`;
/// - `slots:move(i, to, n?)`: moves up to `n` (all it can if nil) out of
///   slot `i` into another of the machine's slot groups, onto slots of
///   the same kind first, returning how many moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotsRef {
    pos: IVec3,
    serial: u64,
    name: String,
}

impl UserData for MachineRef {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        add_block_fields(fields);
        fields.add_field_method_get("active", |lua, m| m.op(lua, Op::Active, ()));
        fields.add_field_method_get("ports", |lua, m| m.op(lua, Op::Ports, ()));
        fields.add_field_method_get("energy", |lua, m| {
            if m.op(lua, Op::Energy, "present")? != Value::Boolean(true) {
                return Ok(Value::Nil);
            }
            let energy = EnergyRef {
                pos: m.pos,
                serial: m.serial,
            };
            Ok(Value::UserData(lua.create_userdata(energy)?))
        });
        fields.add_field_method_get("tanks", |lua, m| {
            let names = m.op(lua, Op::TankNames, ())?;
            let table = lua.create_table()?;
            for name in lua.unpack::<Vec<String>>(names)? {
                let tank = TankRef {
                    pos: m.pos,
                    serial: m.serial,
                    name: name.clone(),
                };
                table.raw_set(name, tank)?;
            }
            table.set_readonly(true);
            Ok(table)
        });
        fields.add_field_method_get("slots", |lua, m| {
            let names = m.op(lua, Op::SlotNames, ())?;
            let table = lua.create_table()?;
            for name in lua.unpack::<Vec<String>>(names)? {
                let slots = SlotsRef {
                    pos: m.pos,
                    serial: m.serial,
                    name: name.clone(),
                };
                table.raw_set(name, slots)?;
            }
            table.set_readonly(true);
            Ok(table)
        });
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        add_block_methods(methods);
        methods.add_method("set_active", |lua, m, on: bool| {
            m.op(lua, Op::SetActive, on)
        });
    }
}

impl UserData for EnergyRef {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        for field in [
            "stored",
            "capacity",
            "room",
            "max_in",
            "max_out",
            "max_generate",
        ] {
            fields.add_field_method_get(field, move |lua, e| {
                world_op(lua, e.serial, Op::Energy, e.pos, field)
            });
        }
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("take", |lua, e, n: Value| {
            world_op(lua, e.serial, Op::EnergyTake, e.pos, amount(n)?)
        });
        methods.add_method("generate", |lua, e, n: Value| {
            world_op(lua, e.serial, Op::EnergyGenerate, e.pos, amount(n)?)
        });
    }
}

impl UserData for TankRef {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, tank| Ok(tank.name.clone()));
        for field in ["fluid", "amount", "capacity", "room"] {
            fields.add_field_method_get(field, move |lua, tank| {
                world_op(
                    lua,
                    tank.serial,
                    Op::Tank,
                    tank.pos,
                    (tank.name.as_str(), field),
                )
            });
        }
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("move_to", |lua, tank, (to, n): (Value, Value)| {
            let to = match &to {
                Value::String(name) => name.to_str()?.to_string(),
                Value::UserData(other) => other.borrow::<TankRef>()?.name.clone(),
                _ => return Err(mlua::Error::runtime("move_to takes a tank, or its name")),
            };
            let args = (tank.name.as_str(), to, amount(n)?);
            world_op(lua, tank.serial, Op::TankMove, tank.pos, args)
        });
        methods.add_method("pour", |lua, tank, (target, n): (Value, Value)| {
            let at = target_pos(lua, tank.pos, target)?;
            let args = (tank.name.as_str(), at.x, at.y, at.z, amount(n)?);
            world_op(lua, tank.serial, Op::TankPour, tank.pos, args)
        });
        methods.add_method("draw", |lua, tank, (target, n): (Value, Value)| {
            let at = target_pos(lua, tank.pos, target)?;
            let args = (tank.name.as_str(), at.x, at.y, at.z, amount(n)?);
            world_op(lua, tank.serial, Op::TankDraw, tank.pos, args)
        });
        methods.add_meta_method(MetaMethod::ToString, |_, tank, ()| {
            Ok(format!("tank `{}`", tank.name))
        });
    }
}

impl UserData for SlotsRef {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("name", |_, slots| Ok(slots.name.clone()));
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Len, |lua, slots, ()| {
            world_op(lua, slots.serial, Op::Slots, slots.pos, slots.name.as_str())
        });
        methods.add_method("get", |lua, slots, index: i64| {
            let args = (slots.name.as_str(), index);
            world_op(lua, slots.serial, Op::SlotGet, slots.pos, args)
        });
        methods.add_method("count", |lua, slots, id: Option<String>| {
            let args = (slots.name.as_str(), id);
            world_op(lua, slots.serial, Op::SlotCount, slots.pos, args)
        });
        methods.add_method("move", |lua, slots, (index, to, n): (i64, Value, Value)| {
            let to = match &to {
                Value::String(name) => name.to_str()?.to_string(),
                Value::UserData(other) => other.borrow::<SlotsRef>()?.name.clone(),
                _ => {
                    return Err(mlua::Error::runtime("move takes a slot group, or its name"));
                }
            };
            let args = (slots.name.as_str(), index, to, amount(n)?);
            world_op(lua, slots.serial, Op::SlotMove, slots.pos, args)
        });
        methods.add_meta_method(MetaMethod::ToString, |_, slots, ()| {
            Ok(format!("slots `{}`", slots.name))
        });
    }
}

/// An amount a script asked for: a whole number, at least 0, or nil for
/// as much as there is. Fractions round down.
fn amount(n: Value) -> mlua::Result<f64> {
    let n = match n {
        Value::Nil => return Ok(f64::INFINITY),
        Value::Integer(n) => n as f64,
        Value::Number(n) => n,
        other => {
            return Err(mlua::Error::runtime(format!(
                "an amount is a number, not a {}",
                other.type_name()
            )));
        }
    };
    if n.is_nan() || n < 0.0 {
        return Err(mlua::Error::runtime(format!(
            "an amount can't be {n}: it's 0 or more"
        )));
    }
    Ok(n.floor())
}

/// An amount as a whole number of units.
fn units(n: f64) -> u64 {
    if n >= u64::MAX as f64 {
        u64::MAX
    } else {
        n as u64
    }
}

/// Where a tank pours or draws: a face of the machine at `pos`, by name,
/// or a block handle or position.
fn target_pos(lua: &Lua, pos: IVec3, target: Value) -> mlua::Result<IVec3> {
    match target {
        Value::String(face) => Ok(pos + face_named(&face.to_str()?)?.normal()),
        Value::UserData(handle) => handle_pos(&handle)
            .ok_or_else(|| mlua::Error::runtime("a target is a face, a block or a position")),
        Value::Vector(_) => position(lua, MultiValue::from_vec(vec![target])),
        _ => Err(mlua::Error::runtime(
            "a target is a face, a block or a position",
        )),
    }
}

impl Call<'_, '_, '_> {
    /// A machine handle's `op`, for the machine at `pos`: the call's
    /// origin, which must have a block entity with machine parts.
    pub(super) fn machine_op(
        &mut self,
        lua: &Lua,
        op: Op,
        pos: IVec3,
        args: MultiValue,
    ) -> mlua::Result<Value> {
        if self.origin != Some(pos) {
            return Err(mlua::Error::runtime(
                "a machine's handle works only in its own hooks",
            ));
        }
        let content = self.content;
        let entity = self
            .block_entities
            .get(pos)
            .ok_or_else(|| mlua::Error::runtime("the machine isn't there any more"))?;
        let mut args = args.into_iter();
        let mut arg = move || args.next().unwrap_or(Value::Nil);
        let Ok((energy, tanks, slots, ports, active)) = self.machines.get_mut(entity) else {
            return Err(mlua::Error::runtime("the machine isn't there any more"));
        };
        let no_tank =
            |name: &str| mlua::Error::runtime(format!("the machine has no tank `{name}`"));
        let no_slots =
            |name: &str| mlua::Error::runtime(format!("the machine has no slot group `{name}`"));
        Ok(match op {
            Op::Active => Value::Boolean(active.is_some_and(|a| a.0)),
            Op::SetActive => {
                let on: bool = lua.unpack(arg())?;
                let Some(mut active) = active else {
                    return Ok(Value::Boolean(false));
                };
                let changed = active.0 != on;
                if changed {
                    active.0 = on;
                }
                Value::Boolean(changed)
            }
            Op::Ports => {
                let list = lua.create_table()?;
                for link in ports.map(|p| p.0.as_slice()).unwrap_or_default() {
                    let port = lua.create_table()?;
                    port.raw_set("face", link.face.name())?;
                    port.raw_set("kind", link.kind.as_str())?;
                    port.raw_set("role", link.role.name())?;
                    port.raw_set("network", link.network.map(|id| id.0))?;
                    let frozen = link
                        .network
                        .and_then(|id| self.networks?.get(id))
                        .is_some_and(|network| network.is_frozen());
                    port.raw_set("frozen", frozen)?;
                    list.push(port)?;
                }
                Value::Table(list)
            }
            Op::TankNames => {
                let names = tanks
                    .iter()
                    .flat_map(|t| t.0.iter().map(|t| t.name.as_str()));
                Value::Table(lua.create_sequence_from(names)?)
            }
            Op::SlotNames => {
                let names = slots
                    .iter()
                    .flat_map(|s| s.0.iter().map(|g| g.name.as_str()));
                Value::Table(lua.create_sequence_from(names)?)
            }
            Op::Energy => {
                let field: String = lua.unpack(arg())?;
                let Some(energy) = energy else {
                    return Ok(match field.as_str() {
                        "present" => Value::Boolean(false),
                        _ => Value::Nil,
                    });
                };
                let value = match field.as_str() {
                    "present" => return Ok(Value::Boolean(true)),
                    "stored" => energy.stored,
                    "capacity" => energy.capacity,
                    "room" => energy.room(),
                    "max_in" => energy.max_in,
                    "max_out" => energy.max_out,
                    "max_generate" => energy.max_generate,
                    _ => return Ok(Value::Nil),
                };
                lua.pack(value)?
            }
            Op::EnergyTake | Op::EnergyGenerate => {
                let want = units(lua.unpack(arg())?);
                let Some(mut energy) = energy else {
                    return Err(mlua::Error::runtime("the machine has no energy buffer"));
                };
                let moved = if op == Op::EnergyTake {
                    let taken = want.min(energy.stored);
                    if taken > 0 {
                        energy.drain(taken);
                        if let Some(report) = self.report.as_deref_mut() {
                            report.used += taken;
                        }
                    }
                    taken
                } else {
                    let made = want.min(self.generate).min(energy.room());
                    if made > 0 {
                        energy.fill(made);
                        self.generate -= made;
                        if let Some(report) = self.report.as_deref_mut() {
                            report.generated += made;
                        }
                    }
                    made
                };
                lua.pack(moved)?
            }
            Op::Tank => {
                let name: String = lua.unpack(arg())?;
                let field: String = lua.unpack(arg())?;
                let tank = tanks
                    .as_deref()
                    .and_then(|t| t.get(&name))
                    .ok_or_else(|| no_tank(&name))?;
                match field.as_str() {
                    "fluid" => lua.pack(tank.fluid.as_deref())?,
                    "amount" => lua.pack(tank.amount)?,
                    "capacity" => lua.pack(tank.capacity)?,
                    "room" => lua.pack(tank.room())?,
                    _ => Value::Nil,
                }
            }
            Op::TankMove => {
                let from: String = lua.unpack(arg())?;
                let to: String = lua.unpack(arg())?;
                let want = units(lua.unpack(arg())?);
                let mut tanks = tanks.ok_or_else(|| no_tank(&from))?;
                let a = tanks.0.iter().position(|t| t.name == from);
                let b = tanks.0.iter().position(|t| t.name == to);
                let (a, b) = (
                    a.ok_or_else(|| no_tank(&from))?,
                    b.ok_or_else(|| no_tank(&to))?,
                );
                if a == b {
                    return lua.pack(0);
                }
                let moved = move_fluid(&tanks.0[a].clone(), &tanks.0[b].clone(), want);
                if moved > 0 {
                    let fluid = tanks.0[a]
                        .fluid
                        .clone()
                        .expect("a tank giving holds a fluid");
                    let drained = tanks.0[a].drain(moved);
                    let filled = tanks.0[b].fill(&fluid, drained);
                    debug_assert_eq!((drained, filled), (moved, moved));
                }
                lua.pack(moved)?
            }
            Op::TankPour | Op::TankDraw => {
                let name: String = lua.unpack(arg())?;
                let at = IVec3::new(lua.unpack(arg())?, lua.unpack(arg())?, lua.unpack(arg())?);
                let want = units(lua.unpack(arg())?);
                let mut tanks = tanks.ok_or_else(|| no_tank(&name))?;
                let index = tanks
                    .0
                    .iter()
                    .position(|t| t.name == name)
                    .ok_or_else(|| no_tank(&name))?;
                if at == pos {
                    return Err(mlua::Error::runtime(
                        "a machine can't pour into or draw from its own voxel",
                    ));
                }
                check_writable(self.origin, at)?;
                let mut tank = tanks.0[index].clone();
                let moved = if op == Op::TankPour {
                    pour(self.voxels, content, at, want, &mut tank)
                } else {
                    take(self.voxels, content, at, want, &mut tank)
                };
                if moved > 0 {
                    tanks.0[index] = tank;
                }
                lua.pack(moved)?
            }
            Op::Slots => {
                let name: String = lua.unpack(arg())?;
                let group = slots
                    .as_deref()
                    .and_then(|s| s.group(&name))
                    .ok_or_else(|| no_slots(&name))?;
                lua.pack(group.slots.len())?
            }
            Op::SlotGet => {
                let name: String = lua.unpack(arg())?;
                let index: i64 = lua.unpack(arg())?;
                let group = slots
                    .as_deref()
                    .and_then(|s| s.group(&name))
                    .ok_or_else(|| no_slots(&name))?;
                match slot_index(group, index)?.and_then(|i| group.slots[i].as_ref()) {
                    Some(stack) => {
                        let table = item_table(lua, content, stack.item)?;
                        table.raw_set("count", stack.count)?;
                        Value::Table(table)
                    }
                    None => Value::Nil,
                }
            }
            Op::SlotCount => {
                let name: String = lua.unpack(arg())?;
                let id: Option<String> = lua.unpack(arg())?;
                let group = slots
                    .as_deref()
                    .and_then(|s| s.group(&name))
                    .ok_or_else(|| no_slots(&name))?;
                let count: u64 = group
                    .slots
                    .iter()
                    .flatten()
                    .filter(|stack| {
                        id.as_deref().is_none_or(|id| {
                            content
                                .item_of(stack.item)
                                .is_some_and(|item| item.id == id)
                        })
                    })
                    .map(|stack| stack.count as u64)
                    .sum();
                lua.pack(count)?
            }
            Op::SlotMove => {
                let from: String = lua.unpack(arg())?;
                let index: i64 = lua.unpack(arg())?;
                let to: String = lua.unpack(arg())?;
                let want = units(lua.unpack(arg())?).min(u32::MAX as u64) as u32;
                let mut slots = slots.ok_or_else(|| no_slots(&from))?;
                let a = slots.0.iter().position(|g| g.name == from);
                let b = slots.0.iter().position(|g| g.name == to);
                let (a, b) = (
                    a.ok_or_else(|| no_slots(&from))?,
                    b.ok_or_else(|| no_slots(&to))?,
                );
                let Some(index) = slot_index(&slots.0[a], index)? else {
                    return lua.pack(0);
                };
                if a == b {
                    return lua.pack(0);
                }
                let Some(there) = slots.0[a].slots[index].clone() else {
                    return lua.pack(0);
                };
                // A recipe's input slots take only what its recipes use,
                // as from a tube or the player's hand.
                let takes = self
                    .voxels
                    .get(pos)
                    .and_then(|state| content.machine(state))
                    .is_none_or(|machine| content.slot_group_takes(machine, b, there.item));
                if !takes {
                    return lua.pack(0);
                }
                let room = slots.0[b].room_for(content, &there);
                let count = want.min(there.count).min(room);
                if count == 0 {
                    return lua.pack(0);
                }
                let taken = slots.0[a]
                    .take(index, count)
                    .expect("the slot holds what it's taken from");
                let back = slots.0[b].insert(content, taken);
                debug_assert!(back.is_none(), "it had room");
                if let Some(back) = back {
                    put_back(&mut slots.0[a], index, back);
                }
                lua.pack(count)?
            }
            _ => unreachable!("not a machine op"),
        })
    }
}

/// How much of tank `a`'s fluid can move into `b`, up to `want`.
fn move_fluid(a: &Tank, b: &Tank, want: u64) -> u64 {
    let Some(fluid) = &a.fluid else {
        return 0;
    };
    if !b.takes(fluid) {
        return 0;
    }
    want.min(a.amount).min(b.room())
}

/// Slot `index` (from 1) of `group`, as an index into it; `None` past its
/// end. Less than 1 is an error.
fn slot_index(group: &SlotGroup, index: i64) -> mlua::Result<Option<usize>> {
    if index < 1 {
        return Err(mlua::Error::runtime(format!(
            "slot {index} of `{}`: slots count from 1",
            group.name
        )));
    }
    let index = (index - 1) as usize;
    Ok((index < group.slots.len()).then_some(index))
}

/// Puts `stack`, just taken out of slot `index`, back in it.
fn put_back(group: &mut SlotGroup, index: usize, stack: ItemStack) {
    match &mut group.slots[index] {
        Some(there) => there.count += stack.count,
        empty => *empty = Some(stack),
    }
}
