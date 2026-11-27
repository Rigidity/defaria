//! Filling `{property}` in references and names, and the other names the
//! bake makes up.

use crate::content::{Content, NONE, Property};

/// Fills `{property}` in `text` with whatever `value` gives for it.
pub(super) fn fill_with(
    text: &str,
    mut value: impl FnMut(&str) -> Option<String>,
) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('}')
            .ok_or_else(|| format!("`{text}` has a `{{` with no `}}`"))?;
        let name = &rest[start + 1..start + end];
        let filled = value(name)
            .ok_or_else(|| format!("`{text}` uses `{{{name}}}`, which isn't a property"))?;
        out.push_str(&filled);
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Fills `{property}` in a reference with the state's value of that
/// property: for a material, its id.
pub(super) fn fill(text: &str, values: &[(String, String)]) -> Result<String, String> {
    fill_with(text, |name| {
        values
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    })
}

/// Fills `{property}` in a name: a material's name, nothing for `none`, or
/// the plain value. Spaces left doubled or at the ends by an empty value
/// are taken out.
pub(super) fn fill_name(
    text: &str,
    properties: &[Property],
    index: &[usize],
    content: &Content,
) -> Result<String, String> {
    let name = fill_with(text, |name| {
        let p = properties.iter().position(|prop| prop.name == name)?;
        let value = &properties[p].values[index[p]];
        Some(if !properties[p].materials {
            value.clone()
        } else if value == NONE {
            String::new()
        } else {
            content.materials[value].name.clone()
        })
    })?;
    Ok(name.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// The last part of an id (after `:` or `/`), in title case with spaces
/// for underscores: `defaria:hot_oil` is "Hot oil".
pub fn title_case(id: &str) -> String {
    let name = id.rsplit(['/', ':']).next().unwrap_or(id).replace('_', " ");
    let mut chars = name.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Names the state an error is about, if the block has properties.
pub(super) fn in_state(values: &[(String, String)], error: String) -> String {
    if values.is_empty() {
        return error;
    }
    let state: Vec<String> = values.iter().map(|(n, v)| format!("{n}={v}")).collect();
    format!("state {}: {error}", state.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::bake::test_util::*;

    #[test]
    fn fill_replaces_property_names() {
        let values = [("color".to_string(), "red".to_string())];
        assert_eq!(fill("wool_{color}", &values).unwrap(), "wool_red");
        assert_eq!(fill("plain", &values).unwrap(), "plain");
        assert!(fill("wool_{colour}", &values).is_err());
        assert!(fill("wool_{color", &values).is_err());
    }

    #[test]
    fn names_skip_empty_values() {
        let content = materials(&[(
            "blocks/x.json5",
            r##"{ name: "{ore} Rock", properties: { ore: { materials: "#ore", optional: true } },
                 model: "quad", textures: { all: "a" } }"##,
        )]);
        let block = content.block("test:x").unwrap();
        assert_eq!(content.state(block.default_state()).name, "Rock");
    }

    #[test]
    fn names_default_to_the_id_in_title_case() {
        assert_eq!(title_case("defaria:grass_top"), "Grass top");
        assert_eq!(title_case("defaria:ores/iron_ore"), "Iron ore");
    }
}
