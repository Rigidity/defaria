//! Item categories.

use super::Def;
use crate::content::{Category, Content, qualify, schema::CategoryDef};

/// Adds the categories, in their order: lowest `order` first, then by id.
pub(super) fn add_categories(defs: &[Def<CategoryDef>], content: &mut Content) {
    let mut categories: Vec<Category> = defs
        .iter()
        .map(|def| Category {
            id: def.id.clone(),
            name: def.def.name.clone(),
            order: def.def.order,
        })
        .collect();
    categories.sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
    content.category_index = categories
        .iter()
        .enumerate()
        .map(|(index, category)| (category.id.clone(), index))
        .collect();
    content.categories = categories;
}

/// The category `reference` (from a file in `namespace`) names.
pub(super) fn category(
    reference: Option<&String>,
    namespace: &str,
    content: &Content,
) -> Result<Option<usize>, String> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    let id = qualify(reference, namespace)?;
    content
        .category_index
        .get(&id)
        .copied()
        .map(Some)
        .ok_or_else(|| format!("no category `{id}`"))
}

#[cfg(test)]
mod tests {
    use crate::content::{bake::test_util::*, test_mod as content};

    #[test]
    fn items_are_listed_under_their_categories_in_order() {
        let content = content(&[
            ("categories/tools.json5", r#"{ name: "Tools", order: 20 }"#),
            (
                "categories/natural.json5",
                r#"{ name: "Natural", order: 10 }"#,
            ),
            (
                "categories/a_later.json5",
                r#"{ name: "Later", order: 20 }"#,
            ),
            (
                "blocks/dirt.json5",
                r#"{ model: "quad", textures: { all: "a" }, category: "natural" }"#,
            ),
            // The item file moves the block's item to another category.
            (
                "blocks/rock.json5",
                r#"{ model: "quad", textures: { all: "a" }, category: "natural" }"#,
            ),
            ("items/rock.json5", r#"{ category: "tools" }"#),
            ("items/pick.json5", r#"{ texture: "a", category: "tools" }"#),
            ("items/stick.json5", r#"{ texture: "a" }"#),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let names: Vec<&str> = content.categories.iter().map(|c| c.name.as_str()).collect();
        // By order, then by id.
        assert_eq!(names, ["Natural", "Later", "Tools"]);
        let category = |id: &str| {
            let item = content.default_item(id).unwrap();
            content.item_category(item).map(|c| c.name.as_str())
        };
        assert_eq!(category("test:dirt"), Some("Natural"));
        assert_eq!(category("test:rock"), Some("Tools"));
        assert_eq!(category("test:pick"), Some("Tools"));
        assert_eq!(category("test:stick"), None);
    }

    #[test]
    fn an_unknown_category_is_an_error() {
        let content = content(&[("items/pick.json5", r#"{ texture: "a", category: "nope" }"#)]);
        assert!(
            errors(&content).contains("no category `test:nope`"),
            "{}",
            errors(&content)
        );
        assert!(content.item("test:pick").is_none());
    }
}
