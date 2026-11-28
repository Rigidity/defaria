//! Plain values ([`Value`], what block entities keep and what messages
//! between scripts carry) to Lua and back.

use std::collections::BTreeMap;

use mlua::{Lua, Table};

use crate::block_entity::{MAX_DEPTH, Value};

/// `value` as a Lua value: numbers, strings, booleans, and tables (a list
/// is a table counting from 1).
pub fn to_lua(lua: &Lua, value: &Value) -> mlua::Result<mlua::Value> {
    Ok(match value {
        Value::Bool(b) => mlua::Value::Boolean(*b),
        Value::Int(n) => mlua::Value::Number(*n as f64),
        Value::Float(n) => mlua::Value::Number(*n),
        Value::Text(text) => mlua::Value::String(lua.create_string(text)?),
        Value::List(items) => {
            let table = lua.create_table_with_capacity(items.len(), 0)?;
            for item in items {
                table.raw_push(to_lua(lua, item)?)?;
            }
            mlua::Value::Table(table)
        }
        Value::Table(entries) => mlua::Value::Table(table_to_lua(lua, entries)?),
    })
}

/// A table of plain values as a Lua table.
pub fn table_to_lua(lua: &Lua, entries: &BTreeMap<String, Value>) -> mlua::Result<Table> {
    let table = lua.create_table_with_capacity(0, entries.len())?;
    for (key, value) in entries {
        table.raw_set(key.as_str(), to_lua(lua, value)?)?;
    }
    Ok(table)
}

/// A Lua value as a plain value: a number, string or boolean, or a table
/// of them with string keys or as a list, no deeper than [`MAX_DEPTH`].
/// `name` is what errors call it (`data`, `message`).
///
/// The walk counts what it has read as it goes and stops once that
/// passes `max_bytes` (a byte for each value and key, plus their
/// strings, which is never more than any encoding of it takes): a table
/// that refers to another twice is copied twice, so a few Lua steps can
/// make a structure exponentially big here. Callers check the size of
/// what they keep or send of it themselves.
pub fn from_lua(value: mlua::Value, name: &str, max_bytes: usize) -> mlua::Result<Value> {
    let mut size = Size {
        bytes: 0,
        max: max_bytes,
        name,
    };
    from_value(value, 0, name, &mut size)
}

/// At least how many bytes what's been read so far takes: a byte for each
/// value and key, plus their strings.
struct Size<'a> {
    bytes: usize,
    max: usize,
    name: &'a str,
}

impl Size<'_> {
    fn add(&mut self, bytes: usize) -> mlua::Result<()> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > self.max {
            return Err(mlua::Error::runtime(format!(
                "`{}` is more than the {} bytes it may take",
                self.name, self.max
            )));
        }
        Ok(())
    }
}

fn from_value(
    value: mlua::Value,
    depth: usize,
    path: &str,
    size: &mut Size,
) -> mlua::Result<Value> {
    // Each value takes at least its tag byte.
    size.add(1)?;
    Ok(match value {
        mlua::Value::Boolean(b) => Value::Bool(b),
        mlua::Value::Integer(n) => Value::Int(n),
        mlua::Value::Number(n) => {
            // Lua has one kind of number; whole ones come back as ints.
            if n.fract() == 0.0 && n.abs() < (1u64 << 53) as f64 {
                Value::Int(n as i64)
            } else {
                Value::Float(n)
            }
        }
        mlua::Value::String(text) => {
            size.add(text.as_bytes().len())?;
            Value::Text(text.to_str()?.to_string())
        }
        mlua::Value::Table(table) => from_table(&table, depth + 1, path, size)?,
        other => {
            return Err(mlua::Error::runtime(format!(
                "`{path}` is a {}: only numbers, strings, booleans and tables of them can be \
                 kept or sent",
                other.type_name()
            )));
        }
    })
}

