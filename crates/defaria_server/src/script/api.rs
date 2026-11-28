//! What the server's hooks are handed besides what every side reads
//! (`defaria_core::script::api`: block handles, the `world` global):
//! changing blocks, blocks' `data`, machines' handles on themselves
//! ([`MachineRef`]), and players and items. Hooks are called with
//! [`ScriptWorld::call`], which sets them up for the call and saves what
//! changed. Documented for modders in `docs/scripting.md`.

mod machine;
mod value;
mod world;

use bevy::prelude::*;
use mlua::{Lua, Table};

pub use machine::{EnergyRef, MachineRef, SlotsRef, TankRef};
pub use value::{MAX_DATA_BYTES, table_from_lua};
pub use world::{ScriptWorld, WorldCall};

use defaria_core::{
    content::{Content, Direction, ItemState},
    game_mode::GameMode,
    identity::PlayerId,
    script::api::BlockRef,
};

/// The handle a hook on the block at `pos` gets: a [`MachineRef`] for a
/// machine, a [`BlockRef`] for anything else.
pub fn handle(lua: &Lua, pos: IVec3, machine: bool) -> mlua::Result<mlua::AnyUserData> {
    if machine {
        MachineRef::create(lua, pos)
    } else {
        BlockRef::create(lua, pos)
    }
}

/// A player, as a hook's `player` argument sees them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerView {
    /// Their id, which `net.send` sends to: their key's hash, the same
    /// every time they play. `None` for a player without one (a test's).
    pub id: Option<PlayerId>,
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

/// `player` as a table: `id` (what `net.send` sends to), `pos` and
/// `look` (vectors), `mode` (`"creative"` or `"survival"`), `sneaking`,
/// and `target` (the block they look at, a handle, or nil) with `face`,
/// the face of it they look at.
pub fn player_table(lua: &Lua, player: &PlayerView) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    // 64 hex digits.
    table.raw_set("id", player.id.map(|id| id.to_string()))?;
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
