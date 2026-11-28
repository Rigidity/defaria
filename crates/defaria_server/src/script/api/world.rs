//! [`ScriptWorld`]: what a server hook can reach of the world, and
//! calling a hook with it. What every side reads is
//! `defaria_core::script::api`'s ([`read`]); the server's call adds
//! changing blocks near the hook's own, blocks' `data`, and machines'
//! parts (`machine.rs`).

use std::cell::RefCell;

use bevy::{ecs::system::SystemParam, prelude::*};
use mlua::{FromLuaMulti, Function, IntoLuaMulti, Lua, MultiValue, Scope, Table, Value};

use super::value::table_from_lua;
use crate::{
    block_entity::{BlockData, BlockEntities},
    machine::Ports,
    network::{Networks, energy::EnergyReport},
};
use defaria_core::{
    content::{BlockState, Content},
    light::{ChunkLight, Light},
    machine::{EnergyBuffer, MachineActive, Slots, Tanks},
    script::{
        CallError, Caller, ScriptRuntime,
        api::{Op, WorldView, read, table_to_lua, with_world},
    },
    world::{ChunkCoord, ChunkMap, VoxelFluid, Voxels},
};

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
/// [`VoxelChanged`](defaria_core::world::VoxelChanged) or
/// [`PropertiesUpdated`](defaria_core::world::PropertiesUpdated)), the content,
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
        let content = self.0.borrow().content;
        let dispatch = move |lua: &Lua, op: Op, pos: IVec3, args: MultiValue| {
            self.0.borrow_mut().dispatch(lua, op, pos, args)
        };
        with_world(lua, scope, dispatch, || {
            args(lua, content)
                .and_then(|args| hook.call::<R>(args))
                .and_then(|result| self.0.borrow_mut().save_data().map(|()| result))
        })
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
    /// The `data` tables handed out, to save when the hook returns.
    tables: Vec<(IVec3, Table)>,
    /// What the origin's machine may still make in this call.
    pub(super) generate: u64,
}

impl Call<'_, '_, '_> {
    fn dispatch(&mut self, lua: &Lua, op: Op, pos: IVec3, args: MultiValue) -> mlua::Result<Value> {
        if op.is_machine() {
            return self.machine_op(lua, op, pos, args);
        }
        if op.is_read() {
            return read(lua, self, op, pos, args);
        }
        let mut args = args.into_iter();
        let mut arg = move || args.next().unwrap_or(Value::Nil);
        let content = self.content;
        let Some(state) = self.voxels.get(pos) else {
            return Ok(match op {
                Op::SetState | Op::Set => Value::Boolean(false),
                _ => Value::Nil,
            });
        };
        let info = content.state(state);
        Ok(match op {
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
            _ => unreachable!("{op:?} is a read or a machine's"),
        })
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

impl WorldView for Call<'_, '_, '_> {
    fn content(&self) -> &Content {
        self.content
    }

    fn block(&self, pos: IVec3) -> Option<BlockState> {
        self.voxels.get(pos)
    }

    fn fluid(&self, pos: IVec3) -> Option<VoxelFluid> {
        self.voxels.get_with_fluid(pos).and_then(|(_, fluid)| fluid)
    }

    fn light(&self, pos: IVec3) -> Option<Light> {
        let (coord, local) = ChunkCoord::split(pos);
        let chunk = self.map.get(coord)?;
        self.lights.get(chunk).ok().map(|light| light.at(local))
    }
}
