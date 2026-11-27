//! Sorting the definition files into their kinds.

use serde::de::DeserializeOwned;

use super::{Def, Defs};
use crate::content::check_name;

/// The kinds of definition, each in its own folder.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Categories,
    Networks,
    Materials,
    Textures,
    Models,
    Blocks,
    Items,
    Recipes,
    Reactions,
}

impl Kind {
    const ALL: [Kind; 9] = [
        Kind::Categories,
        Kind::Networks,
        Kind::Materials,
        Kind::Textures,
        Kind::Models,
        Kind::Blocks,
        Kind::Items,
        Kind::Recipes,
        Kind::Reactions,
    ];

    fn folder(self) -> &'static str {
        match self {
            Kind::Categories => "categories",
            Kind::Networks => "networks",
            Kind::Materials => "materials",
            Kind::Textures => "textures",
            Kind::Models => "models",
            Kind::Blocks => "blocks",
            Kind::Items => "items",
            Kind::Recipes => "recipes",
            Kind::Reactions => "reactions",
        }
    }
}

/// Parses one file into its kind's list, with its id.
pub(super) fn parse_file(
    namespace: &str,
    dir: &str,
    path: &str,
    text: &str,
    defs: &mut Defs,
) -> Result<(), String> {
    let (folder, rest) = path.split_once('/').unwrap_or(("", path));
    let kind = Kind::ALL.into_iter().find(|kind| kind.folder() == folder);
    let (Some(kind), Some(name)) = (kind, rest.strip_suffix(".json5")) else {
        let folders: Vec<&str> = Kind::ALL.iter().map(|kind| kind.folder()).collect();
        return Err(format!(
            "not a definition: files go in {}, named <name>.json5",
            folders.join(", ")
        ));
    };
    // Not `qualify`: a `:` in a file name mustn't reach another mod's ids.
    check_name(name)?;
    let id = format!("{namespace}:{name}");
    fn def<T: DeserializeOwned>(
        id: String,
        namespace: &str,
        file: String,
        text: &str,
    ) -> Result<Def<T>, String> {
        let def = json5::from_str(text).map_err(|error| error.to_string())?;
        Ok(Def {
            id,
            namespace: namespace.into(),
            file,
            def,
        })
    }
    let file = format!("{dir}/{path}");
    match kind {
        Kind::Categories => defs.categories.push(def(id, namespace, file, text)?),
        Kind::Networks => defs.networks.push(def(id, namespace, file, text)?),
        Kind::Materials => defs.materials.push(def(id, namespace, file, text)?),
        Kind::Textures => defs.textures.push(def(id, namespace, file, text)?),
        Kind::Models => defs.models.push(def(id, namespace, file, text)?),
        Kind::Blocks => defs.blocks.push(def(id, namespace, file, text)?),
        Kind::Items => defs.items.push(def(id, namespace, file, text)?),
        Kind::Recipes => defs.recipes.push(def(id, namespace, file, text)?),
        Kind::Reactions => defs.reactions.push(def(id, namespace, file, text)?),
    }
    Ok(())
}
