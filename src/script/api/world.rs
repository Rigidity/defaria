//! [`ScriptWorld`]: what a hook call can reach of the world, and calling
//! a hook with it.
//!
//! Block handles ([`BlockRef`]) and the `world` global are plain Lua
//! values that reach the world through one function the call puts in the
//! Lua state's app data for as long as it lasts, made in the call's
//! scope. So a handle a script keeps past its call stops working: the
//! function is gone, and a handle from an earlier call is refused by its
//! serial.

use std::{
    cell::RefCell,
    sync::atomic::{AtomicU64, Ordering},
};

use bevy::{ecs::system::SystemParam, prelude::*};
use mlua::{FromLuaMulti, Function, IntoLuaMulti, Lua, MultiValue, Scope, Table, Value};

use super::{
    block::BlockRef,
    value::{table_from_lua, table_to_lua},
};
use crate::{
    block_entity::{BlockData, BlockEntities},
    content::{BlockState, Content, Direction},
    light::ChunkLight,
    machine::{EnergyBuffer, MachineActive, Ports, Slots, Tanks},
    network::{Networks, energy::EnergyReport},
    script::{CallError, Caller, ScriptRuntime},
    world::{ChunkCoord, ChunkMap, Voxels},
};

/// The running call's world function, as app data: looked up on every
/// world operation, so it's kept where that's cheapest.
struct CallWorld(Function);

/// Every call's serial, so a handle can tell its call is over.
static SERIAL: AtomicU64 = AtomicU64::new(1);

/// The serial of the call running in a Lua state, as app data.
#[derive(Debug, Clone, Copy)]
pub(super) struct CallSerial(pub u64);

/// What a block handle or `world` asks of the call's world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Op {
    Loaded,
    Id,
    Name,
    State,
    Stat,
    SetState,
    Set,
    Data,
    BlockDef,
    ItemDef,
    MaterialDef,
    Light,
    Fluid,
    // A machine's own handle's, in its own hooks: see `machine.rs`.
    Active,
    SetActive,
    Ports,
    TankNames,
    SlotNames,
    Energy,
    EnergyTake,
    EnergyGenerate,
    Tank,
    TankMove,
    TankPour,
    TankDraw,
    Slots,
    SlotGet,
    SlotCount,
    SlotMove,
}

impl Op {
    const ALL: [Op; 29] = [
        Op::Loaded,
        Op::Id,
        Op::Name,
        Op::State,
        Op::Stat,
        Op::SetState,
        Op::Set,
        Op::Data,
        Op::BlockDef,
        Op::ItemDef,
        Op::MaterialDef,
        Op::Light,
        Op::Fluid,
        Op::Active,
        Op::SetActive,
        Op::Ports,
        Op::TankNames,
        Op::SlotNames,
        Op::Energy,
        Op::EnergyTake,
        Op::EnergyGenerate,
        Op::Tank,
        Op::TankMove,
        Op::TankPour,
        Op::TankDraw,
        Op::Slots,
        Op::SlotGet,
        Op::SlotCount,
        Op::SlotMove,
    ];

    /// Whether it's a machine handle's, for the machine the call is about.
    fn is_machine(self) -> bool {
        self as u8 >= Op::Active as u8
    }
}

/// Asks the current call's world for `op` at `pos`, for a handle made in
/// call `serial`.
pub(super) fn world_op(
    lua: &Lua,
    serial: u64,
    op: Op,
    pos: IVec3,
    args: impl IntoLuaMulti,
) -> mlua::Result<Value> {
    let function = lua.app_data_ref::<CallWorld>().map(|world| world.0.clone());
    let Some(function) = function else {
        return Err(mlua::Error::runtime(
            "the world can only be reached while a hook runs",
        ));
    };
    // `dispatch` turns the number back into the `Op` at that index of
    // `Op::ALL`, so each must be at its own number there; checked as the
    // game compiles.
    const _: () = {
        let mut i = 0;
        while i < Op::ALL.len() {
            assert!(Op::ALL[i] as usize == i, "`Op::ALL` is out of order");
            i += 1;
        }
    };
    function.call((serial, op as u8, pos.x, pos.y, pos.z, args))
}

