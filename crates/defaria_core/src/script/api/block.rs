//! [`BlockRef`]: a script's handle on a block, by position.

use bevy::prelude::*;
use mlua::{
    AnyUserData, Lua, MetaMethod, MultiValue, ObjectLike, Table, UserData, UserDataFields, Value,
    Vector,
};

use super::world::{Op, current_serial, world_op};
use crate::content::Direction;

/// A block, as a script sees it: its position, and through the call it
/// was made in, its id, state, stats and data, and changing it.
///
/// - `block.pos`: its position, a vector.
/// - `block.id`, `block.name`: its block's id and its state's name.
/// - `block.state`: its property values, a table (`true`/`false` for
///   those that are `"true"`/`"false"`).
/// - `block.data`: its block entity's data, a table that's saved when the
///   hook returns, or nil for a block with no block entity.
/// - `block.light`: the light in its voxel, `{ sky, red, green, blue }`
///   from 0 to 15, or nil where there's none worked out.
/// - `block:stat(name)`: a stat of its state, or nil.
/// - `block:set_state { prop = value }`: changes some of its properties.
/// - `block:set(id, { prop = value }?)`: makes it another block.
/// - `block:neighbour(face)`: the block next to it on that face, or nil
///   if it isn't loaded.
///
/// Changes return whether the block changed, and are allowed only within
/// one step of the block the hook is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRef {
    pub pos: IVec3,
    serial: u64,
}

impl BlockRef {
    /// A handle on the block at `pos`, for the call running in `lua`.
    pub fn create(lua: &Lua, pos: IVec3) -> mlua::Result<AnyUserData> {
        lua.create_userdata(BlockRef {
            pos,
            serial: current_serial(lua),
        })
    }

    /// A handle on the block at `pos`, or nil if it isn't loaded.
    pub fn loaded(lua: &Lua, pos: IVec3) -> mlua::Result<Value> {
        let serial = current_serial(lua);
        if world_op(lua, serial, Op::Loaded, pos, ())? != Value::Boolean(true) {
            return Ok(Value::Nil);
        }
        Ok(Value::UserData(Self::create(lua, pos)?))
    }
}

/// A position as a vector.
pub fn vector(pos: IVec3) -> Vector {
    Vector::new(pos.x as f32, pos.y as f32, pos.z as f32)
}

/// A position from a vector or three numbers, rounded down to the block.
pub fn position(lua: &Lua, args: MultiValue) -> mlua::Result<IVec3> {
    let args: Vec<Value> = args.into_iter().collect();
    let pos = match args.as_slice() {
        [Value::Vector(v), ..] => Vec3::new(v.x(), v.y(), v.z()),
        [x, y, z, ..] => Vec3::new(
            lua.unpack::<f32>(x.clone())?,
            lua.unpack::<f32>(y.clone())?,
            lua.unpack::<f32>(z.clone())?,
        ),
        _ => {
            return Err(mlua::Error::runtime(
                "a position is a vector, or three numbers",
            ));
        }
    };
    let pos = pos.floor();
    if pos.is_finite() && pos.abs().max_element() <= MAX_COORD as f32 {
        Ok(pos.as_ivec3())
    } else {
        Err(mlua::Error::runtime(format!(
            "there's no block at ({}, {}, {}): positions are at most {MAX_COORD} from the \
             origin on each axis",
            pos.x, pos.y, pos.z
        )))
    }
}

/// The furthest a position a script gives may be from the origin on each
/// axis, 2^30: so far that no world reaches it, and near enough that the
/// difference of two positions never overflows.
const MAX_COORD: i32 = 1 << 30;

/// A face from its name: `up`, `down`, `north`, `south`, `east` or `west`.
pub fn face_named(name: &str) -> mlua::Result<Direction> {
    Direction::ALL
        .into_iter()
        .find(|d| d.name() == name)
        .ok_or_else(|| {
            mlua::Error::runtime(format!(
                "there's no face `{name}`: it's up, down, north, south, east or west"
            ))
        })
}

/// A handle on a block by position, made in some call: a [`BlockRef`], or
/// a machine's handle, which is a block handle and more.
pub trait Located: 'static {
    fn pos(&self) -> IVec3;
    fn serial(&self) -> u64;

    fn op(&self, lua: &Lua, op: Op, args: impl mlua::IntoLuaMulti) -> mlua::Result<Value> {
        world_op(lua, self.serial(), op, self.pos(), args)
    }
}

impl Located for BlockRef {
    fn pos(&self) -> IVec3 {
        self.pos
    }

    fn serial(&self) -> u64 {
        self.serial
    }
}

/// A block handle's fields: `pos`, `id`, `name`, `state`, `data`, `light`
/// and `fluid`.
pub fn add_block_fields<T: Located, F: UserDataFields<T>>(fields: &mut F) {
    fields.add_field_method_get("pos", |_, block| Ok(vector(block.pos())));
    fields.add_field_method_get("id", |lua, block| block.op(lua, Op::Id, ()));
    fields.add_field_method_get("name", |lua, block| block.op(lua, Op::Name, ()));
    fields.add_field_method_get("state", |lua, block| block.op(lua, Op::State, ()));
    fields.add_field_method_get("data", |lua, block| block.op(lua, Op::Data, ()));
    fields.add_field_method_get("light", |lua, block| block.op(lua, Op::Light, ()));
    fields.add_field_method_get("fluid", |lua, block| block.op(lua, Op::Fluid, ()));
}

/// A block handle's methods: `stat`, `set_state`, `set`, `neighbour`, and
/// comparing and printing handles.
pub fn add_block_methods<T: Located, M: mlua::UserDataMethods<T>>(methods: &mut M) {
    methods.add_method("stat", |lua, block, name: String| {
        block.op(lua, Op::Stat, name)
    });
    methods.add_method("set_state", |lua, block, values: Table| {
        block.op(lua, Op::SetState, values)
    });
    methods.add_method(
        "set",
        |lua, block, (id, values): (String, Option<Table>)| block.op(lua, Op::Set, (id, values)),
    );
    methods.add_method("neighbour", |lua, block, face: String| {
        let face = face_named(&face)?;
        BlockRef::loaded(lua, block.pos() + face.normal())
    });
    methods.add_meta_method(MetaMethod::Eq, |_, block, other: AnyUserData| {
        Ok(handle_pos(&other) == Some(block.pos()))
    });
    methods.add_meta_method(MetaMethod::ToString, |_, block, ()| {
        let IVec3 { x, y, z } = block.pos();
        Ok(format!("block at ({x}, {y}, {z})"))
    });
}

/// The position of a block handle, or of any handle that is one and more
/// (a machine's, which has a block handle's `pos`).
pub fn handle_pos(handle: &AnyUserData) -> Option<IVec3> {
    if let Ok(block) = handle.borrow::<BlockRef>() {
        return Some(block.pos);
    }
    let pos: Vector = handle.get("pos").ok()?;
    Some(Vec3::new(pos.x(), pos.y(), pos.z()).as_ivec3())
}

impl UserData for BlockRef {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        add_block_fields(fields);
    }

    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        add_block_methods(methods);
    }
}
