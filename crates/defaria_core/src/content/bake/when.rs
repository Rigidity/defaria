//! `when` patterns: which states a variant, part or recipe applies to.

use std::collections::BTreeMap;

use crate::content::{Content, NONE, Property, qualify, schema::Condition};

/// A variant's `when`, worked out: for each property it names, which of
/// its values match.
pub(in crate::content) struct When(Vec<(usize, Vec<bool>)>);

/// Each of a list of `when`s worked out, an error naming the one at fault
/// by `label` and its place in the list ("variant 2: ...").
pub(in crate::content) fn all_whens<'d>(
    label: &str,
    list: impl Iterator<Item = &'d BTreeMap<String, Condition>>,
    properties: &[Property],
    namespace: &str,
    content: &Content,
) -> Result<Vec<When>, String> {
    list.enumerate()
        .map(|(i, when)| {
            When::new(when, properties, namespace, content)
                .map_err(|error| format!("{label} {}: {error}", i + 1))
        })
        .collect()
}

impl When {
    pub(in crate::content) fn new(
        when: &BTreeMap<String, Condition>,
        properties: &[Property],
        namespace: &str,
        content: &Content,
    ) -> Result<Self, String> {
        let mut out = Vec::new();
        for (name, condition) in when {
            let p = properties
                .iter()
                .position(|prop| &prop.name == name)
                .ok_or_else(|| format!("no property `{name}`"))?;
            let property = &properties[p];
            let (patterns, negate) = match condition {
                Condition::Is(values) => (values, false),
                Condition::Not(not) => (&not.not, true),
            };
            if patterns.values().is_empty() {
                return Err(format!("`{name}` lists no values"));
            }
            let mut matches = vec![false; property.values.len()];
            for pattern in patterns.values() {
                let hits = pattern_matches(property, pattern, namespace, content)?;
                if !hits.iter().any(|&hit| hit) {
                    return Err(format!("`{name}` has no value `{pattern}`"));
                }
                for (m, hit) in matches.iter_mut().zip(hits) {
                    *m |= hit;
                }
            }
            if negate {
                matches.iter_mut().for_each(|m| *m = !*m);
            }
            out.push((p, matches));
        }
        Ok(Self(out))
    }

    pub(in crate::content) fn matches(&self, index: &[usize]) -> bool {
        self.0.iter().all(|(p, values)| values[index[*p]])
    }
}

/// Which of a property's values `pattern` matches: a value, or for a
/// material property, `none`, a material or `#tag`.
pub(in crate::content) fn pattern_matches(
    property: &Property,
    pattern: &str,
    namespace: &str,
    content: &Content,
) -> Result<Vec<bool>, String> {
    if !property.materials || pattern == NONE {
        return Ok(property.values.iter().map(|v| v == pattern).collect());
    }
    if let Some(tag) = pattern.strip_prefix('#') {
        return Ok(property
            .values
            .iter()
            .map(|v| content.materials.get(v).is_some_and(|m| m.has_tag(tag)))
            .collect());
    }
    let id = qualify(pattern, namespace)?;
    Ok(property.values.iter().map(|v| *v == id).collect())
}

#[cfg(test)]
mod tests {
    use crate::content::bake::test_util::*;

    /// `when` can name materials directly (in this mod or not), list them,
    /// and mix them with tags under `not`.
    #[test]
    fn conditions_name_materials_by_id() {
        let content = materials(&[(
            "blocks/x.json5",
            r##"{
                properties: { ore: { materials: "#ore", optional: true } },
                model: "quad",
                textures: { all: "a" },
                variants: [
                    { when: { ore: "coal" }, name: "Coal" },
                    { when: { ore: ["test:iron", "none"] }, light: [1, 1, 1] },
                    { when: { ore: { not: ["coal", "#metal"] } }, name: "Plain" },
                ],
            }"##,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let block = content.block("test:x").unwrap();
        let state = |ore| content.state(block.state(&[("ore", ore)]).unwrap());
        assert_eq!(state("test:coal").name, "Coal");
        assert!(!state("test:coal").emits_light());
        assert!(state("test:iron").emits_light() && state("none").emits_light());
        assert_eq!(state("none").name, "Plain");
        assert_eq!(state("test:iron").name, "X");
    }

    /// An optional material adds `none` first; `when` matches it, `#tags`
    /// and `not`; and plain properties aren't part of the item.
    #[test]
    fn optional_materials_and_conditions_pick_variants() {
        let content = materials(&[(
            "blocks/stone.json5",
            r##"{
                name: "Stone",
                properties: {
                    ore: { materials: "#ore", optional: true },
                    rock: { materials: "#rock" },
                    lit: ["no", "yes"],
                },
                model: "quad",
                textures: { all: "a" },
                variants: [
                    {
                        when: { ore: { not: "none" } },
                        name: "{ore} Ore",
                        textures: { all: { texture: "ore", materials: { rock: "{rock}", ore: "{ore}" } } },
                    },
                    { when: { ore: "#metal" }, light: [5, 5, 5] },
                ],
            }"##,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let stone = content.block("test:stone").unwrap();
        assert_eq!(
            stone.properties[1].values,
            ["none", "test:coal", "test:iron"]
        );
        let state = |ore| content.state(stone.state(&[("ore", ore)]).unwrap());
        let (plain, coal, iron) = (state("none"), state("test:coal"), state("test:iron"));
        assert_eq!(plain.name, "Stone");
        assert_eq!(plain.faces[0].layer, content.texture("test:a").unwrap());
        assert_eq!(coal.name, "Coal Ore");
        assert_eq!(
            content.textures[coal.faces[0].layer as usize].id,
            "test:ore(ore=test:coal,rock=test:rock)"
        );
        assert!(!coal.emits_light() && iron.emits_light());
        // `lit` isn't part of the item: 3 ores × 1 rock.
        let item = content.item("test:stone").unwrap();
        assert_eq!(item.state_count, 3);
        let names: Vec<&str> = content
            .item_states
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, ["Stone", "Coal Ore", "Iron Ore"]);
    }
}