/// The serial of the call running in `lua`, for a new handle.
pub(super) fn current_serial(lua: &Lua) -> u64 {
    lua.app_data_ref::<CallSerial>()
        .map_or(0, |serial| serial.0)
}

/// The parts of a machine's block entity a machine handle reaches.
pub(super) type MachineParts = (
    Option<&'static mut EnergyBuffer>,
    Option<&'static mut Tanks>,
    Option<&'static mut Slots>,
    Option<&'static Ports>,
    Option<&'static mut MachineActive>,
);

/// The world as hooks see it: blocks to read and change through
/// [`Voxels`] (so every change is an ordinary edit, with its
/// [`VoxelChanged`](crate::world::VoxelChanged) or
/// [`PropertiesUpdated`](crate::world::PropertiesUpdated)), the content,
/// the [`BlockData`] of block entities, a scripted block's `data`, and
/// the light. A machine's hooks also reach its own energy buffer, tanks
/// and slots, through the same methods the engine's solvers use.
#[derive(SystemParam)]
pub struct ScriptWorld<'w, 's> {
    pub content: Res<'w, Content>,
    pub voxels: Voxels<'w, 's>,
    block_entities: Res<'w, BlockEntities>,
    data: Query<'w, 's, &'static mut BlockData>,
    map: Res<'w, ChunkMap>,
    lights: Query<'w, 's, &'static ChunkLight>,
    machines: Query<'w, 's, MachineParts>,
    networks: Option<Res<'w, Networks>>,
    report: Option<ResMut<'w, EnergyReport>>,
}

impl<'w, 's> ScriptWorld<'w, 's> {
    /// Calls `hook` of script `script` for `caller`, with the arguments
    /// `args` makes (block handles with [`BlockRef::create`]). While it
    /// runs, the script can change the blocks within one step of `origin`
    /// (that block and the six next to it), and read any loaded block.
    /// Whatever it leaves in the `data` of blocks is saved to their
    /// [`BlockData`] when the hook returns, if the hook succeeds.
    ///
    /// For arguments that borrow engine state as scoped userdata, use
    /// [`ScriptWorld::begin`] and [`ScriptRuntime::call_scoped`] instead.
    pub fn call<A, R>(
        &mut self,
        runtime: &mut ScriptRuntime,
        caller: Caller,
        script: &str,
        hook: &str,
        origin: Option<IVec3>,
        args: impl FnOnce(&Lua, &Content) -> mlua::Result<A>,
    ) -> Result<Option<R>, CallError>
    where
        A: IntoLuaMulti,
        R: FromLuaMulti,
    {
        if !runtime.has_hook(script, hook) {
            // Checked first, so a call that won't run sets nothing up.
            return runtime.call_with(caller, script, hook, ());
        }
        let call = self.begin(origin);
        runtime.call_scoped(caller, script, hook, |lua, scope, function| {
            call.run(lua, scope, function, args)
        })
    }

    /// Sets up a call that can change the blocks within one step of
    /// `origin`, to run inside [`ScriptRuntime::call_scoped`] with
    /// [`WorldCall::run`]:
    ///
    /// ```ignore
    /// let call = world.begin(Some(pos));
    /// runtime.call_scoped(caller, script, "on_tick", |lua, scope, hook| {
    ///     let machine = scope.create_userdata_ref_mut(&mut machine)?;
    ///     call.run(lua, scope, hook, |lua, _| Ok((BlockRef::create(lua, pos)?, machine)))
    /// })
    /// ```
    pub fn begin(&mut self, origin: Option<IVec3>) -> WorldCall<'_, 'w, 's> {
        let ScriptWorld {
            content,
            voxels,
            block_entities,
            data,
            map,
            lights,
            machines,
            networks,
            report,
        } = self;
        WorldCall(RefCell::new(Call {
            content,
            voxels,
            block_entities,
            data,
            map,
            lights,
            machines,
            networks: networks.as_deref(),
            report: report.as_deref_mut(),
            origin,
            serial: SERIAL.fetch_add(1, Ordering::Relaxed),
            tables: Vec::new(),
            generate: 0,
        }))
    }

    /// The block state at `pos`, if it's loaded.
    pub fn get(&self, pos: IVec3) -> Option<BlockState> {
        self.voxels.get(pos)
    }

    /// Whether the voxel at `pos` has its block entity yet. One placed
    /// this tick gets it when the frame's commands are applied.
    pub fn has_block_entity(&self, pos: IVec3) -> bool {
        self.block_entities.get(pos).is_some()
    }
}

