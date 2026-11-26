//! Searching items: what's typed in a screen's search box, parsed into an
//! [`ItemQuery`], and whether an item matches it. Item lists show only
//! what matches their filter.
//!
//! A query is words separated by spaces, and an item matches when every
//! word does:
//!
//! - A plain word matches items whose name contains it: `iron` finds Iron
//!   Pickaxe and Iron Ore.
//! - `category:<name>` matches items in a category whose name or id starts
//!   with it: `category:tool`.
//! - `material:<name>` matches items made of a material (any of their
//!   material properties) whose name or id starts with it, or that has a
//!   tag starting with it: `material:oak`, `material:metal`.
//!
//! Case doesn't matter. A word with some other `key:` is taken as plain
//! text, colon and all.

use crate::content::{Content, ItemState};

/// A parsed search: every part has to match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemQuery {
    /// Words the name must contain, lowercase.
    pub words: Vec<String>,
    /// Starts of category names or ids, lowercase.
    pub categories: Vec<String>,
    /// Starts of material names, ids or tags, lowercase.
    pub materials: Vec<String>,
}

impl ItemQuery {
    pub fn parse(text: &str) -> Self {
        let mut query = Self::default();
        for word in text.split_whitespace() {
            let word = word.to_lowercase();
            match word.split_once(':') {
                Some(("category", value)) if !value.is_empty() => {
                    query.categories.push(value.to_owned());
                }
                Some(("material", value)) if !value.is_empty() => {
                    query.materials.push(value.to_owned());
                }
                _ => query.words.push(word),
            }
        }
        query
    }

    /// Whether it matches everything: nothing typed.
    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.categories.is_empty() && self.materials.is_empty()
    }

    pub fn matches(&self, content: &Content, item: ItemState) -> bool {
        let Some(info) = content.item_state(item) else {
            return false;
        };
        let name = info.name.to_lowercase();
        if !self.words.iter().all(|word| name.contains(word.as_str())) {
            return false;
        }
        let category = content.item_category(item);
        let in_category = |wanted: &String| {
            category.is_some_and(|category| {
                category.name.to_lowercase().starts_with(wanted.as_str())
                    || local_name(&category.id).starts_with(wanted.as_str())
            })
        };
        if !self.categories.iter().all(in_category) {
            return false;
        }
        let made_of = |wanted: &String| {
            info.values.iter().any(|(_, value)| {
                content.materials.get(value).is_some_and(|material| {
                    material.name.to_lowercase().starts_with(wanted.as_str())
                        || local_name(&material.id).starts_with(wanted.as_str())
                        || material
                            .tags
                            .iter()
                            .any(|tag| tag.starts_with(wanted.as_str()))
                })
            })
        };
        self.materials.iter().all(made_of)
    }
}

/// An id's part after its mod: `defaria:oak` is `oak`.
fn local_name(id: &str) -> String {
    id.split_once(':')
        .map_or(id, |(_, name)| name)
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::base;

    #[test]
    fn queries_split_into_words_and_filters() {
        assert_eq!(
            ItemQuery::parse("  Iron category:Tools  pick material:oak x:y category: "),
            ItemQuery {
                words: vec![
                    "iron".into(),
                    "pick".into(),
                    "x:y".into(),
                    "category:".into()
                ],
                categories: vec!["tools".into()],
                materials: vec!["oak".into()],
            }
        );
        assert!(ItemQuery::parse("   ").is_empty());
    }

    fn names(query: &str) -> Vec<String> {
        let content = base();
        let query = ItemQuery::parse(query);
        (0..content.item_states.len() as u32)
            .map(ItemState)
            .filter(|&item| query.matches(content, item))
            .map(|item| content.item_name(item).unwrap().to_owned())
            .collect()
    }

    #[test]
    fn words_match_names() {
        let found = names("lamp");
        assert!(!found.is_empty());
        assert!(
            found
                .iter()
                .all(|name| name.to_lowercase().contains("lamp"))
        );
        assert!(names("STONE slab").contains(&"Stone Slab".to_owned()));
        assert!(names("no such thing").is_empty());
        // Nothing typed: everything.
        assert_eq!(names("").len(), base().item_states.len());
    }

    #[test]
    fn filters_match_categories_and_materials() {
        let content = base();
        let tools = names("category:tool");
        assert!(tools.iter().any(|name| name.ends_with("Pickaxe")));
        let in_tools = |name: &String| {
            let item = (0..content.item_states.len() as u32)
                .map(ItemState)
                .find(|&item| content.item_name(item) == Some(name))
                .unwrap();
            content.item_category(item).unwrap().name == "Tools"
        };
        assert!(tools.iter().all(in_tools));
        // Birch is a material of logs and planks, and tool handles.
        let birch = names("material:birch");
        assert!(birch.contains(&"Birch Log".to_owned()), "{birch:?}");
        assert!(birch.iter().all(|name| !name.contains("Stone")));
        // Both filters, and a word.
        let birch_tools = names("category:tools material:birch axe");
        assert!(!birch_tools.is_empty());
        // "axe" is in Pickaxe too.
        assert!(
            birch_tools
                .iter()
                .all(|name| name.to_lowercase().contains("axe"))
        );
        assert!(names("category:nope").is_empty());
    }
}
