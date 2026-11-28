//! How a hook reaches the world, on either side: block handles
//! ([`BlockRef`]) and the `world` global are plain Lua values that reach
//! it through one function a call puts in the Lua state's app data for as
//! long as it lasts ([`with_world`]), made in the call's scope. So a
//! handle a script keeps past its call stops working: the function is
//! gone, and a handle from an earlier call is refused by its serial.
//!
//! What every side can read is here ([`read`], over any [`WorldView`]);
//! each side's call answers the rest. A client's answers nothing else
//! ([`call_read_only`]): its world is read-only. The server's
//! (`defaria_server::script::api`) changes blocks, keeps blocks' `data`
//! and reaches machines.

use std::sync::atomic::{AtomicU64, Ordering};

use bevy::{ecs::system::SystemParam, prelude::*};
use mlua::{FromLuaMulti, Function, IntoLuaMulti, Lua, MultiValue, Scope, Table, Value};

use super::block::BlockRef;
use crate::{
    content::{BlockState, Content, Direction},
    light::{ChunkLight, Light},
    script::{CallError, Caller, ScriptRuntime},
    world::{Chunk, ChunkCoord, ChunkMap, VoxelFluid},
};

/// The running call's world function, as app data: looked up on every
/// world operation, so it's kept where that's cheapest.
struct CallWorld(Function);

/// Every call's serial, so a handle can tell its call is over.
static SERIAL: AtomicU64 = AtomicU64::new(1);

/// The serial of the call running in a Lua state, as app data.
#[derive(Debug, Clone, Copy)]
struct CallSerial(u64);

/// What a block handle or `world` asks of the call's world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
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
    // A machine's own handle's, in its own hooks, which only the server
    // makes (`defaria_server::script::api::machine`).
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
    pub fn is_machine(self) -> bool {
        self as u8 >= Op::Active as u8
    }

    /// Whether [`read`] answers it: what every side can do.
    pub fn is_read(self) -> bool {
        !self.is_machine() && !matches!(self, Op::SetState | Op::Set | Op::Data)
    }

    /// What a script calls it, for errors.
    fn name(self) -> &'static str {
        match self {
            Op::SetState => "block:set_state",
            Op::Set => "block:set",
            Op::Data => "block.data",
            _ => "a machine's handle",
        }
    }
}