/// A call set up by [`ScriptWorld::begin`].
pub struct WorldCall<'a, 'w, 's>(RefCell<Call<'a, 'w, 's>>);

impl WorldCall<'_, '_, '_> {
    /// Lets the machine at the call's origin make up to `energy` with
    /// `m.energy:generate(n)` during the call: its definition's
    /// `max_generate` share of a network tick, for `on_tick`. Without it,
    /// a call makes none.
    pub fn allow_generating(self, energy: u64) -> Self {
        self.0.borrow_mut().generate = energy;
        self
    }

    /// Calls `hook` with the arguments `args` makes, with the world
    /// reachable while it runs, then saves the `data` it changed. Call it
    /// in [`ScriptRuntime::call_scoped`]'s closure, with what that gets.
    pub fn run<'scope, 'env, A, R>(
        &'env self,
        lua: &Lua,
        scope: &'scope Scope<'scope, 'env>,
        hook: &Function,
        args: impl FnOnce(&Lua, &Content) -> mlua::Result<A>,
    ) -> mlua::Result<R>
    where
        A: IntoLuaMulti,
        R: FromLuaMulti,
    {
        let (serial, content) = {
            let call = self.0.borrow();
            (call.serial, call.content)
        };
        lua.set_app_data(CallSerial(serial));
        let dispatch = scope.create_function(
            move |lua, (serial, op, x, y, z, args): (u64, u8, i32, i32, i32, MultiValue)| {
                self.0
                    .borrow_mut()
                    .dispatch(lua, serial, op, IVec3::new(x, y, z), args)
            },
        )?;
        lua.set_app_data(CallWorld(dispatch));
        let result = args(lua, content)
            .and_then(|args| hook.call::<R>(args))
            .and_then(|result| self.0.borrow_mut().save_data().map(|()| result));
        lua.remove_app_data::<CallWorld>();
        result
    }
}

/// One call's view of the world.
pub(super) struct Call<'a, 'w, 's> {
    pub(super) content: &'a Content,
    pub(super) voxels: &'a mut Voxels<'w, 's>,
    pub(super) block_entities: &'a BlockEntities,
    data: &'a mut Query<'w, 's, &'static mut BlockData>,
    map: &'a ChunkMap,
    lights: &'a Query<'w, 's, &'static ChunkLight>,
    pub(super) machines: &'a mut Query<'w, 's, MachineParts>,
    pub(super) networks: Option<&'a Networks>,
    pub(super) report: Option<&'a mut EnergyReport>,
    pub(super) origin: Option<IVec3>,
    serial: u64,
    /// The `data` tables handed out, to save when the hook returns.
    tables: Vec<(IVec3, Table)>,
    /// What the origin's machine may still make in this call.
    pub(super) generate: u64,
}

