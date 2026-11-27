//! How a plain [`Value`] is written in a saved chunk, and read back:
//! a tag byte, then what the tag says follows. See
//! `docs/world-format.md`.

use std::collections::BTreeMap;

use defaria_core::block_entity::Value;

use crate::save::chunk::{Reader, write_varint};

/// How deep lists and tables can nest in a saved value. Deeper is an error
/// when reading, so damaged data can't overflow the stack.
pub const MAX_DEPTH: usize = 32;

const BOOL_FALSE: u8 = 0;
const BOOL_TRUE: u8 = 1;
const INT: u8 = 2;
const FLOAT: u8 = 3;
const TEXT: u8 = 4;
const LIST: u8 = 5;
const TABLE: u8 = 6;

/// Appends `value`: a tag byte, then what the tag says follows. See
/// `docs/world-format.md`.
pub fn write_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Bool(false) => out.push(BOOL_FALSE),
        Value::Bool(true) => out.push(BOOL_TRUE),
        Value::Int(n) => {
            out.push(INT);
            // Zigzag, so small negative numbers are short too.
            write_varint(out, ((n << 1) ^ (n >> 63)) as u64);
        }
        Value::Float(f) => {
            out.push(FLOAT);
            out.extend_from_slice(&f.to_le_bytes());
        }
        Value::Text(text) => {
            out.push(TEXT);
            write_text(out, text);
        }
        Value::List(list) => {
            out.push(LIST);
            write_varint(out, list.len() as u64);
            for item in list {
                write_value(out, item);
            }
        }
        Value::Table(table) => {
            out.push(TABLE);
            write_varint(out, table.len() as u64);
            for (key, item) in table {
                write_text(out, key);
                write_value(out, item);
            }
        }
    }
}

/// Appends a string: its length in bytes, then its UTF-8.
pub fn write_text(out: &mut Vec<u8>, text: &str) {
    write_varint(out, text.len() as u64);
    out.extend_from_slice(text.as_bytes());
}

/// Reads a string written by [`write_text`].
pub fn read_text(reader: &mut Reader) -> Result<String, String> {
    let len = reader.length(usize::MAX)?;
    let bytes = reader.bytes(len)?;
    String::from_utf8(bytes.to_vec()).map_err(|_| "a string isn't UTF-8".into())
}

/// Reads a value written by [`write_value`].
pub fn read_value(reader: &mut Reader) -> Result<Value, String> {
    read_nested(reader, 0)
}

fn read_nested(reader: &mut Reader, depth: usize) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err("a value nests too deep".into());
    }
    let tag = reader.bytes(1)?[0];
    Ok(match tag {
        BOOL_FALSE => Value::Bool(false),
        BOOL_TRUE => Value::Bool(true),
        INT => {
            let zigzag = reader.varint()?;
            Value::Int(((zigzag >> 1) as i64) ^ -((zigzag & 1) as i64))
        }
        FLOAT => {
            let bytes: [u8; 8] = reader.bytes(8)?.try_into().expect("eight bytes");
            Value::Float(f64::from_le_bytes(bytes))
        }
        TEXT => Value::Text(read_text(reader)?),
        LIST => {
            // Each item takes a byte at least, so the length can't be
            // more than what's left; checked as it reads.
            let len = reader.length(usize::MAX)?;
            let mut list = Vec::new();
            for _ in 0..len {
                list.push(read_nested(reader, depth + 1)?);
            }
            Value::List(list)
        }
        TABLE => {
            let len = reader.length(usize::MAX)?;
            let mut table = BTreeMap::new();
            for _ in 0..len {
                let key = read_text(reader)?;
                table.insert(key, read_nested(reader, depth + 1)?);
            }
            Value::Table(table)
        }
        _ => return Err(format!("{tag} isn't a kind of value")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(value: &Value) -> Value {
        let mut out = Vec::new();
        write_value(&mut out, value);
        let mut reader = Reader::new(&out);
        let back = read_value(&mut reader).unwrap();
        assert!(reader.is_done());
        back
    }

    #[test]
    fn values_round_trip() {
        let table = Value::Table(BTreeMap::from([
            ("on".into(), Value::Bool(true)),
            ("off".into(), Value::Bool(false)),
            ("small".into(), Value::Int(-3)),
            ("big".into(), Value::Int(i64::MIN)),
            ("max".into(), Value::Int(i64::MAX)),
            ("half".into(), Value::Float(0.5)),
            ("nan".into(), Value::Float(f64::NAN)),
            ("name".into(), Value::Text("ünïcode".into())),
            (
                "list".into(),
                Value::List(vec![Value::Int(1), Value::List(vec![])]),
            ),
        ]));
        assert_eq!(round_trip(&table), table);
    }

    #[test]
    fn small_ints_are_short() {
        let mut out = Vec::new();
        write_value(&mut out, &Value::Int(-1));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn bad_values_are_errors() {
        let read = |bytes: &[u8]| read_value(&mut Reader::new(bytes));
        assert!(read(&[]).is_err());
        assert!(read(&[99]).is_err(), "unknown tag");
        assert!(read(&[FLOAT, 0, 0]).is_err(), "truncated");
        assert!(read(&[LIST, 200, 1]).is_err(), "too long for what's left");
        assert!(read(&[TEXT, 2, 0xff, 0xfe]).is_err(), "not UTF-8");
        let deep = [LIST, 1].repeat(MAX_DEPTH + 2);
        assert!(read(&deep).is_err(), "too deep");
    }
}
