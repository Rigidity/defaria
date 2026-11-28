//! A block's `data` from Lua: the plain values (`defaria_core`'s
//! `script::api::from_lua`) a block entity keeps, no bigger saved than a
//! block may keep.

use std::collections::BTreeMap;

use mlua::Table;

use defaria_core::{block_entity::Value, script::api::from_lua};

use crate::block_entity::write_value;

/// The most a block's data may take saved, in bytes. A hook that leaves
/// more fails, and its changes to the data are dropped.
pub const MAX_DATA_BYTES: usize = 16 << 10;

/// A Lua table as a table of plain values, for a block's data: string
/// keys, and values that are plain (no functions or userdata), no deeper
/// than `MAX_DEPTH` and no bigger saved than [`MAX_DATA_BYTES`].
pub fn table_from_lua(table: &Table) -> mlua::Result<BTreeMap<String, Value>> {
    let Value::Table(entries) =
        from_lua(mlua::Value::Table(table.clone()), "data", MAX_DATA_BYTES)?
    else {
        return Err(mlua::Error::runtime(
            "`data` must be a table with string keys, not a list",
        ));
    };
    let mut saved = Vec::new();
    write_value(&mut saved, &Value::Table(entries.clone()));
    if saved.len() > MAX_DATA_BYTES {
        return Err(mlua::Error::runtime(format!(
            "`data` is {} bytes saved, more than the {MAX_DATA_BYTES} a block may keep",
            saved.len()
        )));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use mlua::Lua;

    use super::*;

    #[test]
    fn data_is_a_table_that_fits_saved() {
        let lua = Lua::new();
        let table: Table = lua.load("return { uses = 2 }").eval().unwrap();
        assert_eq!(table_from_lua(&table).unwrap()["uses"], Value::Int(2));
        let list: Table = lua.load("return { 1, 2 }").eval().unwrap();
        let error = table_from_lua(&list).unwrap_err().to_string();
        assert!(error.contains("not a list"), "{error}");
        let big: Table = lua
            .load("return { s = string.rep('x', 20000) }")
            .eval()
            .unwrap();
        let error = table_from_lua(&big).unwrap_err().to_string();
        assert!(error.contains("bytes"), "{error}");
    }
}