impl Call<'_, '_, '_> {
    fn dispatch(
        &mut self,
        lua: &Lua,
        serial: u64,
        op: u8,
        pos: IVec3,
        args: MultiValue,
    ) -> mlua::Result<Value> {
        if serial != self.serial {
            return Err(mlua::Error::runtime(
                "a block handle kept from an earlier call: handles last only as long as the \
                 hook they were given to",
            ));
        }
        let op = *Op::ALL
            .get(op as usize)
            .ok_or_else(|| mlua::Error::runtime("no such world operation"))?;
        if op.is_machine() {
            return self.machine_op(lua, op, pos, args);
        }
        let mut args = args.into_iter();
        let mut arg = move || args.next().unwrap_or(Value::Nil);
        let content = self.content;
        if let Op::BlockDef | Op::ItemDef | Op::MaterialDef = op {
            let id: String = lua.unpack(arg())?;
            return match op {
                Op::BlockDef => block_def(lua, content, &id),
                Op::ItemDef => item_def(lua, content, &id),
                _ => material_def(lua, content, &id),
            };
        }
        let Some(state) = self.voxels.get(pos) else {
            return Ok(match op {
                Op::Loaded => Value::Boolean(false),
                Op::SetState | Op::Set => Value::Boolean(false),
                _ => Value::Nil,
            });
        };
        let info = content.state(state);
        Ok(match op {
            Op::Loaded => Value::Boolean(true),
            Op::Id => lua.pack(content.block_of(state).id.as_str())?,
            Op::Name => lua.pack(info.name.as_str())?,
            Op::State => Value::Table(state_table(lua, &info.values)?),
            Op::Stat => {
                let name: String = lua.unpack(arg())?;
                lua.pack(info.stat(&name))?
            }
            Op::SetState => {
                self.check_writable(pos)?;
                let changes: Table = lua.unpack(arg())?;
                let after = with_values(content, state, &changes)?;
                Value::Boolean(self.voxels.update(pos, after))
            }
            Op::Set => {
                self.check_writable(pos)?;
                let id: String = lua.unpack(arg())?;
                let values: Option<Table> = lua.unpack(arg())?;
                let block = content
                    .block(&id)
                    .ok_or_else(|| mlua::Error::runtime(format!("there's no block `{id}`")))?;
                let base = block.default_state();
                let after = match &values {
                    Some(values) => with_values(content, base, values)?,
                    None => base,
                };
                // Fluid the new block leaves no room for would be pushed
                // aside, or deleted where there's no room around.
                let fluid = self.voxels.get_with_fluid(pos).and_then(|(_, fluid)| fluid);
                let spills =
                    fluid.is_some_and(|fluid| fluid.amount > content.state(after).fluid_room);
                if spills || content.state(after).is_fluid() {
                    // Fluid is only ever moved, so none is made or lost.
                    return Err(mlua::Error::runtime(
                        "`set` can't put fluid in a voxel or replace it: a machine's tanks \
                         pour and draw it",
                    ));
                }
                if content.state(after).block == info.block {
                    // Another state of the same block: not a new block.
                    Value::Boolean(self.voxels.update(pos, after))
                } else if content.block_of(after).has_block_entity() {
                    // A new block entity would come from nothing: a
                    // machine (a creative generator, an item source) made
                    // without being placed, which mining then gives.
                    return Err(mlua::Error::runtime(format!(
                        "`set` can't make `{id}`: it has a block entity, so only placing it \
                         makes one"
                    )));
                } else if content.block_of(state).has_block_entity() {
                    // Its block entity would go, with what it holds: a
                    // machine's contents, a pipe's fluid, its `data`.
                    return Err(mlua::Error::runtime(format!(
                        "`set` can't replace `{}`: it has a block entity, which would be lost \
                         with what it holds",
                        content.block_of(state).id
                    )));
                } else {
                    self.tables.retain(|(at, _)| *at != pos);
                    Value::Boolean(self.voxels.set(pos, after) && after != state)
                }
            }
            Op::Data => self.data_table(lua, pos)?.map_or(Value::Nil, Value::Table),
            Op::Light => self.light(lua, pos)?,
            Op::Fluid => match self.voxels.get_with_fluid(pos).and_then(|(_, fluid)| fluid) {
                Some(fluid) => {
                    let table = lua.create_table_with_capacity(0, 2)?;
                    table.raw_set("id", content.block_of(fluid.kind).id.as_str())?;
                    table.raw_set("amount", fluid.amount)?;
                    Value::Table(table)
                }
                None => Value::Nil,
            },
            _ => unreachable!(),
        })
    }

    /// `block.light`: the light at `pos`, or nil where there's none.
    fn light(&self, lua: &Lua, pos: IVec3) -> mlua::Result<Value> {
        let (coord, local) = ChunkCoord::split(pos);
        let Some(light) = self
            .map
            .get(coord)
            .and_then(|chunk| self.lights.get(chunk).ok())
            .map(|light| light.at(local))
        else {
            return Ok(Value::Nil);
        };
        let table = lua.create_table_with_capacity(0, 4)?;
        for (channel, name) in ["sky", "red", "green", "blue"].into_iter().enumerate() {
            table.raw_set(name, light.get(channel))?;
        }
        Ok(Value::Table(table))
    }

    /// Whether the call may change the block at `pos`: within one step of
    /// its origin.
    fn check_writable(&self, pos: IVec3) -> mlua::Result<()> {
        check_writable(self.origin, pos)
    }

    /// The `data` table of the block entity at `pos`, the same one each
    /// time in a call. `None` if it has no [`BlockData`].
    fn data_table(&mut self, lua: &Lua, pos: IVec3) -> mlua::Result<Option<Table>> {
        if let Some((_, table)) = self.tables.iter().find(|(at, _)| *at == pos) {
            return Ok(Some(table.clone()));
        }
        let Some(data) = self
            .block_entities
            .get(pos)
            .and_then(|entity| self.data.get(entity).ok())
        else {
            return Ok(None);
        };
        let table = table_to_lua(lua, &data.0)?;
        if self.check_writable(pos).is_err() {
            // Out of the call's reach: a copy to read, which can't be
            // changed, and isn't saved.
            Self::set_readonly_deep(&table)?;
            return Ok(Some(table));
        }
        self.tables.push((pos, table.clone()));
        Ok(Some(table))
    }

    /// The tables in a `data` table, and it, made read-only.
    fn set_readonly_deep(table: &Table) -> mlua::Result<()> {
        for pair in table.pairs::<Value, Value>() {
            if let (_, Value::Table(inner)) = pair? {
                Self::set_readonly_deep(&inner)?;
            }
        }
        table.set_readonly(true);
        Ok(())
    }

    /// Writes back each `data` table handed out, where it changed.
    /// All or nothing: if one can't be saved, none is.
    fn save_data(&mut self) -> mlua::Result<()> {
        let saved = std::mem::take(&mut self.tables)
            .into_iter()
            .map(|(pos, table)| Ok((pos, table_from_lua(&table)?)))
            .collect::<mlua::Result<Vec<_>>>()?;
        for (pos, entries) in saved {
            let Some(mut data) = self
                .block_entities
                .get(pos)
                .and_then(|entity| self.data.get_mut(entity).ok())
            else {
                continue;
            };
            if data.0 != entries {
                data.0 = entries;
            }
        }
        Ok(())
    }
}

