//! What scripts are handed: block handles ([`BlockRef`]), the `world`
//! global, players and items, and blocks' `data`. Hooks are called with
//! [`ScriptWorld::call`], which sets them up for the call and saves what
//! changed. Documented for modders in `docs/scripting.md`.

mod block;
mod value;
mod world;

use bevy::prelude::*;
use mlua::{Lua, Table};

pub use block::{BlockRef, face_named, vector};
pub use value::{MAX_DATA_BYTES, table_from_lua, table_to_lua, to_lua};
pub use world::ScriptWorld;
pub(crate) use world::world_table;

use crate::{
    content::{Content, Direction, ItemState},
    game_mode::GameMode,
};

/// A player, as a hook's `player` argument sees them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerView {
    /// Their eyes.
    pub pos: Vec3,
    /// The way they look.
    pub look: Vec3,
    pub mode: GameMode,
    pub sneaking: bool,
    /// The block under their crosshair and the face of it they look at,
    /// if any.
    pub target: Option<(IVec3, Option<Direction>)>,
}

/// `player` as a table: `pos` and `look` (vectors), `mode` (`"creative"`
/// or `"survival"`), `sneaking`, and `target` (the block they look at, a
/// handle, or nil) with `face`, the face of it they look at.
pub fn player_table(lua: &Lua, player: &PlayerView) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    let v = |v: Vec3| mlua::Vector::new(v.x, v.y, v.z);
    table.raw_set("pos", v(player.pos))?;
    table.raw_set("look", v(player.look))?;
    let mode = if player.mode.is_creative() {
        "creative"
    } else {
        "survival"
    };
    table.raw_set("mode", mode)?;
    table.raw_set("sneaking", player.sneaking)?;
    if let Some((pos, face)) = player.target {
        table.raw_set("target", BlockRef::create(lua, pos)?)?;
        table.raw_set("face", face.map(Direction::name))?;
    }
    Ok(table)
}

/// An item as a table: `id`, `name` and `state` (its property values).
pub fn item_table(lua: &Lua, content: &Content, item: ItemState) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    if let (Some(def), Some(info)) = (content.item_of(item), content.item_state(item)) {
        table.raw_set("id", def.id.as_str())?;
        table.raw_set("name", info.name.as_str())?;
        let state = lua.create_table()?;
        for (name, value) in &info.values {
            state.raw_set(name.as_str(), value.as_str())?;
        }
        table.raw_set("state", state)?;
    }
    Ok(table)
}

/// Block states' property values as a table, for hooks' arguments.
pub(super) use world::state_table;