/// Asks the current call's world for `op` at `pos`, for a handle made in
/// call `serial`.
pub fn world_op(
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
    // `with_world` turns the number back into the `Op` at that index of
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
pub fn current_serial(lua: &Lua) -> u64 {
    lua.app_data_ref::<CallSerial>()
        .map_or(0, |serial| serial.0)
}

/// Runs `body` (which calls the hook) with the world reachable through
/// `dispatch`, which answers each [`Op`] a handle or `world` asks for, at
/// a position, with the script's arguments. Handles made in the call
/// work only in it. Call it in [`ScriptRuntime::call_scoped`]'s closure,
/// with the scope that gets.
pub fn with_world<'scope, 'env, R>(
    lua: &Lua,
    scope: &'scope Scope<'scope, 'env>,
    dispatch: impl Fn(&Lua, Op, IVec3, MultiValue) -> mlua::Result<Value> + 'env,
    body: impl FnOnce() -> mlua::Result<R>,
) -> mlua::Result<R> {
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    lua.set_app_data(CallSerial(serial));
    let function = scope.create_function(
        move |lua, (from, op, x, y, z, args): (u64, u8, i32, i32, i32, MultiValue)| {
            if from != serial {
                return Err(mlua::Error::runtime(
                    "a block handle kept from an earlier call: handles last only as long as \
                     the hook they were given to",
                ));
            }
            let op = *Op::ALL
                .get(op as usize)
                .ok_or_else(|| mlua::Error::runtime("no such world operation"))?;
            dispatch(lua, op, IVec3::new(x, y, z), args)
        },
    )?;
    lua.set_app_data(CallWorld(function));
    let result = body();
    lua.remove_app_data::<CallWorld>();
    result
}

/// What every side's scripts can read of the world: the content, and the
/// loaded voxels' blocks, fluid and light.
pub trait WorldView {
    fn content(&self) -> &Content;
    /// The block at `pos`, if it's loaded.
    fn block(&self, pos: IVec3) -> Option<BlockState>;
    /// The fluid in the voxel at `pos`, if it's loaded and has some.
    fn fluid(&self, pos: IVec3) -> Option<VoxelFluid>;
    /// The light at `pos`, if it's loaded.
    fn light(&self, pos: IVec3) -> Option<Light>;
}

/// The loaded chunks, to read: what a client's scripts see of its copy
/// of the world (the server's see theirs through `Voxels`, which also
/// writes).
#[derive(SystemParam)]
pub struct ChunkReader<'w, 's> {
    pub content: Res<'w, Content>,
    map: Res<'w, ChunkMap>,
    chunks: Query<'w, 's, (&'static Chunk, Option<&'static ChunkLight>)>,
}

impl ChunkReader<'_, '_> {
    fn at(&self, pos: IVec3) -> Option<(&Chunk, Option<&ChunkLight>, IVec3)> {
        let (coord, local) = ChunkCoord::split(pos);
        let (chunk, light) = self.chunks.get(self.map.get(coord)?).ok()?;
        Some((chunk, light, local))
    }
}

impl WorldView for ChunkReader<'_, '_> {
    fn content(&self) -> &Content {
        &self.content
    }

    fn block(&self, pos: IVec3) -> Option<BlockState> {
        self.at(pos).map(|(chunk, _, local)| chunk.at(local))
    }

    fn fluid(&self, pos: IVec3) -> Option<VoxelFluid> {
        self.at(pos)
            .and_then(|(chunk, _, local)| chunk.fluid_at(local))
    }

    fn light(&self, pos: IVec3) -> Option<Light> {
        let (_, light, local) = self.at(pos)?;
        light.map(|light| light.at(local))
    }
}

/// Answers `op`, which must be one [`Op::is_read`] says is, from `view`.
pub fn read(
    lua: &Lua,
    view: &impl WorldView,
    op: Op,
    pos: IVec3,
    args: MultiValue,
) -> mlua::Result<Value> {
    debug_assert!(op.is_read(), "{op:?} isn't a read");
    let mut args = args.into_iter();
    let mut arg = move || args.next().unwrap_or(Value::Nil);
    let content = view.content();
    if let Op::BlockDef | Op::ItemDef | Op::MaterialDef = op {
        let id: String = lua.unpack(arg())?;
        return match op {
            Op::BlockDef => block_def(lua, content, &id),
            Op::ItemDef => item_def(lua, content, &id),
            _ => material_def(lua, content, &id),
        };
    }
    let Some(state) = view.block(pos) else {
        return Ok(match op {
            Op::Loaded => Value::Boolean(false),
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
        Op::Light => match view.light(pos) {
            Some(light) => {
                let table = lua.create_table_with_capacity(0, 4)?;
                for (channel, name) in ["sky", "red", "green", "blue"].into_iter().enumerate() {
                    table.raw_set(name, light.get(channel))?;
                }
                Value::Table(table)
            }
            None => Value::Nil,
        },
        Op::Fluid => match view.fluid(pos) {
            Some(fluid) => {
                let table = lua.create_table_with_capacity(0, 2)?;
                table.raw_set("id", content.block_of(fluid.kind).id.as_str())?;
                table.raw_set("amount", fluid.amount)?;
                Value::Table(table)
            }
            None => Value::Nil,
        },
        _ => unreachable!("{op:?} isn't a read"),
    })
}

/// Calls `hook` of script `script` for `caller` with the world in `view`
/// to read and nothing to change: a client's hooks. A block's `data` is
/// nil (a client isn't sent it), and changing a block is an error that
/// fails the hook, as any script error does. `args` makes the hook's
/// arguments.
pub fn call_read_only<A, R>(
    runtime: &mut ScriptRuntime,
    caller: Caller,
    script: &str,
    hook: &str,
    view: &impl WorldView,
    args: impl FnOnce(&Lua) -> mlua::Result<A>,
) -> Result<Option<R>, CallError>
where
    A: IntoLuaMulti,
    R: FromLuaMulti,
{
    if !runtime.has_hook(script, hook) {
        // Checked first, so a call that won't run sets nothing up.
        return runtime.call_with(caller, script, hook, ());
    }
    runtime.call_scoped(caller, script, hook, |lua, scope, function| {
        let dispatch = |lua: &Lua, op: Op, pos: IVec3, args: MultiValue| {
            if op.is_read() {
                read(lua, view, op, pos, args)
            } else if op == Op::Data {
                Ok(Value::Nil)
            } else {
                Err(mlua::Error::runtime(format!(
                    "the world is read-only on a client: `{}` can't change it here (ask the \
                     server's scripts with `net.send`)",
                    op.name()
                )))
            }
        };
        with_world(lua, scope, dispatch, || function.call::<R>(args(lua)?))
    })
}

/// A state's property values as a table: `"true"` and `"false"` as
/// booleans, everything else as text.
pub fn state_table(lua: &Lua, values: &[(String, String)]) -> mlua::Result<Table> {
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