/// Whether a call with `origin` may change the block at `pos`: within one
/// step of it.
pub(super) fn check_writable(origin: Option<IVec3>, pos: IVec3) -> mlua::Result<()> {
    match origin {
        // In i64: any two positions' distance fits.
        Some(origin) if (pos.as_i64vec3() - origin.as_i64vec3()).abs().element_sum() <= 1 => Ok(()),
        _ => Err(mlua::Error::runtime(format!(
            "can't change the block at ({}, {}, {}): a hook can change only its own \
             block and the six next to it",
            pos.x, pos.y, pos.z
        ))),
    }
}

/// A state's property values as a table: `"true"` and `"false"` as
/// booleans, everything else as text.
pub(in crate::script) fn state_table(
    lua: &Lua,
    values: &[(String, String)],
) -> mlua::Result<Table> {
    let table = lua.create_table_with_capacity(0, values.len())?;
    for (name, value) in values {
        match value.as_str() {
            "true" => table.raw_set(name.as_str(), true)?,
            "false" => table.raw_set(name.as_str(), false)?,
            text => table.raw_set(name.as_str(), text)?,
        }
    }
    Ok(table)
}

/// A property value a script gave, as it's written in content.
fn value_text(name: &str, value: Value) -> mlua::Result<String> {
    Ok(match value {
        Value::Boolean(b) => b.to_string(),
        Value::Integer(n) => n.to_string(),
        Value::Number(n) if n.fract() == 0.0 => (n as i64).to_string(),
        Value::String(text) => text.to_str()?.to_string(),
        other => {
            return Err(mlua::Error::runtime(format!(
                "property `{name}` can't be a {}",
                other.type_name()
            )));
        }
    })
}

