//! Plain data: what a block entity's parts are saved as, and what a mod
//! can give a block entity to start with (`block_entity: { data: ... }`).
//!
//! Numbers, strings, booleans, and lists and tables of them: enough for
//! anything a feature or a script wants to keep, and self-describing, so a
//! saved value can be read without knowing what wrote it. How a value is
//! written in a saved chunk is the server's
//! (`defaria_server::block_entity::value`).

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::Deserialize;

/// A plain value. In mod files it's written as JSON5: `true`, `3`, `0.5`,
/// `"text"`, `[...]` or `{ key: ... }`.
#[derive(Reflect, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    List(Vec<Value>),
    Table(BTreeMap<String, Value>),
}

/// Floats compare by their bits, so a value always equals itself (NaN
/// included) and a saved value equals what it was saved from.
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a.to_bits() == b.to_bits(),
            (Self::Text(a), Self::Text(b)) => a == b,
            (Self::List(a), Self::List(b)) => a == b,
            (Self::Table(a), Self::Table(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Self::Table(table) => Some(table),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_from_json5() {
        let value: Value = json5::from_str(r#"{ a: 1, b: 0.5, c: "x", d: [true] }"#).unwrap();
        assert_eq!(
            value,
            Value::Table(BTreeMap::from([
                ("a".into(), Value::Int(1)),
                ("b".into(), Value::Float(0.5)),
                ("c".into(), Value::Text("x".into())),
                ("d".into(), Value::List(vec![Value::Bool(true)])),
            ]))
        );
    }
}