/// A table: a list if its keys are 1 to n (or it's empty), a table of
/// plain values if they're all strings.
fn from_table(table: &Table, depth: usize, path: &str, size: &mut Size) -> mlua::Result<Value> {
    if depth > MAX_DEPTH {
        return Err(mlua::Error::runtime(format!(
            "`{path}` nests deeper than {MAX_DEPTH} tables"
        )));
    }
    let length = table.raw_len();
    // Each item takes at least a byte, so a longer list can't fit (and
    // isn't walked).
    if length > size.max {
        size.add(length)?;
    }
    let mut count = 0;
    let mut entries = BTreeMap::new();
    for pair in table.pairs::<mlua::Value, mlua::Value>() {
        let (key, value) = pair?;
        count += 1;
        match key {
            mlua::Value::String(key) => {
                // The key's length (at least a byte) and its text.
                size.add(1 + key.as_bytes().len())?;
                let key = key.to_str()?.to_string();
                let inner = format!("{path}.{key}");
                entries.insert(key, from_value(value, depth, &inner, size)?);
            }
            // A list item, read below; its tag byte is counted then.
            mlua::Value::Integer(_) | mlua::Value::Number(_) if length > 0 => {}
            _ => {
                return Err(mlua::Error::runtime(format!(
                    "`{path}` has a key that isn't a string: tables are lists or have string \
                     keys"
                )));
            }
        }
    }
    if length > 0 {
        if count != length || !entries.is_empty() {
            return Err(mlua::Error::runtime(format!(
                "`{path}` mixes a list with other keys"
            )));
        }
        let items = (1..=length)
            .map(|i| from_value(table.raw_get(i)?, depth, &format!("{path}[{i}]"), size))
            .collect::<mlua::Result<_>>()?;
        return Ok(Value::List(items));
    }
    Ok(Value::Table(entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: usize = 16 << 10;

    fn read(code: &str) -> mlua::Result<Value> {
        let lua = Lua::new();
        let value: mlua::Value = lua.load(code).eval().unwrap();
        from_lua(value, "data", MAX)
    }

    #[test]
    fn plain_values_go_to_lua_and_back() {
        let lua = Lua::new();
        let mut entries = BTreeMap::new();
        entries.insert("count".into(), Value::Int(3));
        entries.insert("ratio".into(), Value::Float(0.5));
        entries.insert("name".into(), Value::Text("lamp".into()));
        entries.insert("on".into(), Value::Bool(true));
        entries.insert(
            "list".into(),
            Value::List(vec![Value::Int(1), Value::Text("two".into())]),
        );
        entries.insert("empty".into(), Value::Table(BTreeMap::new()));
        let table = table_to_lua(&lua, &entries).unwrap();
        assert_eq!(
            from_lua(mlua::Value::Table(table), "data", MAX).unwrap(),
            Value::Table(entries)
        );
    }

    #[test]
    fn values_a_script_makes_are_read_back() {
        let data = read("return { uses = 2, at = { x = 1.25 }, path = { 'a', 'b' } }").unwrap();
        let data = data.as_table().unwrap();
        assert_eq!(data["uses"], Value::Int(2));
        assert_eq!(data["at"].as_table().unwrap()["x"], Value::Float(1.25));
        assert_eq!(
            data["path"],
            Value::List(vec![Value::Text("a".into()), Value::Text("b".into())])
        );
        assert_eq!(read("return 'hi'").unwrap(), Value::Text("hi".into()));
    }

    #[test]
    fn values_that_arent_plain_or_are_too_big_are_refused() {
        for (code, says) in [
            ("return { f = function() end }", "function"),
            ("return { [true] = 1 }", "isn't a string"),
            ("return { 1, 2, x = 3 }", "mixes"),
            (
                "local t = {} local c = t for i = 1, 40 do c.n = {} c = c.n end return t",
                "deeper",
            ),
            ("return { s = string.rep('x', 20000) }", "bytes"),
            (
                "local t = {} for i = 1, 20000 do t[i] = true end return { l = t }",
                "bytes",
            ),
        ] {
            let error = read(code).unwrap_err().to_string();
            assert!(error.contains(says), "{code}: {error}");
        }
    }

    #[test]
    fn sharing_one_table_many_times_is_refused_quickly() {
        // 31 Lua steps, but 2^30 tables if each reference is walked.
        let started = std::time::Instant::now();
        let error =
            read("local t = { x = 1 } for i = 1, 30 do t = { a = t, b = t } end return { d = t }")
                .unwrap_err()
                .to_string();
        assert!(error.contains("bytes"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn sharing_a_table_that_fits_is_copied() {
        let data = read("local p = { x = 1 } return { a = p, b = p }").unwrap();
        let data = data.as_table().unwrap();
        assert_eq!(data["a"], data["b"]);
    }
}