/// `state` with the property values in `changes` set.
fn with_values(content: &Content, state: BlockState, changes: &Table) -> mlua::Result<BlockState> {
    let block = content.block_of(state);
    let mut values = content.state(state).values.clone();
    for pair in changes.pairs::<String, Value>() {
        let (name, value) = pair?;
        let text = value_text(&name, value)?;
        let property = block
            .properties
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| {
                mlua::Error::runtime(format!("block `{}` has no property `{name}`", block.id))
            })?;
        if !property.values.contains(&text) {
            return Err(mlua::Error::runtime(format!(
                "block `{}`'s `{name}` can't be `{text}`: it's one of {}",
                block.id,
                property.values.join(", ")
            )));
        }
        if let Some(slot) = values.iter_mut().find(|(n, _)| *n == name) {
            slot.1 = text;
        }
    }
    let pairs: Vec<(&str, &str)> = values
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    block
        .state(&pairs)
        .ok_or_else(|| mlua::Error::runtime("no such state"))
}

/// `world.block_def(id)`: a block's id and properties, or nil.
fn block_def(lua: &Lua, content: &Content, id: &str) -> mlua::Result<Value> {
    let Some(block) = content.block(id) else {
        return Ok(Value::Nil);
    };
    let table = lua.create_table()?;
    table.raw_set("id", block.id.as_str())?;
    table.raw_set("name", content.state(block.default_state()).name.as_str())?;
    let properties = lua.create_table()?;
    for property in &block.properties {
        properties.raw_set(
            property.name.as_str(),
            lua.create_sequence_from(property.values.iter().map(String::as_str))?,
        )?;
    }
    table.raw_set("properties", properties)?;
    table.raw_set("script", block.script.as_deref())?;
    Ok(Value::Table(table))
}

/// `world.item_def(id)`: an item's id, name and the block it places, or
/// nil.
fn item_def(lua: &Lua, content: &Content, id: &str) -> mlua::Result<Value> {
    let Some(item) = content.item(id) else {
        return Ok(Value::Nil);
    };
    let info = &content.item_states[item.first_state as usize];
    let table = lua.create_table()?;
    table.raw_set("id", item.id.as_str())?;
    table.raw_set("name", info.name.as_str())?;
    table.raw_set(
        "block",
        info.block.map(|state| content.block_of(state).id.as_str()),
    )?;
    table.raw_set("script", item.script.as_deref())?;
    Ok(Value::Table(table))
}

/// `world.material_def(id)`: a material's id, name, tags and stats, or
/// nil.
fn material_def(lua: &Lua, content: &Content, id: &str) -> mlua::Result<Value> {
    let Some(material) = content.materials.get(id) else {
        return Ok(Value::Nil);
    };
    let table = lua.create_table()?;
    table.raw_set("id", material.id.as_str())?;
    table.raw_set("name", material.name.as_str())?;
    table.raw_set(
        "tags",
        lua.create_sequence_from(material.tags.iter().map(String::as_str))?,
    )?;
    let stats = lua.create_table()?;
    stats.raw_set("hardness", material.hardness)?;
    stats.raw_set("weight", material.weight)?;
    for (name, value) in &material.stats {
        stats.raw_set(name.as_str(), *value)?;
    }
    table.raw_set("stats", stats)?;
    Ok(Value::Table(table))
}

/// The `world` global: `world.block(pos)`, `world.block_def(id)`,
/// `world.item_def(id)` and `world.material_def(id)`. Read-only.
pub(crate) fn world_table(lua: &Lua) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.raw_set(
        "block",
        lua.create_function(|lua, args: MultiValue| {
            let pos = super::block::position(lua, args)?;
            BlockRef::loaded(lua, pos)
        })?,
    )?;
    table.raw_set(
        "block_def",
        lua.create_function(|lua, id: String| {
            world_op(lua, current_serial(lua), Op::BlockDef, IVec3::ZERO, id)
        })?,
    )?;
    table.raw_set(
        "item_def",
        lua.create_function(|lua, id: String| {
            world_op(lua, current_serial(lua), Op::ItemDef, IVec3::ZERO, id)
        })?,
    )?;
    table.raw_set(
        "material_def",
        lua.create_function(|lua, id: String| {
            world_op(lua, current_serial(lua), Op::MaterialDef, IVec3::ZERO, id)
        })?,
    )?;
    let faces = lua.create_sequence_from(Direction::ALL.map(Direction::name))?;
    faces.set_readonly(true);
    table.raw_set("faces", faces)?;
    table.set_readonly(true);
    Ok(table)
}
