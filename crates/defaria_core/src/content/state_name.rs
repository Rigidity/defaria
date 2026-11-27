//! State names: an id with the values of some properties, as text
//! (`defaria:log[axis=x,wood=defaria:birch]`). Saves name block and item
//! states this way, templated recipes their materials, and scenario
//! scripts the blocks and items they use. This is the one writer and the
//! one reader of that text.

use std::fmt;

/// An id and property values, written `id` or `id[property=value,…]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateName {
    pub id: String,
    /// In the order they're written.
    pub values: Vec<(String, String)>,
}

impl StateName {
    pub fn new<K: Into<String>, V: Into<String>>(
        id: impl Into<String>,
        values: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        Self {
            id: id.into(),
            values: values
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }

    /// Reads `id` or `id[property=value,…]`. Empty pairs are skipped
    /// (`id[a=b,]`); a `[` with no `]` at the end, or a pair with no `=`,
    /// is an error, which names the text at fault.
    pub fn parse(text: &str) -> Result<Self, String> {
        let Some((id, rest)) = text.split_once('[') else {
            return Ok(Self::new(text, Vec::<(String, String)>::new()));
        };
        let rest = rest
            .strip_suffix(']')
            .ok_or_else(|| format!("`{text}` has a `[` with no `]`"))?;
        let values = rest
            .split(',')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                pair.split_once('=')
                    .ok_or_else(|| format!("`{pair}` isn't `property=value`"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(id, values))
    }

    /// The id `text` names, whether or not the rest of it parses: what's
    /// before any `[`.
    pub fn id_of(text: &str) -> &str {
        text.split_once('[').map_or(text, |(id, _)| id)
    }

    /// The values, borrowed, as `Block::state` and `Item::state` take them.
    pub fn pairs(&self) -> Vec<(&str, &str)> {
        self.values
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }
}

impl fmt::Display for StateName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.id)?;
        if self.values.is_empty() {
            return Ok(());
        }
        f.write_str("[")?;
        for (i, (k, v)) in self.values.iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            write!(f, "{k}={v}")?;
        }
        f.write_str("]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(id: &str, values: &[(&str, &str)]) -> StateName {
        StateName::new(id, values.iter().copied())
    }

    #[test]
    fn names_round_trip() {
        for state in [
            name("defaria:lamp", &[]),
            name("defaria:log", &[("axis", "x"), ("wood", "defaria:birch")]),
            name(
                "defaria:soil",
                &[("soil", "defaria:sand"), ("turf", "none")],
            ),
        ] {
            let text = state.to_string();
            assert_eq!(StateName::parse(&text), Ok(state));
        }
    }

    #[test]
    fn names_are_written_as_ids_with_bracketed_values() {
        assert_eq!(name("defaria:lamp", &[]).to_string(), "defaria:lamp");
        assert_eq!(
            name("defaria:log", &[("axis", "x"), ("wood", "defaria:birch")]).to_string(),
            "defaria:log[axis=x,wood=defaria:birch]"
        );
    }

    #[test]
    fn empty_brackets_and_pairs_are_skipped() {
        assert_eq!(StateName::parse("a[]"), Ok(name("a", &[])));
        assert_eq!(
            StateName::parse("a[x=1,,y=2,]"),
            Ok(name("a", &[("x", "1"), ("y", "2")]))
        );
    }

    #[test]
    fn malformed_names_are_errors() {
        assert_eq!(
            StateName::parse("a[x=1"),
            Err("`a[x=1` has a `[` with no `]`".into())
        );
        assert_eq!(
            StateName::parse("a[x=1]b"),
            Err("`a[x=1]b` has a `[` with no `]`".into())
        );
        assert_eq!(
            StateName::parse("a[x=1,y]"),
            Err("`y` isn't `property=value`".into())
        );
    }

    #[test]
    fn the_id_is_found_in_any_name() {
        assert_eq!(StateName::id_of("a"), "a");
        assert_eq!(StateName::id_of("a[x=1]"), "a");
        assert_eq!(StateName::id_of("a[x"), "a");
    }
}
