//! Parses the definition files and resolves the references between them,
//! in dependency order: categories, network kinds, materials, textures,
//! models, blocks, items.
//!
//! Each definition that fails is reported once, naming its file, and left
//! out; definitions referring to it then fail with "no such …".
//!
//! Textures with `$inputs` are templates: each combination of materials a
//! block state or item uses is baked into its own tile, once, when the
//! first user asks for it. Textures with `#tag` inputs are baked for every
//! combination of tagged materials as they load.

use std::collections::{BTreeMap, BTreeSet};

use bevy::{platform::collections::HashMap, prelude::*};
use serde::de::DeserializeOwned;

use super::{
    AIR, BakedFace, Block, BlockBox, BlockState, Category, Conduit, Connections, Content,
    Direction, FULL_FLUID, Fluid, Item, ItemStateInfo, MAX_LIGHT, Material, ModInfo, Model,
    ModelFace, NONE, NetworkKind, Placement, Property, Rule, State, StatePart, Texture,
    TextureLayer, Vertex, connect::side_properties, index_of, load::LoadedMod, qualify, schema::*,
    state_count,
};
use crate::{block_texture::Palette, fluid::flow::MIN_RIPPLE};

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
}

impl Kind {
    const ALL: [Kind; 7] = [
        Kind::Categories,
        Kind::Networks,
        Kind::Materials,
        Kind::Textures,
        Kind::Models,
        Kind::Blocks,
        Kind::Items,
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
        }
    }
}
/// Most states one block or item may have.
const MAX_STATES: usize = 4096;

/// A parsed definition and where it came from.
struct Def<T> {
    id: String,
    /// The mod's namespace, for its references.
    namespace: String,
    /// The file's asset path, for errors.
    file: String,
    def: T,
}

#[derive(Default)]
struct Defs {
    categories: Vec<Def<CategoryDef>>,
    networks: Vec<Def<NetworkDef>>,
    materials: Vec<Def<MaterialDef>>,
    textures: Vec<Def<TextureDef>>,
    models: Vec<Def<ModelDef>>,
    blocks: Vec<Def<BlockDef>>,
    items: Vec<Def<ItemDef>>,
}

pub fn bake(mods: Vec<LoadedMod>, errors: &mut Vec<String>) -> Content {
    let mut defs = Defs::default();
    // Every image, by asset path, and each mod's folder by namespace.
    let mut images = BTreeSet::new();
    let mut dirs = HashMap::new();
    for loaded in &mods {
        let dir = &loaded.files.dir;
        dirs.insert(loaded.manifest.id.clone(), dir.clone());
        images.extend(
            loaded
                .files
                .images
                .iter()
                .map(|path| format!("{dir}/{path}")),
        );
        for (path, text) in &loaded.files.files {
            if let Err(error) = parse_file(&loaded.manifest.id, dir, path, text, &mut defs) {
                errors.push(format!("{dir}/{path}: {error}"));
            }
        }
    }

    let mut content = Content {
        mods: mods
            .into_iter()
            .map(|loaded| ModInfo {
                id: loaded.manifest.id,
                name: loaded.manifest.name,
                version: loaded.manifest.version,
                description: loaded.manifest.description,
                authors: loaded.manifest.authors,
                dir: loaded.files.dir,
            })
            .collect(),
        materials: BTreeMap::new(),
        textures: Vec::new(),
        models: BTreeMap::new(),
        blocks: vec![Block {
            id: AIR.into(),
            properties: Vec::new(),
            first_state: 0,
            state_count: 1,
            connections: None,
        }],
        states: vec![State {
            name: "Air".into(),
            ..default()
        }],
        items: Vec::new(),
        item_states: Vec::new(),
        categories: Vec::new(),
        networks: Vec::new(),
        errors: Vec::new(),
        category_index: HashMap::new(),
        network_index: HashMap::new(),
        texture_index: HashMap::new(),
        block_index: HashMap::new(),
        item_index: HashMap::new(),
    };
    content.block_index.insert(AIR.into(), 0);

    let mut report = |file: &str, result: Result<(), String>| {
        if let Err(error) = result {
            errors.push(format!("{file}: {error}"));
        }
    };
    add_categories(&defs.categories, &mut content);
    add_networks(&defs.networks, &mut content);
    for def in &defs.materials {
        let result = material(def).map(|material| {
            content.materials.insert(def.id.clone(), material);
        });
        report(&def.file, result);
    }
    let mut textures = Textures {
        images,
        dirs,
        ..default()
    };
    for def in &defs.textures {
        match template(def, &content, &textures) {
            Ok((template, problems)) => {
                for problem in problems {
                    report(&def.file, Err(problem));
                }
                // Textures without inputs, and those with `#tag` inputs in
                // every combination, are baked now, so every one is checked
                // and has a tile, used or not.
                textures.templates.insert(def.id.clone(), template);
                match textures.bake_all(&def.id, &mut content) {
                    Ok(missing) => {
                        for image in missing {
                            report(
                                &def.file,
                                Err(format!(
                                    "there's no {image}; the tiles that use it show a placeholder"
                                )),
                            );
                        }
                    }
                    Err(error) => {
                        textures.templates.remove(&def.id);
                        report(&def.file, Err(error));
                    }
                }
            }
            Err(error) => report(&def.file, Err(error)),
        }
    }
    for def in &defs.models {
        let result = model(def).map(|model| {
            content.models.insert(def.id.clone(), model);
        });
        report(&def.file, result);
    }
    for def in &defs.blocks {
        report(&def.file, add_block(def, &mut content, &textures));
    }
    add_items(&defs, &mut content, &textures, &mut report);
    content
}

/// Adds the categories, in their order: lowest `order` first, then by id.
fn add_categories(defs: &[Def<CategoryDef>], content: &mut Content) {
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

/// Adds the network kinds, in load order.
fn add_networks(defs: &[Def<NetworkDef>], content: &mut Content) {
    for def in defs {
        content
            .network_index
            .insert(def.id.clone(), content.networks.len());
        content.networks.push(NetworkKind {
            id: def.id.clone(),
            name: def.def.name.clone(),
            solver: def.def.solver,
        });
    }
}

/// The category `reference` (from a file in `namespace`) names.
fn category(
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

/// Parses one file into its kind's list, with its id.
fn parse_file(
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
    super::check_name(name)?;
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
    }
    Ok(())
}

/// The box every fluid is drawn as (`fluid_box.json5`), built into the
/// engine rather than taken from a mod.
fn fluid_box() -> &'static Model {
    static BOX: std::sync::LazyLock<Model> = std::sync::LazyLock::new(|| {
        let def: ModelDef =
            json5::from_str(include_str!("fluid_box.json5")).expect("the fluid box is valid JSON5");
        model(&Def {
            id: "fluid box".into(),
            namespace: String::new(),
            file: "src/content/fluid_box.json5".into(),
            def,
        })
        .expect("the fluid box is a valid model")
    });
    &BOX
}

/// A palette from 2 to 8 hex colors, darkest first.
fn palette(colors: &[String]) -> Result<Palette, String> {
    if !(2..=8).contains(&colors.len()) {
        return Err(format!(
            "a palette needs 2 to 8 colors, not {}",
            colors.len()
        ));
    }
    let stops = colors
        .iter()
        .map(|color| Srgba::hex(color).map_err(|_| format!("`{color}` isn't a hex color")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Palette::ramp(&stops))
}

fn material(def: &Def<MaterialDef>) -> Result<Material, String> {
    let material = &def.def;
    for tag in &material.tags {
        super::check_name(tag).map_err(|error| format!("tag {error}"))?;
    }
    let palettes = material
        .palettes
        .iter()
        .map(|(role, colors)| {
            palette(colors)
                .map(|palette| (role.clone(), palette))
                .map_err(|error| format!("palette `{role}`: {error}"))
        })
        .collect::<Result<_, String>>()?;
    check_amount("hardness", material.hardness)?;
    check_amount("weight", material.weight)?;
    for (name, &value) in &material.stats {
        check_stat_name(name)?;
        check_amount(name, value)?;
    }
    Ok(Material {
        id: def.id.clone(),
        name: material.name.clone().unwrap_or_else(|| title_case(&def.id)),
        tags: material.tags.clone(),
        palette: palette(&material.palette)?,
        palettes,
        hardness: material.hardness,
        weight: material.weight,
        stats: material.stats.clone(),
    })
}

/// Checks a hardness, weight, stat or share: a number, not below 0.
fn check_amount(what: &str, value: f32) -> Result<(), String> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(format!(
            "`{what}` must be a number of at least 0, not {value}"
        ))
    }
}

/// The numbers each state of a block or item has: its hardness, weight and
/// `stats`.
#[derive(Debug, Clone, Default)]
struct Numbers {
    hardness: f32,
    weight: f32,
    stats: BTreeMap<String, f32>,
}

/// A block's or item's hardness, weight and `stats`, checked against its
/// properties, to work out per state. Hardness and weight are `None` where
/// the file leaves them out.
struct Attributes {
    hardness: Option<Mix>,
    weight: Option<Mix>,
    stats: Vec<(String, Mix)>,
}

impl Attributes {
    /// `block` is the block's properties when these are its item's.
    fn new(
        hardness: Option<&Attribute>,
        weight: Option<&Attribute>,
        stats: &BTreeMap<String, Attribute>,
        properties: &[Property],
        block: Option<&[Property]>,
        content: &Content,
    ) -> Result<Self, String> {
        let mix = |field: &str, stat: &str, def: &Attribute| {
            Mix::new(field, stat, def, properties, block, content)
        };
        Ok(Attributes {
            hardness: hardness
                .map(|def| mix("hardness", "hardness", def))
                .transpose()?,
            weight: weight.map(|def| mix("weight", "weight", def)).transpose()?,
            stats: stats
                .iter()
                .map(|(name, def)| {
                    check_stat_name(name)?;
                    Ok((name.clone(), mix(&format!("stats.{name}"), name, def)?))
                })
                .collect::<Result<_, String>>()?,
        })
    }

    /// The numbers of the state with these value indices: `fallback`'s
    /// hardness and weight where the file leaves one out, and its stats
    /// under the file's own.
    fn values(
        &self,
        fallback: &Numbers,
        properties: &[Property],
        index: &[usize],
        content: &Content,
    ) -> Numbers {
        let value = |mix: &Option<Mix>, fallback| {
            mix.as_ref()
                .map_or(fallback, |mix| mix.value(properties, index, content))
        };
        let mut stats = fallback.stats.clone();
        for (name, mix) in &self.stats {
            stats.insert(name.clone(), mix.value(properties, index, content));
        }
        Numbers {
            hardness: value(&self.hardness, fallback.hardness),
            weight: value(&self.weight, fallback.weight),
            stats,
        }
    }
}

/// Checks a stat's name: lowercase letters, digits and `_`, and not one a
/// material has as a field of its own.
fn check_stat_name(name: &str) -> Result<(), String> {
    super::check_namespace(name).map_err(|error| format!("stat {error}"))?;
    if super::BUILT_IN_STATS.contains(&name) {
        return Err(format!(
            "`{name}` is a field of its own, not one of the `stats`"
        ));
    }
    Ok(())
}

/// An attribute, checked against a block's or item's properties: a number,
/// or a base times a weighted average of one stat of material properties'
/// materials. Any number a block or item works out per state from its
/// materials uses this, so each field reads stats the same way.
struct Mix {
    base: f32,
    /// The materials' stat it averages.
    stat: String,
    /// The value for a material without the stat.
    default: Option<f32>,
    /// Property index and how much it counts.
    shares: Vec<(usize, f32)>,
}

impl Mix {
    /// `field` names the attribute, for errors, and `stat` is the stat it
    /// averages unless `def` names one. `block` is the block's properties
    /// when `properties` are its item's.
    ///
    /// Every state must have a material in at least one counted property:
    /// `base` alone is a multiplier (a volume, a fraction of a block), not
    /// a value of its own. Every material a counted property can be must
    /// have the stat, unless the attribute gives a `default`.
    fn new(
        field: &str,
        stat: &str,
        def: &Attribute,
        properties: &[Property],
        block: Option<&[Property]>,
        content: &Content,
    ) -> Result<Self, String> {
        let mixed = match def {
            Attribute::Fixed(value) => {
                check_amount(field, *value)?;
                return Ok(Mix {
                    base: *value,
                    stat: stat.into(),
                    default: None,
                    shares: Vec::new(),
                });
            }
            Attribute::Mixed(mixed) => mixed,
        };
        check_amount(field, mixed.base)?;
        let stat = mixed.stat.as_deref().unwrap_or(stat);
        let known = super::BUILT_IN_STATS.contains(&stat)
            || content
                .materials
                .values()
                .any(|m| m.stats.contains_key(stat));
        if !known {
            return Err(format!(
                "`{field}` averages stat `{stat}`, which no material has"
            ));
        }
        if let Some(default) = mixed.default {
            check_amount(&format!("{field}` `default"), default)?;
        }
        if mixed.materials.is_empty() {
            return Err(format!(
                "`{field}` has no `materials`; for a fixed value, write a number"
            ));
        }
        let mut shares = Vec::new();
        for (property, &share) in &mixed.materials {
            let p = properties
                .iter()
                .position(|prop| &prop.name == property)
                .ok_or_else(|| {
                    let item_false =
                        block.is_some_and(|block| block.iter().any(|p| &p.name == property));
                    if item_false {
                        format!(
                            "`{field}` counts `{property}`, which isn't one of the item's \
                             properties: the block marks it `item: false`"
                        )
                    } else {
                        format!("`{field}` counts `{property}`, which isn't a property")
                    }
                })?;
            if !properties[p].materials {
                return Err(format!(
                    "`{field}` counts `{property}`, which isn't a material property"
                ));
            }
            if !(share.is_finite() && share > 0.0) {
                return Err(format!(
                    "`{field}` counts `{property}` {share}; a share must be more than 0"
                ));
            }
            if mixed.default.is_none() {
                let lacking = properties[p]
                    .values
                    .iter()
                    .filter(|v| *v != NONE)
                    .find(|v| content.materials[*v].stat(stat).is_none());
                if let Some(material) = lacking {
                    return Err(format!(
                        "`{field}` averages stat `{stat}` of `{property}`, but material \
                         `{material}` has no `{stat}`; give it one, or give `{field}` a \
                         `default`"
                    ));
                }
            }
            shares.push((p, share));
        }
        // States are every combination, so one has them all at `none`
        // exactly when each can be.
        if shares
            .iter()
            .all(|&(p, _)| properties[p].values.iter().any(|v| v == NONE))
        {
            let counted: Vec<String> = shares
                .iter()
                .map(|&(p, _)| format!("{}=none", properties[p].name))
                .collect();
            return Err(format!(
                "`{field}`: in states with {}, none of its counted properties has a \
                 material; count a property that isn't optional, or write a number",
                counted.join(",")
            ));
        }
        Ok(Mix {
            base: mixed.base,
            stat: stat.into(),
            default: mixed.default,
            shares,
        })
    }

    /// The value for the state with these value indices: a fixed value's
    /// base, or the base times the shares' weighted average of their
    /// materials' stat, skipping properties at `none` (`new` checked that
    /// not every one is, and that each material has the stat or there's a
    /// default).
    fn value(&self, properties: &[Property], index: &[usize], content: &Content) -> f32 {
        if self.shares.is_empty() {
            // A fixed value.
            return self.base;
        }
        let (mut sum, mut total) = (0.0, 0.0);
        for &(p, share) in &self.shares {
            let value = &properties[p].values[index[p]];
            if value == NONE {
                continue;
            }
            let stat = content.materials[value].stat(&self.stat).or(self.default);
            sum += share * stat.unwrap_or_default();
            total += share;
        }
        debug_assert!(total > 0.0, "every counted property is `none`");
        self.base * sum / total
    }
}

/// A texture file, checked, with its inputs still to fill.
struct Template {
    layers: Vec<LayerTemplate>,
    /// The inputs its layers use, and for a `#tag` input the tag its
    /// materials need.
    inputs: BTreeMap<String, Option<String>>,
}

impl Template {
    /// Whether every input is a `#tag`, so the texture is baked for every
    /// combination of tagged materials at load.
    fn permutes(&self) -> bool {
        !self.inputs.is_empty() && self.inputs.values().all(Option::is_some)
    }
}

struct LayerTemplate {
    image: LayerImage,
    color: LayerColor,
}

enum LayerImage {
    /// The image's asset path, relative to `assets/`.
    Path(String),
    /// An image name with `{input}`s in it, filled with the name of each
    /// input's material (its id without the mod). Without a mod of its
    /// own, it's in the mod of the first input's material.
    Pattern(String),
}

enum LayerColor {
    Palette(Palette),
    Material { id: String, role: Option<String> },
    Input { name: String, role: Option<String> },
}

/// A texture file as a template, and problems that don't stop it loading:
/// an image file that isn't there is reported, and the texture shows the
/// placeholder tile (see `block_texture::build`) until it is. Images named
/// by their inputs' materials are checked when the texture is baked.
fn template(
    def: &Def<TextureDef>,
    content: &Content,
    textures: &Textures,
) -> Result<(Template, Vec<String>), String> {
    if def.def.layers.is_empty() {
        return Err("a texture needs at least one layer".into());
    }
    let mut problems = Vec::new();
    let mut layers = Vec::new();
    let mut inputs = BTreeMap::new();
    for (i, layer) in def.def.layers.iter().enumerate() {
        let layer = layer_template(layer, &def.namespace, content, &textures.dirs, &mut inputs)
            .map_err(|error| format!("layer {}: {error}", i + 1))?;
        if let LayerImage::Path(path) = &layer.image
            && !textures.images.contains(path)
        {
            problems.push(format!(
                "layer {}: there's no {path}; the texture shows a placeholder",
                i + 1,
            ));
        }
        layers.push(layer);
    }
    let tagged = inputs.values().filter(|tag| tag.is_some()).count();
    if tagged > 0 && tagged < inputs.len() {
        return Err("a texture's inputs are all `$name` or all `#tag`, not both".into());
    }
    for (i, layer) in layers.iter().enumerate() {
        if let LayerImage::Pattern(pattern) = &layer.image {
            let mut untagged = None;
            fill_with(pattern, |name| {
                if !inputs.get(name).is_some_and(Option::is_some) {
                    untagged.get_or_insert(name.to_owned());
                }
                Some(String::new())
            })
            .map_err(|error| format!("layer {}: {error}", i + 1))?;
            if let Some(name) = untagged {
                return Err(format!(
                    "layer {}: image `{pattern}` uses `{{{name}}}`, which isn't a `#tag` input",
                    i + 1
                ));
            }
        }
    }
    Ok((Template { layers, inputs }, problems))
}

fn layer_template(
    layer: &LayerDef,
    namespace: &str,
    content: &Content,
    dirs: &HashMap<String, String>,
    inputs: &mut BTreeMap<String, Option<String>>,
) -> Result<LayerTemplate, String> {
    let image = if layer.image.contains('{') {
        LayerImage::Pattern(layer.image.clone())
    } else {
        LayerImage::Path(image_path(&qualify(&layer.image, namespace)?, dirs)?)
    };
    let role = layer.role.clone();
    let mut input = |name: &str, tag: Option<String>| {
        super::check_name(name).map_err(|error| format!("input {error}"))?;
        if inputs.get(name).is_some_and(|t| *t != tag) {
            return Err(format!("`${name}` and `#{name}` can't both be inputs"));
        }
        inputs.insert(name.to_owned(), tag);
        Ok(LayerColor::Input {
            name: name.into(),
            role: role.clone(),
        })
    };
    let color = match (&layer.material, &layer.palette) {
        (Some(material), None) => {
            if let Some(name) = material.strip_prefix('$') {
                input(name, None)?
            } else if let Some(tag) = material.strip_prefix('#') {
                input(tag, Some(tag.to_owned()))?
            } else {
                let id = qualify(material, namespace)?;
                if !content.materials.contains_key(&id) {
                    return Err(format!("no material `{id}`"));
                }
                LayerColor::Material { id, role }
            }
        }
        (None, Some(colors)) if role.is_none() => LayerColor::Palette(palette(colors)?),
        (None, Some(_)) => return Err("`role` is for layers with a `material`".into()),
        _ => return Err("give a layer either a `material` or a `palette`".into()),
    };
    Ok(LayerTemplate { image, color })
}

/// The asset path of image `id` (`mod:name`).
fn image_path(id: &str, dirs: &HashMap<String, String>) -> Result<String, String> {
    let (image_mod, name) = id
        .split_once(':')
        .ok_or_else(|| format!("image `{id}` has no mod"))?;
    let dir = dirs
        .get(image_mod)
        .ok_or_else(|| format!("image `{id}`: no mod `{image_mod}` is loaded"))?;
    Ok(format!("{dir}/images/{name}.png"))
}

/// The most tiles the game has room for: every tile is a layer of one
/// texture array, and 2048 layers is what GPUs commonly allow.
pub const MAX_TILES: usize = 2048;

/// The texture templates, and the tiles baked from them so far (in
/// [`Content::textures`]).
#[derive(Default)]
struct Textures {
    templates: HashMap<String, Template>,
    /// Every image, by asset path.
    images: BTreeSet<String>,
    /// Each mod's folder, by namespace.
    dirs: HashMap<String, String>,
}

impl Textures {
    /// Bakes texture `id` for every combination its inputs can take: none
    /// for a texture without inputs, every combination of tagged materials
    /// for one whose inputs are `#tag`s. Every tile is made or none is.
    /// Images that are missing (so their tiles show the placeholder) come
    /// back with the result, each once.
    fn bake_all(&self, id: &str, content: &mut Content) -> Result<BTreeSet<String>, String> {
        let template = &self.templates[id];
        if !template.inputs.is_empty() && !template.permutes() {
            return Ok(BTreeSet::new());
        }
        let choices: Vec<(&String, Vec<&String>)> = template
            .inputs
            .iter()
            .map(|(name, tag)| {
                let tag = tag.as_deref().unwrap_or_default();
                let materials = content
                    .materials
                    .iter()
                    .filter(|(_, material)| material.has_tag(tag))
                    .map(|(id, _)| id)
                    .collect();
                (name, materials)
            })
            .collect();
        let room = MAX_TILES.saturating_sub(content.textures.len());
        let count = choices
            .iter()
            .try_fold(1usize, |n, (_, m)| n.checked_mul(m.len()))
            .filter(|&count| count <= room)
            .ok_or_else(|| {
                format!(
                    "its combinations make more tiles than there's room for: {room} more of the game's {MAX_TILES}"
                )
            })?;
        // Each combination, the last input changing fastest.
        let combinations: Vec<BTreeMap<String, String>> = (0..count)
            .map(|n| {
                let mut rest = n;
                let mut materials = BTreeMap::new();
                for (name, options) in choices.iter().rev() {
                    materials.insert((*name).clone(), options[rest % options.len()].clone());
                    rest /= options.len();
                }
                materials
            })
            .collect();
        // Check every combination's images before making any tile. Fixed
        // images were checked with the template.
        let mut missing = BTreeSet::new();
        for materials in &combinations {
            for layer in &template.layers {
                if matches!(layer.image, LayerImage::Path(_)) {
                    continue;
                }
                let image = self.image(&layer.image, materials)?;
                if !self.images.contains(&image) {
                    missing.insert(image);
                }
            }
        }
        for materials in &combinations {
            self.bake(id, materials, content)?;
        }
        Ok(missing)
    }

    /// The layer of texture `id` with these materials for its inputs,
    /// baking it if nothing has used that combination yet.
    fn bake(
        &self,
        id: &str,
        materials: &BTreeMap<String, String>,
        content: &mut Content,
    ) -> Result<u32, String> {
        let key = texture_key(id, materials);
        if let Some(layer) = content.texture(&key) {
            return Ok(layer);
        }
        let template = self
            .templates
            .get(id)
            .ok_or_else(|| format!("no texture `{id}`"))?;
        if let Some((input, tag)) = template
            .inputs
            .iter()
            .find(|(i, _)| !materials.contains_key(*i))
        {
            let sigil = if tag.is_some() { '#' } else { '$' };
            return Err(format!(
                "texture `{id}` needs a material for `{sigil}{input}`"
            ));
        }
        for (input, material) in materials {
            let tag = template
                .inputs
                .get(input)
                .ok_or_else(|| format!("texture `{id}` has no input `{input}`"))?;
            let found = content
                .materials
                .get(material)
                .ok_or_else(|| format!("no material `{material}`"))?;
            if let Some(tag) = tag
                && !found.has_tag(tag)
            {
                return Err(format!(
                    "texture `{id}` input `{input}` takes `#{tag}` materials, and `{material}` isn't one"
                ));
            }
        }
        if content.textures.len() >= MAX_TILES {
            return Err(format!("there's no room for more than {MAX_TILES} tiles"));
        }
        let layers = template
            .layers
            .iter()
            .map(|layer| {
                let image = self.image(&layer.image, materials)?;
                let (material, role) = match &layer.color {
                    LayerColor::Palette(palette) => {
                        return Ok(TextureLayer {
                            image,
                            palette: *palette,
                        });
                    }
                    LayerColor::Material { id, role } => (id, role),
                    LayerColor::Input { name, role } => (&materials[name], role),
                };
                let material = content
                    .materials
                    .get(material)
                    .ok_or_else(|| format!("no material `{material}`"))?;
                Ok(TextureLayer {
                    image,
                    palette: material.palette_for(role.as_deref()),
                })
            })
            .collect::<Result<_, String>>()?;
        let layer = content.textures.len() as u32;
        content.textures.push(Texture {
            id: key.clone(),
            template: id.into(),
            layers,
            cutout: false,
        });
        content.texture_index.insert(key, layer);
        Ok(layer)
    }

    /// The asset path of a layer's image with these materials. A pattern
    /// is filled with the materials' names and, unless it names a mod, is
    /// in the mod of the first one.
    fn image(
        &self,
        image: &LayerImage,
        materials: &BTreeMap<String, String>,
    ) -> Result<String, String> {
        let pattern = match image {
            LayerImage::Path(path) => return Ok(path.clone()),
            LayerImage::Pattern(pattern) => pattern,
        };
        let mut first_mod = None;
        let name = fill_with(pattern, |input| {
            let (material_mod, name) = materials.get(input)?.split_once(':')?;
            first_mod.get_or_insert(material_mod);
            Some(name.to_owned())
        })?;
        // The template checked that every `{}` names an input.
        let namespace = first_mod.ok_or_else(|| format!("image `{pattern}` names no input"))?;
        image_path(&qualify(&name, namespace)?, &self.dirs)
    }

    /// Bakes a reference from a block or item, with `{property}` filled
    /// in from `values`.
    fn bake_ref(
        &self,
        texture: &TextureRef,
        values: &[(String, String)],
        namespace: &str,
        content: &mut Content,
    ) -> Result<u32, String> {
        let (id, materials) = match texture {
            TextureRef::Id(id) => (id, None),
            TextureRef::Bound(bound) => (&bound.texture, Some(&bound.materials)),
        };
        let id = qualify(&fill(id, values)?, namespace)?;
        let materials = materials
            .into_iter()
            .flatten()
            .map(|(input, material)| {
                let material = qualify(&fill(material, values)?, namespace)?;
                Ok((input.clone(), material))
            })
            .collect::<Result<_, String>>()?;
        self.bake(&id, &materials, content)
    }
}

/// A baked texture's id: the texture's, with its inputs' materials, as in
/// `defaria:log_side(bark=defaria:birch)`.
fn texture_key(id: &str, materials: &BTreeMap<String, String>) -> String {
    if materials.is_empty() {
        return id.into();
    }
    let inputs: Vec<String> = materials.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("{id}({})", inputs.join(","))
}

fn model(def: &Def<ModelDef>) -> Result<Model, String> {
    let model = &def.def;
    for (slot, fallback) in &model.slots {
        if let Some(fallback) = fallback
            && !model.slots.contains_key(fallback)
        {
            return Err(format!(
                "slot `{slot}` falls back to `{fallback}`, which isn't a slot"
            ));
        }
        // Following the fallbacks must end, not go round in a circle.
        let mut at = slot;
        let mut steps = 0;
        while let Some(Some(next)) = model.slots.get(at) {
            at = next;
            steps += 1;
            if steps > model.slots.len() {
                return Err(format!("slot `{slot}`'s fallbacks go round in a circle"));
            }
        }
    }
    let faces = model
        .faces
        .iter()
        .enumerate()
        .map(|(i, face)| {
            let vertices: Vec<Vertex> = face
                .vertices
                .iter()
                .map(|v| Vertex {
                    pos: Vec3::from(v.pos),
                    uv: Vec2::from(v.uv),
                })
                .collect();
            let normal = check_face(&face.texture, &vertices, &model.slots)
                .map_err(|error| format!("face {}: {error}", i + 1))?;
            Ok(ModelFace {
                slot: face.texture.clone(),
                cull: face.cull,
                vertices,
                normal,
            })
        })
        .collect::<Result<Vec<ModelFace>, String>>()?;
    let collision = match &model.collision {
        Some(boxes) => boxes
            .iter()
            .enumerate()
            .map(|(i, b)| {
                block_box(Vec3::from(b.from), Vec3::from(b.to))
                    .map_err(|error| format!("collision box {}: {error}", i + 1))
            })
            .collect::<Result<_, String>>()?,
        None => around_faces(&faces).into_iter().collect(),
    };
    Ok(Model {
        id: def.id.clone(),
        slots: model.slots.clone(),
        occludes: model.occludes.clone(),
        collision,
        faces,
    })
}

/// The box between two opposite corners, which must be inside the block
/// and apart on every axis.
fn block_box(from: Vec3, to: Vec3) -> Result<BlockBox, String> {
    let (min, max) = (from.min(to), from.max(to));
    if !(min.is_finite() && max.is_finite()) {
        return Err("a corner has a number that isn't finite".into());
    }
    if min.cmplt(Vec3::ZERO).any() || max.cmpgt(Vec3::ONE).any() {
        return Err("it must be inside the block, from [0, 0, 0] to [1, 1, 1]".into());
    }
    if min.cmpge(max).any() {
        return Err("it's flat: its corners must differ on every axis".into());
    }
    Ok(BlockBox { min, max })
}

/// The box around every face, if they have one that isn't flat and is
/// inside the block, for a model that doesn't give its `collision`.
fn around_faces(faces: &[ModelFace]) -> Option<BlockBox> {
    let mut points = faces
        .iter()
        .flat_map(|face| face.vertices.iter().map(|v| v.pos));
    let first = points.next()?;
    let (min, max) = points.fold((first, first), |(min, max), p| (min.min(p), max.max(p)));
    block_box(min.max(Vec3::ZERO), max.min(Vec3::ONE)).ok()
}

/// Checks a face and returns its normal: the face of a triangle, or of a
/// quad's first three corners, which run counter-clockwise around it.
fn check_face(
    slot: &str,
    vertices: &[Vertex],
    slots: &BTreeMap<String, Option<String>>,
) -> Result<Vec3, String> {
    if !slots.contains_key(slot) {
        return Err(format!("`{slot}` isn't one of the model's slots"));
    }
    if !(3..=4).contains(&vertices.len()) {
        return Err(format!(
            "a face has 3 or 4 vertices, not {}",
            vertices.len()
        ));
    }
    if let Some(i) = vertices
        .iter()
        .position(|v| !v.pos.is_finite() || !v.uv.is_finite())
    {
        return Err(format!("vertex {} has a number that isn't finite", i + 1));
    }
    let [a, b, c] = [0, 1, 2].map(|i| vertices[i].pos);
    let normal = (b - a).cross(c - a).normalize_or_zero();
    if normal == Vec3::ZERO {
        return Err("its first three vertices lie on a line".into());
    }
    Ok(normal)
}

fn title_case(id: &str) -> String {
    let name = id.rsplit(['/', ':']).next().unwrap_or(id).replace('_', " ");
    let mut chars = name.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Resolves a block's or item's properties, sorted by name. Materials are
/// listed by id, `#tag` by every material with the tag, in order of id.
fn properties(
    defs: &BTreeMap<String, PropertyDef>,
    namespace: &str,
    content: &Content,
    for_block: bool,
) -> Result<Vec<Property>, String> {
    let mut out = Vec::new();
    for (name, def) in defs {
        let made = match def {
            PropertyDef::Full(spec) if spec.connections.is_some() => {
                connection_properties(spec, for_block)
            }
            _ => property(name, def, namespace, content, for_block).map(|p| vec![p]),
        };
        out.extend(made.map_err(|error| format!("property `{name}`: {error}"))?);
    }
    // A connections property's sides sort in among the others.
    out.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(pair) = out.windows(2).find(|pair| pair[0].name == pair[1].name) {
        return Err(format!(
            "there are two properties called `{}`: a `connections` property makes `north`, \
             `south`, `east`, `west`, `up` and `down`",
            pair[0].name
        ));
    }
    Ok(out)
}

/// The six side properties a `connections` property becomes.
fn connection_properties(spec: &PropertySpec, for_block: bool) -> Result<Vec<Property>, String> {
    if !for_block {
        return Err("`connections` is only for block properties".into());
    }
    let alone = spec.values.is_none()
        && spec.materials.is_none()
        && !spec.optional
        && spec.default.is_none()
        && spec.item.is_none()
        && spec.place.is_none();
    if !alone {
        return Err("`connections` goes alone: the engine sets its values".into());
    }
    Ok(side_properties().collect())
}

/// What a block's `connections` property connects by: a rule, with its
/// mod, or its conduit. A conduit must connect by its conduit, and only a
/// conduit can.
fn connection_rule(
    defs: &BTreeMap<String, PropertyDef>,
    namespace: &str,
    conduit: bool,
) -> Result<Option<Rule>, String> {
    let found = defs.iter().find_map(|(name, def)| match def {
        PropertyDef::Full(spec) => spec.connections.as_ref().map(|rule| (name, rule)),
        PropertyDef::Values(_) => None,
    });
    match (found, conduit) {
        (None, false) => Ok(None),
        (None, true) => Err(
            "a conduit needs a property with `connections: true`, to join \
             the conduits next to it"
                .into(),
        ),
        (Some((_, ConnectionsDef::Conduit)), true) => Ok(Some(Rule::Conduit)),
        (Some((name, ConnectionsDef::Conduit)), false) => Err(format!(
            "property `{name}`: `connections: true` connects by the block's `conduit`, and it \
             has none"
        )),
        (Some((name, ConnectionsDef::Rule(_))), true) => Err(format!(
            "property `{name}`: a conduit connects by its network kind and group: write \
             `connections: true`"
        )),
        (Some((name, ConnectionsDef::Rule(rule))), false) => qualify(rule, namespace)
            .map(|rule| Some(Rule::Named(rule)))
            .map_err(|error| format!("property `{name}`: {error}")),
    }
}

/// A block's `conduit`, checked against its properties, to work out per
/// state.
struct ConduitMix {
    network: usize,
    group: Option<String>,
    capacity: Mix,
    throughput: Mix,
}

impl ConduitMix {
    fn new(
        def: &ConduitDef,
        namespace: &str,
        properties: &[Property],
        content: &Content,
    ) -> Result<Self, String> {
        let id = qualify(&def.network, namespace).map_err(|error| format!("conduit: {error}"))?;
        let network = content
            .network(&id)
            .ok_or_else(|| format!("conduit: no network kind `{id}`"))?;
        let mix = |field: &str, def: &Attribute| {
            Mix::new(
                &format!("conduit.{field}"),
                field,
                def,
                properties,
                None,
                content,
            )
        };
        Ok(ConduitMix {
            network,
            group: def.group.clone(),
            capacity: mix("capacity", &def.capacity)?,
            throughput: mix("throughput", &def.throughput)?,
        })
    }

    /// The conduit of the state with these values.
    fn value(
        &self,
        properties: &[Property],
        index: &[usize],
        values: &[(String, String)],
        content: &Content,
    ) -> Result<Conduit, String> {
        let group = self
            .group
            .as_deref()
            .map(|group| fill(group, values))
            .transpose()
            .map_err(|error| format!("conduit group: {error}"))?;
        if group.as_deref() == Some("") {
            return Err("conduit group is empty".into());
        }
        Ok(Conduit {
            network: self.network,
            group,
            capacity: self.capacity.value(properties, index, content),
            throughput: self.throughput.value(properties, index, content),
        })
    }
}

fn property(
    name: &str,
    def: &PropertyDef,
    namespace: &str,
    content: &Content,
    for_block: bool,
) -> Result<Property, String> {
    let mut property = listed_property(name, def, namespace, content, for_block)?;
    let PropertyDef::Full(spec) = def else {
        return Ok(property);
    };
    if let Some(default) = &spec.default {
        let default = if property.materials && default != NONE {
            qualify(default, namespace)?
        } else {
            default.clone()
        };
        let at = property
            .values
            .iter()
            .position(|value| *value == default)
            .ok_or_else(|| format!("the default `{default}` isn't one of its values"))?;
        // The default is the first value: a block's default state has every
        // property at its first.
        let value = property.values.remove(at);
        property.values.insert(0, value);
    }
    // Placement refers to values by position, so it's worked out from the
    // final order.
    if let Some(place) = &spec.place {
        property.place = Some(Placement::new(
            place.from,
            &property.values,
            place.values.as_ref(),
        )?);
    }
    Ok(property)
}

/// A property with its values in the order they're listed, and no
/// placement yet.
fn listed_property(
    name: &str,
    def: &PropertyDef,
    namespace: &str,
    content: &Content,
    for_block: bool,
) -> Result<Property, String> {
    let spec = match def {
        PropertyDef::Values(values) => {
            return plain_property(name, values, false);
        }
        PropertyDef::Full(spec) => spec,
    };
    if spec.item.is_some() && !for_block {
        return Err("`item` is only for block properties".into());
    }
    if spec.place.is_some() && !for_block {
        return Err("`place` is only for block properties".into());
    }
    match (&spec.values, &spec.materials) {
        (Some(values), None) if !spec.optional => {
            let property = plain_property(name, values, spec.item.unwrap_or(false))?;
            if spec.place.is_some() && property.item {
                return Err("a property set by `place` can't be part of the item".into());
            }
            Ok(property)
        }
        (Some(_), None) => Err("`optional` is for material properties".into()),
        (None, Some(_)) if spec.place.is_some() => {
            Err("`place` is only for plain values, not materials".into())
        }
        (None, Some(materials)) => {
            let mut values: Vec<String> = Vec::new();
            if spec.optional {
                values.push(NONE.into());
            }
            for entry in materials.values() {
                let found: Vec<String> = match entry.strip_prefix('#') {
                    Some(tag) => {
                        let tagged: Vec<String> = content
                            .materials
                            .values()
                            .filter(|m| m.has_tag(tag))
                            .map(|m| m.id.clone())
                            .collect();
                        if tagged.is_empty() {
                            return Err(format!("no material has tag `{tag}`"));
                        }
                        tagged
                    }
                    None => {
                        let id = qualify(entry, namespace)?;
                        if !content.materials.contains_key(&id) {
                            return Err(format!("no material `{id}`"));
                        }
                        vec![id]
                    }
                };
                for id in found {
                    if !values.contains(&id) {
                        values.push(id);
                    }
                }
            }
            if values.iter().all(|v| v == NONE) {
                return Err("it lists no materials".into());
            }
            Ok(Property {
                name: name.into(),
                values,
                materials: true,
                item: spec.item.unwrap_or(true),
                place: None,
            })
        }
        _ => Err("give either `values` or `materials`".into()),
    }
}

fn plain_property(name: &str, values: &[String], item: bool) -> Result<Property, String> {
    if values.is_empty() {
        return Err("it has no values".into());
    }
    if values.iter().collect::<BTreeSet<_>>().len() != values.len() {
        return Err("it lists a value twice".into());
    }
    Ok(Property {
        name: name.into(),
        values: values.to_vec(),
        materials: false,
        item,
        place: None,
    })
}

/// Checks the number of states properties make.
fn check_count(properties: &[Property]) -> Result<usize, String> {
    let count = state_count(properties);
    if count > MAX_STATES {
        return Err(format!(
            "{count} states is too many; a block or item may have {MAX_STATES}"
        ));
    }
    Ok(count)
}

/// Every combination of the properties' values, in state order (the last
/// property fastest): each one's value indices, and names and values.
fn combinations(
    properties: &[Property],
) -> impl Iterator<Item = (Vec<usize>, Vec<(String, String)>)> + '_ {
    (0..state_count(properties)).map(|offset| {
        let index = index_of(properties, offset);
        let values = named_values(properties, &index);
        (index, values)
    })
}

/// Each state's property names and values, by value index.
fn named_values(properties: &[Property], index: &[usize]) -> Vec<(String, String)> {
    properties
        .iter()
        .zip(index)
        .map(|(p, &i)| (p.name.clone(), p.values[i].clone()))
        .collect()
}

/// A variant's `when`, worked out: for each property it names, which of
/// its values match.
struct When(Vec<(usize, Vec<bool>)>);

impl When {
    fn new(
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

    fn matches(&self, index: &[usize]) -> bool {
        self.0.iter().all(|(p, values)| values[index[*p]])
    }
}

/// Which of a property's values `pattern` matches: a value, or for a
/// material property, `none`, a material or `#tag`.
fn pattern_matches(
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

/// Checks a block, bakes every one of its states, and adds them all, or
/// nothing if any state fails.
fn add_block(
    def: &Def<BlockDef>,
    content: &mut Content,
    textures: &Textures,
) -> Result<(), String> {
    let block = &def.def;
    let properties = properties(&block.properties, &def.namespace, content, true)?;
    let connections = connection_rule(&block.properties, &def.namespace, block.conduit.is_some())?
        .and_then(|rule| Connections::new(rule, &properties));
    let conduit = block
        .conduit
        .as_ref()
        .map(|conduit| ConduitMix::new(conduit, &def.namespace, &properties, content))
        .transpose()?;
    let count = check_count(&properties)?;
    if content.states.len() + count > u16::MAX as usize + 1 {
        return Err("out of block states: there are too many blocks".into());
    }
    let whens = block
        .variants
        .iter()
        .enumerate()
        .map(|(i, variant)| {
            When::new(&variant.when, &properties, &def.namespace, content)
                .map_err(|error| format!("variant {}: {error}", i + 1))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let part_whens = block
        .parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            When::new(&part.when, &properties, &def.namespace, content)
                .map_err(|error| format!("part {}: {error}", i + 1))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let attributes = Attributes::new(
        block.hardness.as_ref(),
        block.weight.as_ref(),
        &block.stats,
        &properties,
        None,
        content,
    )?;

    // A fluid is drawn as a box of its height, by the engine.
    if block.fluid.is_some() {
        let named = block.model.is_some() || block.variants.iter().any(|v| v.model.is_some());
        if named {
            return Err("a fluid has no `model`: it's drawn as a box as high as it's full".into());
        }
        if block.rotate.is_some() || block.variants.iter().any(|v| v.rotate.is_some()) {
            return Err("a fluid can't be rotated".into());
        }
        if !block.parts.is_empty() {
            return Err("a fluid has no `parts`: it's drawn as a box as high as it's full".into());
        }
    }

    // Each state's property values, look and parts, last property fastest.
    let mut looks = Vec::with_capacity(count);
    for (index, values) in combinations(&properties) {
        let look = look(block, &whens, &index);
        let parts = if block.fluid.is_some() {
            vec![Part {
                model: fluid_box().clone(),
                rotate: None,
                textures: None,
            }]
        } else {
            parts(
                block,
                &look,
                &part_whens,
                &index,
                &values,
                &def.namespace,
                content,
            )
            .map_err(|error| in_state(&values, error))?
        };
        looks.push((index, values, look, parts));
    }
    // A slot is skipped where the state's models don't have it, so a
    // variant can switch to a model with other slots; a slot no model has
    // is a mistake.
    let slots: BTreeSet<&str> = looks
        .iter()
        .flat_map(|(_, _, _, parts)| parts.iter())
        .flat_map(|part| part.model.slots.keys().map(String::as_str))
        .collect();
    let named = block
        .textures
        .keys()
        .chain(block.variants.iter().flat_map(|v| v.textures.keys()));
    if let Some(slot) = named
        .into_iter()
        .find(|slot| !slots.contains(slot.as_str()))
    {
        return Err(format!("no model this block uses has a slot `{slot}`"));
    }
    let index = content.blocks.len();
    let mut states = Vec::with_capacity(count);
    for (value_index, values, look, parts) in &looks {
        let name = match look.name.or(block.name.as_deref()) {
            Some(name) => fill_name(name, &properties, value_index, content),
            None => Ok(title_case(&def.id)),
        };
        let state = name
            .and_then(|name| {
                // Left out, they're 0.
                let numbers =
                    attributes.values(&Numbers::default(), &properties, value_index, content);
                bake_state(
                    look,
                    parts,
                    values,
                    name,
                    numbers,
                    &def.namespace,
                    index,
                    block,
                    content,
                    textures,
                )
            })
            .and_then(|mut state| {
                state.conduit = conduit
                    .as_ref()
                    .map(|conduit| conduit.value(&properties, value_index, values, content))
                    .transpose()?;
                Ok(state)
            })
            .map_err(|error| in_state(values, error))?;
        states.push(state);
    }

    content.blocks.push(Block {
        id: def.id.clone(),
        properties,
        first_state: content.states.len() as u16,
        state_count: count as u16,
        connections,
    });
    content.block_index.insert(def.id.clone(), index);
    for state in states.iter().filter(|s| s.render == Render::Cutout) {
        for face in &state.faces {
            content.textures[face.layer as usize].cutout = true;
        }
    }
    content.states.extend(states);
    Ok(())
}

/// Names the state an error is about, if the block has properties.
fn in_state(values: &[(String, String)], error: String) -> String {
    if values.is_empty() {
        return error;
    }
    let state: Vec<String> = values.iter().map(|(n, v)| format!("{n}={v}")).collect();
    format!("state {}: {error}", state.join(","))
}

/// A block state's look: the block's own, with every matching variant
/// applied in order. The name, model and rotation are replaced; textures
/// merge slot by slot.
struct Look<'d> {
    name: Option<&'d str>,
    model: Option<&'d str>,
    rotate: Option<[i32; 3]>,
    light: Option<[u8; 3]>,
    render: Option<Render>,
    textures: BTreeMap<&'d str, &'d TextureRef>,
}

fn look<'d>(block: &'d BlockDef, whens: &[When], index: &[usize]) -> Look<'d> {
    let mut look = Look {
        name: None,
        model: block.model.as_deref(),
        rotate: block.rotate,
        light: block.light,
        render: block.render,
        textures: block
            .textures
            .iter()
            .map(|(slot, texture)| (slot.as_str(), texture))
            .collect(),
    };
    for (variant, when) in block.variants.iter().zip(whens) {
        if when.matches(index) {
            look.name = variant.name.as_deref().or(look.name);
            look.model = variant.model.as_deref().or(look.model);
            look.rotate = variant.rotate.or(look.rotate);
            look.light = variant.light.or(look.light);
            look.render = variant.render.or(look.render);
            look.textures.extend(
                variant
                    .textures
                    .iter()
                    .map(|(slot, texture)| (slot.as_str(), texture)),
            );
        }
    }
    look
}

/// One model of a block state: the state's own (from its look), or one of
/// the block's `parts`.
struct Part<'d> {
    model: Model,
    rotate: Option<[i32; 3]>,
    /// The part's own textures, over the state's. `None` for the state's
    /// own model.
    textures: Option<&'d BTreeMap<String, TextureRef>>,
}

/// The model `id` names, with `{property}` filled in.
fn find_model<'a>(
    id: &str,
    values: &[(String, String)],
    namespace: &str,
    content: &'a Content,
) -> Result<&'a Model, String> {
    let id = qualify(&fill(id, values)?, namespace)?;
    content
        .models
        .get(&id)
        .ok_or_else(|| format!("no model `{id}`"))
}

/// A state's parts: the model its look chose, if any, then every one of
/// the block's `parts` that matches it. A block with no `parts` must
/// have a model.
fn parts<'d>(
    block: &'d BlockDef,
    look: &Look,
    whens: &[When],
    index: &[usize],
    values: &[(String, String)],
    namespace: &str,
    content: &Content,
) -> Result<Vec<Part<'d>>, String> {
    let mut parts = Vec::new();
    match look.model {
        Some(id) => parts.push(Part {
            model: find_model(id, values, namespace, content)?.clone(),
            rotate: look.rotate,
            textures: None,
        }),
        None if block.parts.is_empty() => return Err("no `model`".into()),
        None => {}
    }
    for (i, (part, when)) in block.parts.iter().zip(whens).enumerate() {
        if !when.matches(index) {
            continue;
        }
        let model = find_model(&part.model, values, namespace, content)
            .map_err(|error| format!("part {}: {error}", i + 1))?;
        if let Some(slot) = part
            .textures
            .keys()
            .find(|slot| !model.slots.contains_key(slot.as_str()))
        {
            return Err(format!(
                "part {}: model `{}` has no slot `{slot}`",
                i + 1,
                model.id
            ));
        }
        parts.push(Part {
            model: model.clone(),
            rotate: part.rotate,
            textures: Some(&part.textures),
        });
    }
    Ok(parts)
}

/// Bakes one state of `def`, block number `block`, called `name`, with these
/// numbers: each part's faces rotated and textured, one after the other.
/// Textures for slots a part's model doesn't have are left out of that
/// part. A fluid has nothing to bump into, whatever its model's collision.
fn bake_state(
    look: &Look,
    parts: &[Part],
    values: &[(String, String)],
    name: String,
    numbers: Numbers,
    namespace: &str,
    block: usize,
    def: &BlockDef,
    content: &mut Content,
    textures: &Textures,
) -> Result<State, String> {
    let light = look.light.unwrap_or_default();
    if let Some(level) = light.iter().find(|&&level| level > MAX_LIGHT) {
        return Err(format!(
            "light level {level} is too bright; the most is {MAX_LIGHT}"
        ));
    }
    let fluid = def.fluid.as_ref().map(bake_fluid).transpose()?;
    let mut state = State {
        block,
        name,
        values: values.to_vec(),
        light,
        render: look.render.unwrap_or_default(),
        double_sided: def.double_sided,
        hardness: numbers.hardness,
        weight: numbers.weight,
        stats: numbers.stats,
        ..default()
    };
    for part in parts {
        bake_part(
            part, look, values, namespace, def, &mut state, content, textures,
        )
        .map_err(|error| match part.textures {
            Some(_) => format!("part model `{}`: {error}", part.model.id),
            None => error,
        })?;
    }
    state.fluid = fluid;
    Ok(state)
}

/// Adds one part's faces, boxes and covered sides to `state`.
fn bake_part(
    part: &Part,
    look: &Look,
    values: &[(String, String)],
    namespace: &str,
    def: &BlockDef,
    state: &mut State,
    content: &mut Content,
    textures: &Textures,
) -> Result<(), String> {
    let model = &part.model;
    let degrees = part.rotate.unwrap_or([0; 3]);
    let rotation = rotation(degrees)?;
    // Turns a point in the block about the block's center.
    let turn = |p: Vec3| rotation * (p - Vec3::splat(0.5)) + Vec3::splat(0.5);
    let rotate_direction = |d: Direction| {
        Direction::from_normal((rotation * d.normal().as_vec3()).round().as_ivec3()).unwrap()
    };

    // The state's textures, then the part's own.
    let mut slots: BTreeMap<&str, &TextureRef> = look.textures.clone();
    if let Some(own) = part.textures {
        slots.extend(own.iter().map(|(slot, texture)| (slot.as_str(), texture)));
    }
    let mut layers = HashMap::new();
    for (slot, texture) in slots {
        if !model.slots.contains_key(slot) {
            continue;
        }
        let layer = textures.bake_ref(texture, values, namespace, content)?;
        layers.insert(slot, layer);
    }

    let first_face = state.faces.len();
    for face in &model.faces {
        let layer = slot_texture(&face.slot, model, &layers)?;
        state.faces.push(BakedFace {
            // A quarter turn keeps it a unit vector, exactly.
            normal: rotation * face.normal,
            vertices: face
                .vertices
                .iter()
                .map(|v| Vertex {
                    pos: turn(v.pos),
                    uv: v.uv,
                })
                .collect(),
            layer,
            cull: face.cull.map(rotate_direction),
        });
    }
    let occludes = model
        .occludes
        .iter()
        .fold(0, |bits, &d| bits | rotate_direction(d).bit());
    state.occludes |= occludes;
    let first_box = state.collision.len();
    if def.fluid.is_none() {
        state.collision.extend(model.collision.iter().map(|b| {
            let [a, c] = [b.min, b.max].map(turn);
            BlockBox {
                min: a.min(c),
                max: a.max(c),
            }
        }));
    }
    state.parts.push(StatePart {
        model: model.id.clone(),
        rotate: degrees,
        faces: first_face..state.faces.len(),
        collision: first_box..state.collision.len(),
        occludes,
    });
    Ok(())
}

/// A block's `fluid`, with its amounts turned from blocks into
/// [`FULL_FLUID`] units.
fn bake_fluid(def: &FluidDef) -> Result<Fluid, String> {
    let units = |name: &str, blocks: f32| -> Result<u16, String> {
        if !(0.0..=1.0).contains(&blocks) {
            return Err(format!(
                "fluid `{name}` is {blocks}; it must be from 0 to 1 block"
            ));
        }
        Ok((blocks * FULL_FLUID as f32).round() as u16)
    };
    if !(def.rate > 0.0 && def.rate.is_finite()) {
        return Err(format!("fluid `rate` is {}; it must be above 0", def.rate));
    }
    if !(0.0..=1.0).contains(&def.move_speed) {
        return Err(format!(
            "fluid `move_speed` is {}; it must be from 0 to 1",
            def.move_speed
        ));
    }
    if !(def.view.distance > 0.0 && def.view.distance.is_finite()) {
        return Err(format!(
            "fluid view `distance` is {}; it must be above 0",
            def.view.distance
        ));
    }
    let ripple = units("ripple", def.ripple)?;
    if ripple < MIN_RIPPLE {
        return Err(format!(
            "fluid `ripple` is {}; it must be at least {}, so that levelling off ends",
            def.ripple,
            MIN_RIPPLE as f32 / FULL_FLUID as f32
        ));
    }
    let color = Srgba::hex(&def.view.color)
        .map_err(|_| format!("`{}` isn't a hex color", def.view.color))?;
    Ok(Fluid {
        rate: def.rate,
        fall: units("fall", def.fall)?.max(1),
        film: units("film", def.film)?,
        ripple,
        move_speed: def.move_speed,
        view_color: color.into(),
        view_distance: def.view.distance,
    })
}

/// The texture for `slot`: the block's own, or its fallback's.
fn slot_texture(slot: &str, model: &Model, textures: &HashMap<&str, u32>) -> Result<u32, String> {
    let mut at = slot;
    loop {
        if let Some(&layer) = textures.get(at) {
            return Ok(layer);
        }
        // Fallbacks were checked to end when the model loaded.
        match model.slots.get(at).and_then(Option::as_ref) {
            Some(next) => at = next,
            None => return Err(format!("no texture for slot `{slot}`")),
        }
    }
}

/// Fills `{property}` in `text` with whatever `value` gives for it.
fn fill_with(text: &str, mut value: impl FnMut(&str) -> Option<String>) -> Result<String, String> {
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
fn fill(text: &str, values: &[(String, String)]) -> Result<String, String> {
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
fn fill_name(
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

/// A rotation of `degrees` about X, then Y, then Z, each a multiple of 90.
/// Built from exact quarter turns, so whole and half coordinates stay
/// exact.
fn rotation(degrees: [i32; 3]) -> Result<Mat3, String> {
    let quarter_turns = [
        // About X: Y goes to Z, Z to -Y.
        Mat3::from_cols(Vec3::X, Vec3::Z, Vec3::NEG_Y),
        // About Y: Z goes to X, X to -Z.
        Mat3::from_cols(Vec3::NEG_Z, Vec3::Y, Vec3::X),
        // About Z: X goes to Y, Y to -X.
        Mat3::from_cols(Vec3::Y, Vec3::NEG_X, Vec3::Z),
    ];
    let mut out = Mat3::IDENTITY;
    for (axis, d) in degrees.into_iter().enumerate() {
        if d % 90 != 0 {
            return Err(format!("`rotate` takes multiples of 90 degrees, not {d}"));
        }
        for _ in 0..d.rem_euclid(360) / 90 {
            out = quarter_turns[axis] * out;
        }
    }
    Ok(out)
}

/// Gives each block an item, unless it opts out, then adds the item files.
/// An item file with a block's id changes that block's item.
fn add_items(
    defs: &Defs,
    content: &mut Content,
    textures: &Textures,
    report: &mut impl FnMut(&str, Result<(), String>),
) {
    let overrides: HashMap<&str, &Def<ItemDef>> = defs
        .items
        .iter()
        .map(|def| (def.id.as_str(), def))
        .collect();
    for def in &defs.blocks {
        if !def.def.item || content.block(&def.id).is_none() {
            continue;
        }
        let file = overrides.get(def.id.as_str()).copied();
        let result = block_item(def, file, content, textures);
        // A mistake in the item file is that file's.
        report(file.map_or(&def.file, |file| &file.file), result);
    }
    for def in &defs.items {
        let is_block_item = defs
            .blocks
            .iter()
            .any(|block| block.id == def.id && block.def.item);
        if !is_block_item {
            report(&def.file, standalone_item(def, content, textures));
        } else if content.block(&def.id).is_none() {
            report(
                &def.file,
                Err(format!(
                    "block `{}` didn't load, so it has no item to change",
                    def.id
                )),
            );
        }
    }
}

/// Adds the item that places block `id`: one state per combination of the
/// block's item properties, each placing the block with those values and
/// the rest at their defaults. `file` is an item file changing it.
fn block_item(
    block_def: &Def<BlockDef>,
    file: Option<&Def<ItemDef>>,
    content: &mut Content,
    textures: &Textures,
) -> Result<(), String> {
    let id = block_def.id.as_str();
    let block = content.block(id).unwrap().clone();
    let category = match file.and_then(|file| file.def.category.as_ref()) {
        Some(reference) => category(Some(reference), &file.unwrap().namespace, content)?,
        None => category(
            block_def.def.category.as_ref(),
            &block_def.namespace,
            content,
        )?,
    };
    if let Some(file) = file {
        if !file.def.properties.is_empty() {
            return Err("a block's item has the block's properties; it can't add its own".into());
        }
        if file.def.block.is_some() {
            return Err("a block's item places that block; leave out `block`".into());
        }
    }
    let properties: Vec<Property> = block
        .properties
        .iter()
        .filter(|p| p.item)
        .cloned()
        .collect();
    let count = check_count(&properties)?;
    let no_stats = BTreeMap::new();
    let attributes = Attributes::new(
        file.and_then(|f| f.def.hardness.as_ref()),
        file.and_then(|f| f.def.weight.as_ref()),
        file.map_or(&no_stats, |f| &f.def.stats),
        &properties,
        Some(&block.properties),
        content,
    )?;
    let item = content.items.len();
    let mut states = Vec::with_capacity(count);
    for (index, values) in combinations(&properties) {
        let info = (|| {
            let state = placed_state(&block, &values)?;
            let placed = content.state(state);
            let numbers = attributes.values(&placed_numbers(placed), &properties, &index, content);
            let mut info = ItemStateInfo {
                item,
                values: values.clone(),
                name: placed.name.clone(),
                texture: None,
                block: Some(state),
                hardness: numbers.hardness,
                weight: numbers.weight,
                stats: numbers.stats,
            };
            if let Some(file) = file {
                if let Some(name) = &file.def.name {
                    info.name = fill_name(name, &properties, &index, content)?;
                }
                if let Some(texture) = &file.def.texture {
                    info.texture =
                        Some(textures.bake_ref(texture, &values, &file.namespace, content)?);
                }
            }
            Ok::<_, String>(info)
        })()
        .map_err(|error| in_state(&values, error))?;
        states.push(info);
    }
    let unique = file.and_then(|file| file.def.unique).unwrap_or(false);
    push_item(id, properties, unique, category, states, content);
    Ok(())
}

/// Adds an item from its own file.
fn standalone_item(
    def: &Def<ItemDef>,
    content: &mut Content,
    textures: &Textures,
) -> Result<(), String> {
    let item_def = &def.def;
    let category = category(item_def.category.as_ref(), &def.namespace, content)?;
    let properties = properties(&item_def.properties, &def.namespace, content, false)?;
    let count = check_count(&properties)?;
    let block = match &item_def.block {
        Some(block) => {
            let id = qualify(block, &def.namespace)?;
            Some(
                content
                    .block(&id)
                    .ok_or_else(|| format!("no block `{id}`"))?
                    .clone(),
            )
        }
        None => None,
    };
    let attributes = Attributes::new(
        item_def.hardness.as_ref(),
        item_def.weight.as_ref(),
        &item_def.stats,
        &properties,
        None,
        content,
    )?;
    let item = content.items.len();
    let mut states = Vec::with_capacity(count);
    for (index, values) in combinations(&properties) {
        let info = (|| {
            let name = match &item_def.name {
                Some(name) => fill_name(name, &properties, &index, content)?,
                None => title_case(&def.id),
            };
            let texture = match &item_def.texture {
                Some(texture) => {
                    Some(textures.bake_ref(texture, &values, &def.namespace, content)?)
                }
                None => None,
            };
            let block = match &block {
                Some(block) => Some(placed_state(block, &values)?),
                None => None,
            };
            // Left out, it's as hard and heavy as the block it places, with
            // its stats.
            let fallback = block.map_or_else(Numbers::default, |state| {
                placed_numbers(content.state(state))
            });
            let numbers = attributes.values(&fallback, &properties, &index, content);
            Ok::<_, String>(ItemStateInfo {
                item,
                values: values.clone(),
                name,
                texture,
                block,
                hardness: numbers.hardness,
                weight: numbers.weight,
                stats: numbers.stats,
            })
        })()
        .map_err(|error| in_state(&values, error))?;
        states.push(info);
    }
    let unique = item_def.unique.unwrap_or(false);
    push_item(&def.id, properties, unique, category, states, content);
    Ok(())
}

/// The numbers of a block state, which an item placing it falls back to.
fn placed_numbers(state: &State) -> Numbers {
    Numbers {
        hardness: state.hardness,
        weight: state.weight,
        stats: state.stats.clone(),
    }
}

/// The state of `block` an item with these values places: its default,
/// with each property the item shares set to the item's value.
fn placed_state(block: &Block, values: &[(String, String)]) -> Result<BlockState, String> {
    let shared: Vec<(&str, &str)> = values
        .iter()
        .filter(|(name, _)| block.properties.iter().any(|p| &p.name == name))
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    block.state(&shared).ok_or_else(|| {
        let (name, value) = shared
            .iter()
            .find(|(n, v)| block.state(&[(n, v)]).is_none())
            .copied()
            .unwrap_or_default();
        format!("block `{}` has no `{name}` value `{value}`", block.id)
    })
}

fn push_item(
    id: &str,
    properties: Vec<Property>,
    unique: bool,
    category: Option<usize>,
    states: Vec<ItemStateInfo>,
    content: &mut Content,
) {
    content.item_index.insert(id.into(), content.items.len());
    content.items.push(Item {
        id: id.into(),
        properties,
        first_state: content.item_states.len() as u32,
        state_count: states.len() as u32,
        unique,
        category,
    });
    content.item_states.extend(states);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::load::ModFiles;

    use crate::content::{QUAD, test_mod as content};

    fn errors(content: &Content) -> String {
        content.errors.join("\n")
    }

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

    #[test]
    fn a_minimal_block_loads_with_an_item() {
        let content = content(&[(
            "blocks/thing.json5",
            r#"{ model: "quad", textures: { all: "a" } }"#,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let block = content.block("test:thing").unwrap();
        assert_eq!(content.state(block.default_state()).name, "Thing");
        let state = content.state(block.default_state());
        assert_eq!(state.faces.len(), 1);
        assert_eq!(state.faces[0].layer, content.texture("test:a").unwrap());
        assert_eq!(state.faces[0].normal, Vec3::Y);
        assert!(state.occludes(Direction::Up));
        assert!(!state.occludes(Direction::Down));
        let item = content.default_item_info("test:thing").unwrap();
        assert_eq!(item.block, Some(block.default_state()));
        assert_eq!(item.name, "Thing");
        assert!(!content.item("test:thing").unwrap().unique);
    }

    /// `light` gives a block's light per colour channel; variants replace
    /// it. Cubes are opaque to light, other shapes aren't.
    #[test]
    fn blocks_give_off_the_light_they_say() {
        let content = content(&[
            (
                "blocks/lamp.json5",
                r#"{
                    properties: { color: ["red", "off"] },
                    model: "quad",
                    textures: { all: "a" },
                    light: [15, 2, 0],
                    variants: [{ when: { color: "off" }, light: [0, 0, 0] }],
                }"#,
            ),
            (
                "blocks/plain.json5",
                r#"{ model: "quad", textures: { all: "a" } }"#,
            ),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let lamp = content.block("test:lamp").unwrap();
        let red = content.state(lamp.state(&[("color", "red")]).unwrap());
        assert_eq!(red.light, [15, 2, 0]);
        assert!(red.emits_light());
        let off = content.state(lamp.state(&[("color", "off")]).unwrap());
        assert!(!off.emits_light());
        let plain = content.state(content.default_state("test:plain").unwrap());
        assert_eq!(plain.light, [0; 3]);
        assert!(!plain.is_opaque(), "a quad lets light through");
        assert!(
            crate::content::base()
                .state(crate::content::block("stone"))
                .is_opaque()
        );
        assert!(!content.state(crate::content::BlockState::AIR).is_opaque());
    }

    /// A block's `fluid`, for tests that only need it to be a fluid.
    pub(crate) const TEST_FLUID: &str = r##"fluid: {
        rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5,
        view: { color: "#203060", distance: 12 },
    }"##;

    /// A fluid's amounts are turned from blocks into thousandths, and a
    /// `fall` too small to move anything moves one.
    #[test]
    fn fluid_amounts_are_baked_to_thousandths() {
        let content = content(&[(
            "blocks/pool.json5",
            r##"{ textures: { all: "a" }, fluid: {
                rate: 4, fall: 0.0001, film: 0.25, ripple: 0.05, move_speed: 0.3,
                view: { color: "#102030", distance: 3 },
            } }"##,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let state = content.state(content.default_state("test:pool").unwrap());
        let fluid = state.fluid.as_ref().unwrap();
        assert_eq!((fluid.fall, fluid.film, fluid.ripple), (1, 250, 50));
        assert_eq!(
            (fluid.rate, fluid.move_speed, fluid.view_distance),
            (4.0, 0.3, 3.0)
        );
        assert_eq!(fluid.view_color, Srgba::hex("#102030").unwrap().into());
    }

    /// `render` says how a block's faces are drawn (opaque unless it says
    /// otherwise), and variants replace it. A see-through block lets light
    /// through its covered sides, and its covered sides hide only the
    /// faces they should. `fluid` is off unless set.
    #[test]
    fn blocks_are_drawn_as_their_render_says() {
        // The quad covers its top.
        let content = content(&[
            (
                "blocks/pane.json5",
                r#"{
                    properties: { kind: ["clear", "leafy", "solid"] },
                    model: "quad",
                    textures: { all: "a" },
                    render: "translucent",
                    variants: [
                        { when: { kind: "leafy" }, render: "cutout" },
                        { when: { kind: "solid" }, render: "opaque" },
                    ],
                }"#,
            ),
            (
                "blocks/pool.json5",
                &format!(r#"{{ textures: {{ all: "a" }}, {TEST_FLUID} }}"#),
            ),
            (
                "blocks/tar.json5",
                &format!(r#"{{ textures: {{ all: "a" }}, {TEST_FLUID} }}"#),
            ),
            (
                "blocks/deep.json5",
                &format!(r#"{{ textures: {{ all: "a" }}, {TEST_FLUID} }}"#),
            ),
            (
                "blocks/box.json5",
                r#"{ model: "pair", textures: { top: "a", bottom: "a" } }"#,
            ),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let pane = content.block("test:pane").unwrap();
        let state = |kind| pane.state(&[("kind", kind)]).unwrap();
        let [clear, leafy, solid] = ["clear", "leafy", "solid"].map(state);
        let render = |s| content.state(s).render;
        assert_eq!(
            [clear, leafy, solid].map(render),
            [Render::Translucent, Render::Cutout, Render::Opaque]
        );
        for see_through in [clear, leafy] {
            let state = content.state(see_through);
            assert!(state.occludes(Direction::Up) && !state.blocks_light(Direction::Up));
        }
        assert!(content.state(solid).blocks_light(Direction::Up));
        let pool = content.default_state("test:pool").unwrap();
        assert!(content.state(pool).is_fluid() && !content.state(solid).is_fluid());
        assert_eq!(render(pool), Render::Opaque);
        // A fluid has no collision.
        let collision = |id| &content.state(content.default_state(id).unwrap()).collision;
        assert_eq!(collision("test:box"), &[BlockBox::FULL]);
        assert!(collision("test:deep").is_empty());

        // What hides a block's bottom face, from below.
        let hides = |block, neighbour| content.hides(block, neighbour, Direction::Down);
        assert!(hides(clear, solid) && hides(leafy, solid));
        assert!(
            hides(clear, pool),
            "an opaque fluid hides see-through faces"
        );
        // Two different opaque fluids: exactly one face between them.
        let tar = content.default_state("test:tar").unwrap();
        assert!(
            hides(pool, tar) != hides(tar, pool),
            "one face between two fluids"
        );
        assert!(!hides(solid, pool), "but not a solid opaque block's");
        assert!(hides(pool, pool), "an opaque fluid hides itself");
        assert!(hides(clear, clear), "the same translucent block");
        assert!(
            !hides(solid, clear),
            "only the same block's translucent faces"
        );
        assert!(!hides(pool, clear), "another translucent block");
        assert!(
            !hides(pool, leafy) && !hides(leafy, leafy),
            "cut-outs hide nothing"
        );
    }

    #[test]
    fn an_unknown_render_is_an_error() {
        let content = content(&[(
            "blocks/odd.json5",
            r#"{ model: "quad", textures: { all: "a" }, render: "glowing" }"#,
        )]);
        assert!(content.block("test:odd").is_none());
        assert!(errors(&content).contains("glowing"), "{}", errors(&content));
    }

    #[test]
    fn light_brighter_than_the_maximum_is_an_error() {
        let content = content(&[(
            "blocks/sun.json5",
            r#"{ model: "quad", textures: { all: "a" }, light: [16, 0, 0] }"#,
        )]);
        assert!(content.block("test:sun").is_none());
        assert!(
            errors(&content).contains("light level 16"),
            "{}",
            errors(&content)
        );
    }

    #[test]
    fn textures_use_materials_or_inline_palettes() {
        let content = content(&[]);
        let [a, b] =
            ["test:a", "test:b"].map(|id| &content.textures[content.texture(id).unwrap() as usize]);
        assert_eq!(a.layers[0].image, "mods/test/images/a.png");
        assert_eq!(a.layers[0].palette, content.materials["test:grey"].palette);
        let red = b.layers[0].palette.color(0).to_u8_array();
        assert_eq!(red, [255, 0, 0, 255]);
    }

    /// Variants apply in order where they match, textures merge, and
    /// `{property}` picks a texture per state.
    #[test]
    fn variants_and_property_names_pick_the_look() {
        let content = content(&[(
            "blocks/thing.json5",
            r#"{
                properties: { tex: ["a", "b"], turn: ["no", "yes"] },
                model: "quad",
                textures: { all: "{tex}" },
                variants: [
                    { when: { turn: "yes" }, rotate: [0, 0, 90] },
                    { when: { tex: "b", turn: ["yes"] }, textures: { side: "a" } },
                ],
            }"#,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let block = content.block("test:thing").unwrap();
        assert_eq!(block.state_count, 4);
        let [a, b] = ["test:a", "test:b"].map(|id| content.texture(id).unwrap());
        let face =
            |values: &[(&str, &str)]| content.state(block.state(values).unwrap()).faces[0].clone();
        assert_eq!(face(&[("tex", "a")]).layer, a);
        assert_eq!(face(&[("tex", "b")]).layer, b);
        // Turned a quarter about Z, the top face faces west.
        let turned = face(&[("tex", "b"), ("turn", "yes")]);
        assert_eq!(turned.normal, Vec3::NEG_X);
        assert_eq!(turned.cull, Some(Direction::West));
        // About the block's centre: the quad on the top moves to the west
        // side, still inside the block.
        for vertex in &turned.vertices {
            assert_eq!(vertex.pos.x, 0.0, "{vertex:?}");
            assert!(vertex.pos.cmpge(Vec3::ZERO).all() && vertex.pos.cmple(Vec3::ONE).all());
        }
        // The second variant's `side` beats the block's `all`.
        assert_eq!(turned.layer, a);
        let state = content.state(block.state(&[("turn", "yes")]).unwrap());
        assert!(state.occludes(Direction::West) && !state.occludes(Direction::Up));
    }

    /// When several matching variants set the same thing, the last wins:
    /// the model and rotation are replaced, and textures are merged slot by
    /// slot, so a slot only the block sets survives. A block can set slots
    /// for one model and switch to another with different slots.
    #[test]
    fn later_variants_win_and_textures_merge_by_slot() {
        let content = content(&[(
            "blocks/thing.json5",
            r#"{
                properties: { p: ["x", "y"] },
                model: "quad",
                rotate: [0, 0, 90],
                textures: { all: "a", top: "a", bottom: "b" },
                variants: [
                    { when: { p: "y" }, rotate: [0, 0, 180], textures: { top: "b" } },
                    { when: { p: "y" }, model: "pair", rotate: [0, 0, 0], textures: { top: "c" } },
                ],
            }"#,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let [a, b, c] = ["test:a", "test:b", "test:c"].map(|id| content.texture(id).unwrap());
        let block = content.block("test:thing").unwrap();

        let x = content.state(block.state(&[("p", "x")]).unwrap());
        assert_eq!(x.faces.len(), 1);
        assert_eq!(x.faces[0].normal, Vec3::NEG_X);
        assert_eq!(x.faces[0].layer, a);

        let y = content.state(block.state(&[("p", "y")]).unwrap());
        let faces: Vec<(Vec3, u32)> = y.faces.iter().map(|f| (f.normal, f.layer)).collect();
        assert_eq!(faces, [(Vec3::Y, c), (Vec3::NEG_Y, b)]);
    }

    /// Textures for slots a state's model doesn't have aren't looked up, so
    /// a `{property}` texture only has to exist for the states whose model
    /// uses it.
    #[test]
    fn slots_the_model_lacks_are_skipped_for_that_state() {
        let content = content(&[(
            "blocks/thing.json5",
            r#"{
                properties: { p: ["a", "y"] },
                model: "quad",
                textures: { all: "{p}", top: "a", bottom: "a" },
                variants: [{ when: { p: "y" }, model: "pair" }],
            }"#,
        )]);
        // There's no texture `test:y`, but the `pair` state has no `all`.
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let block = content.block("test:thing").unwrap();
        let y = content.state(block.state(&[("p", "y")]).unwrap());
        assert_eq!(y.faces.len(), 2);
    }

    /// An image file that isn't there is reported, but the texture still
    /// loads, so blocks using it show the placeholder instead of vanishing.
    #[test]
    fn a_missing_image_is_reported_and_the_texture_kept() {
        let content = content(&[
            (
                "textures/x.json5",
                r#"{ layers: [{ image: "missing", material: "grey" }] }"#,
            ),
            (
                "blocks/x.json5",
                r#"{ model: "quad", textures: { all: "x" } }"#,
            ),
        ]);
        let all = errors(&content);
        assert_eq!(content.errors.len(), 1, "{all}");
        assert!(
            all.contains("mods/test/textures/x.json5")
                && all.contains("there's no mods/test/images/missing.png"),
            "{all}"
        );
        let texture = content.texture("test:x").unwrap();
        assert_eq!(
            content.textures[texture as usize].layers[0].image,
            "mods/test/images/missing.png"
        );
        assert!(content.block("test:x").is_some());
    }

    /// A model's collision is the box around its faces unless it gives its
    /// own boxes, and it turns with the block.
    #[test]
    fn collision_is_the_box_around_the_faces_or_given() {
        let content = content(&[
            // QUAD is flat, so there's nothing to bump into by default.
            (
                "blocks/flat.json5",
                r#"{ model: "quad", textures: { all: "a" } }"#,
            ),
            (
                "models/post.json5",
                r#"{ collision: [{ from: [0.75, 0, 0.25], to: [0.25, 1, 0.75] }], faces: [] }"#,
            ),
            ("models/ghost.json5", r#"{ collision: [], faces: [] }"#),
            ("blocks/post.json5", r#"{ model: "post" }"#),
            (
                "blocks/lying_post.json5",
                r#"{ model: "post", rotate: [0, 0, 90] }"#,
            ),
            ("blocks/ghost.json5", r#"{ model: "ghost" }"#),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let collision = |id: &str| {
            let state = content.default_state(&format!("test:{id}")).unwrap();
            content.state(state).collision.clone()
        };
        assert_eq!(collision("flat"), []);
        assert_eq!(collision("ghost"), []);
        // Corners in either order.
        let post = BlockBox {
            min: Vec3::new(0.25, 0.0, 0.25),
            max: Vec3::new(0.75, 1.0, 0.75),
        };
        assert_eq!(collision("post"), [post]);
        assert_eq!(
            collision("lying_post"),
            [BlockBox {
                min: Vec3::new(0.0, 0.25, 0.25),
                max: Vec3::new(1.0, 0.75, 0.75),
            }]
        );
        let base = crate::content::base();
        let slab = base.default_state("defaria:stone_slab").unwrap();
        assert_eq!(
            base.state(slab).collision,
            [BlockBox {
                min: Vec3::ZERO,
                max: Vec3::new(1.0, 0.5, 1.0),
            }]
        );
        let stone = base.default_state("defaria:stone").unwrap();
        assert_eq!(base.state(stone).collision, [BlockBox::FULL]);
    }

    /// A broken texture leaves out the block that uses it, and that block's
    /// item: each is reported against its own file.
    #[test]
    fn a_broken_definition_takes_its_dependents_with_it() {
        let content = content(&[
            (
                "textures/broken.json5",
                r#"{ layers: [{ image: "a", material: "missing" }] }"#,
            ),
            (
                "blocks/user.json5",
                r#"{ model: "quad", textures: { all: "broken" } }"#,
            ),
            (
                "blocks/fine.json5",
                r#"{ model: "quad", textures: { all: "a" } }"#,
            ),
        ]);
        assert_eq!(content.errors.len(), 2, "{}", errors(&content));
        assert!(content.errors[0].contains("textures/broken.json5"));
        assert!(content.errors[1].contains("blocks/user.json5"));
        assert!(content.errors[1].contains("no texture `test:broken`"));
        assert!(content.block("test:user").is_none());
        assert!(content.item("test:user").is_none());
        assert!(content.block("test:fine").is_some());

        // An item file changing a block that didn't load says so.
        let content = crate::content::test_mod(&[
            (
                "blocks/sun.json5",
                r#"{ model: "quad", textures: { all: "a" }, light: [16, 0, 0] }"#,
            ),
            ("items/sun.json5", r#"{ unique: true }"#),
        ]);
        let all = errors(&content);
        assert_eq!(content.errors.len(), 2, "{all}");
        assert!(
            content.errors[1].contains("items/sun.json5")
                && content.errors[1].contains("block `test:sun` didn't load"),
            "{all}"
        );
        assert!(content.item("test:sun").is_none());
    }

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

    #[test]
    fn rotations_are_exact_quarter_turns() {
        let turn = rotation([90, 0, 0]).unwrap();
        assert_eq!(turn * Vec3::Y, Vec3::Z);
        assert_eq!(rotation([0, 90, 0]).unwrap() * Vec3::Z, Vec3::X);
        assert_eq!(rotation([0, 0, 90]).unwrap() * Vec3::X, Vec3::Y);
        assert_eq!(rotation([0, 0, -90]).unwrap() * Vec3::Y, Vec3::X);
        // X first, then Z: Y goes to Z, which Z leaves alone.
        assert_eq!(rotation([90, 0, 90]).unwrap() * Vec3::Y, Vec3::Z);
        assert_eq!(rotation([0, 360, 0]).unwrap(), Mat3::IDENTITY);
        assert!(rotation([45, 0, 0]).is_err());
    }

    #[test]
    fn fill_replaces_property_names() {
        let values = [("color".to_string(), "red".to_string())];
        assert_eq!(fill("wool_{color}", &values).unwrap(), "wool_red");
        assert_eq!(fill("plain", &values).unwrap(), "plain");
        assert!(fill("wool_{colour}", &values).is_err());
        assert!(fill("wool_{color", &values).is_err());
    }

    /// An item with a block's id changes the block's item; other items are
    /// new. Blocks can opt out of an item.
    #[test]
    fn items_extend_or_replace_block_items() {
        let content = content(&[
            (
                "blocks/thing.json5",
                r#"{ name: "Oak thing", model: "quad", textures: { all: "a" } }"#,
            ),
            (
                "blocks/plain.json5",
                r#"{ name: "Plain thing", model: "quad", textures: { all: "a" } }"#,
            ),
            ("items/plain.json5", r#"{ unique: true }"#),
            (
                "blocks/hidden.json5",
                r#"{ model: "quad", textures: { all: "a" }, item: false }"#,
            ),
            ("items/thing.json5", r#"{ name: "Special thing" }"#),
            ("items/seeds.json5", r#"{ texture: "b", block: "hidden" }"#),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let thing = content.default_item_info("test:thing").unwrap();
        assert_eq!(thing.name, "Special thing");
        assert!(!content.item("test:thing").unwrap().unique);
        assert_eq!(thing.block, content.default_state("test:thing"));
        // What an item file leaves out comes from the block.
        let plain = content.default_item_info("test:plain").unwrap();
        assert_eq!(plain.name, "Plain thing");
        assert!(content.item("test:plain").unwrap().unique);
        assert_eq!(plain.block, content.default_state("test:plain"));
        assert!(content.item("test:hidden").is_none());
        let seeds = content.default_item_info("test:seeds").unwrap();
        assert_eq!(seeds.name, "Seeds");
        assert_eq!(seeds.block, content.default_state("test:hidden"));
        assert_eq!(seeds.texture, content.texture("test:b"));
        assert_eq!(content.items.len(), 3);
    }

    /// Mods refer to each other's definitions with `mod:name`, and images
    /// load from the mod that owns them.
    #[test]
    fn mods_use_each_others_definitions() {
        let base = ModFiles::new(
            "mods/base",
            r#"{ id: "base", name: "Base" }"#,
            &[
                ("images/rock.png", ""),
                (
                    "materials/rock.json5",
                    r##"{ palette: ["#000", "#fff"], hardness: 1, weight: 1 }"##,
                ),
                ("models/quad.json5", QUAD),
            ],
        );
        let addon = ModFiles::new(
            "mods/addon",
            r#"{ id: "addon", name: "Addon", dependencies: ["base"] }"#,
            &[
                (
                    "textures/rock.json5",
                    r#"{ layers: [{ image: "base:rock", material: "base:rock" }] }"#,
                ),
                (
                    "blocks/rock.json5",
                    r#"{ model: "base:quad", textures: { all: "rock" } }"#,
                ),
            ],
        );
        let content = Content::from_mods(vec![addon, base]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        assert_eq!(content.mods[0].id, "base");
        let texture = &content.textures[content.texture("addon:rock").unwrap() as usize];
        assert_eq!(texture.layers[0].image, "mods/base/images/rock.png");
        assert!(content.block("addon:rock").is_some());
    }

    /// A base mod with soils `dirt` and `sand` (each with its own image)
    /// and turfs `fern` and `moss`, textures `soil` and `soil_turf` with `#tag` inputs
    /// and images named by them, and a block of any soil with optional
    /// turf; plus `extra` files.
    fn soil_mod(extra: &[(&str, &str)]) -> ModFiles {
        let mut files = vec![
            ("images/dirt.png", ""),
            ("images/sand.png", ""),
            ("images/overhang.png", ""),
            (
                "materials/dirt.json5",
                r##"{ tags: ["soil"], palette: ["#100000", "#200000"], hardness: 1, weight: 1 }"##,
            ),
            (
                "materials/sand.json5",
                r##"{ tags: ["soil"], palette: ["#001000", "#002000"], hardness: 1, weight: 1 }"##,
            ),
            (
                "materials/moss.json5",
                r##"{ tags: ["turf"], palette: ["#000010", "#000020"], hardness: 1, weight: 1 }"##,
            ),
            (
                "materials/fern.json5",
                r##"{ tags: ["turf"], palette: ["#001010", "#002020"], hardness: 1, weight: 1 }"##,
            ),
            (
                "textures/soil.json5",
                r##"{ layers: [{ image: "{soil}", material: "#soil" }] }"##,
            ),
            (
                "textures/soil_turf.json5",
                r##"{ layers: [
                    { image: "{soil}", material: "#soil" },
                    { image: "overhang", material: "#turf" },
                ] }"##,
            ),
            ("models/quad.json5", QUAD),
            (
                "blocks/ground.json5",
                r##"{
                    properties: {
                        soil: { materials: "#soil" },
                        turf: { materials: "#turf", optional: true },
                    },
                    model: "quad",
                    textures: { all: { texture: "soil", materials: { soil: "{soil}" } } },
                    variants: [{
                        when: { turf: { not: "none" } },
                        textures: { all: {
                            texture: "soil_turf",
                            materials: { soil: "{soil}", turf: "{turf}" },
                        } },
                    }],
                }"##,
            ),
        ];
        files.extend_from_slice(extra);
        ModFiles::new("mods/base", r##"{ id: "base", name: "Base" }"##, &files)
    }

    /// The images of the texture baked as `key`, by file name.
    fn images_of(content: &Content, key: &str) -> Vec<String> {
        let layer = content
            .texture(key)
            .unwrap_or_else(|| panic!("no tile {key}"));
        content.textures[layer as usize]
            .layers
            .iter()
            .map(|layer| layer.image.rsplit('/').next().unwrap().to_owned())
            .collect()
    }

    /// A texture whose inputs are `#tag`s is baked at load for every
    /// combination of materials with those tags, whether or not anything
    /// uses it, and `{input}` in an image picks the image named after each
    /// material.
    #[test]
    fn tagged_inputs_bake_every_combination() {
        let content = Content::from_mods(vec![soil_mod(&[(
            "textures/unused.json5",
            r##"{ layers: [
                { image: "{soil}", material: "#soil" },
                { image: "overhang", material: "#turf" },
            ] }"##,
        )])]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        for key in [
            "base:soil(soil=base:dirt)",
            "base:soil(soil=base:sand)",
            "base:unused(soil=base:dirt,turf=base:fern)",
            "base:unused(soil=base:dirt,turf=base:moss)",
            "base:unused(soil=base:sand,turf=base:fern)",
            "base:unused(soil=base:sand,turf=base:moss)",
        ] {
            assert!(content.texture(key).is_some(), "no tile {key}");
        }
        assert_eq!(
            images_of(&content, "base:soil(soil=base:sand)"),
            ["sand.png"]
        );
        // Two soils and two turfs make four tiles.
        for (soil, turf) in [
            ("dirt", "fern"),
            ("dirt", "moss"),
            ("sand", "fern"),
            ("sand", "moss"),
        ] {
            let key = format!("base:soil_turf(soil=base:{soil},turf=base:{turf})");
            assert_eq!(
                images_of(&content, &key),
                [format!("{soil}.png"), "overhang.png".into()]
            );
        }
        // Each layer takes its own material's palette.
        let turfed = content
            .texture("base:soil_turf(soil=base:dirt,turf=base:moss)")
            .unwrap();
        let layers = &content.textures[turfed as usize].layers;
        assert_eq!(layers[0].palette, content.materials["base:dirt"].palette);
        assert_eq!(layers[1].palette, content.materials["base:moss"].palette);
        // Blocks use the tiles baked at load rather than making more.
        let ground = content.block("base:ground").unwrap();
        assert_eq!(ground.state_count, 2 * 3);
        let sand = content.state(ground.state(&[("soil", "base:sand")]).unwrap());
        assert_eq!(
            sand.faces[0].layer,
            content.texture("base:soil(soil=base:sand)").unwrap()
        );
        assert_eq!(content.textures.len(), 2 + 2 * 2 + 2 * 2);
    }

    /// Another mod adds a soil with only a material tagged `soil` and its
    /// image: the base mod's textures and block take it in. If its image is
    /// missing, that's reported and its tiles show the placeholder, and the
    /// block still loads.
    #[test]
    fn a_mod_adds_a_soil_with_a_material_and_an_image() {
        let clay = |files: &[(&str, &str)]| {
            ModFiles::new(
                "mods/clay",
                r##"{ id: "clay", name: "Clay", dependencies: ["base"] }"##,
                files,
            )
        };
        let material = (
            "materials/clay.json5",
            r##"{ tags: ["soil"], palette: ["#101010", "#202020"], hardness: 1, weight: 1 }"##,
        );
        let content = Content::from_mods(vec![
            soil_mod(&[]),
            clay(&[("images/clay.png", ""), material]),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        assert_eq!(
            images_of(&content, "base:soil(soil=clay:clay)"),
            ["clay.png"]
        );
        let ground = content.block("base:ground").unwrap();
        assert!(
            ground
                .state(&[("soil", "clay:clay"), ("turf", "base:moss")])
                .is_some()
        );

        let content = Content::from_mods(vec![soil_mod(&[]), clay(&[material])]);
        let all = errors(&content);
        // Once for each texture that uses it, not once per tile.
        let missing = "there's no mods/clay/images/clay.png";
        assert_eq!(all.matches(missing).count(), 2, "{all}");
        assert!(all.contains("mods/base/textures/soil.json5"), "{all}");
        assert!(all.contains("mods/base/textures/soil_turf.json5"), "{all}");
        assert!(content.block("base:ground").is_some(), "{all}");
        assert!(
            content
                .image_paths()
                .contains(&"mods/clay/images/clay.png".to_owned())
        );
    }

    /// 46 materials tagged `a` and 46 tagged `b`: 2116 combinations, more
    /// than [`MAX_TILES`].
    fn many_materials() -> Vec<(String, String)> {
        (0..46)
            .flat_map(|i| {
                ["a", "b"].map(|tag| {
                    (
                        format!("materials/{tag}{i}.json5"),
                        format!(
                            r##"{{ tags: ["{tag}"], palette: ["#000000", "#ffffff"], hardness: 1, weight: 1 }}"##
                        ),
                    )
                })
            })
            .collect()
    }

    fn load_with(files: &[(String, String)]) -> Content {
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        Content::from_mods(vec![soil_mod(&refs)])
    }

    /// Tags whose combinations would make more tiles than the game has
    /// room for are reported against the texture, which is left out, and
    /// what uses it fails rather than making the tiles one at a time.
    #[test]
    fn too_many_combinations_are_reported() {
        let mut files = many_materials();
        files.push((
            "textures/x.json5".into(),
            r##"{ layers: [
                { image: "overhang", material: "#a" },
                { image: "overhang", material: "#b" },
            ] }"##
                .into(),
        ));
        files.push((
            "blocks/x.json5".into(),
            r##"{ model: "quad", textures: { all: {
                texture: "x", materials: { a: "a0", b: "b0" },
            } } }"##
                .into(),
        ));
        let content = load_with(&files);
        let all = errors(&content);
        assert!(
            all.contains("mods/base/textures/x.json5: its combinations make more tiles"),
            "{all}"
        );
        assert!(all.contains("mods/base/blocks/x.json5"), "{all}");
        assert!(content.textures.iter().all(|t| t.template != "base:x"));
        assert!(content.textures.len() < MAX_TILES);
    }

    /// Tiles made as blocks use `$name` inputs stop at [`MAX_TILES`]: the
    /// block that would go past it fails, and the tiles fit the GPU.
    #[test]
    fn tiles_made_on_use_stop_at_the_limit() {
        let mut files = many_materials();
        files.push((
            "textures/y.json5".into(),
            r##"{ layers: [
                { image: "overhang", material: "$a" },
                { image: "overhang", material: "$b" },
            ] }"##
                .into(),
        ));
        files.push((
            "blocks/y.json5".into(),
            r##"{
                properties: { a: { materials: "#a" }, b: { materials: "#b" } },
                model: "quad",
                textures: { all: { texture: "y", materials: { a: "{a}", b: "{b}" } } },
            }"##
            .into(),
        ));
        let content = load_with(&files);
        let all = errors(&content);
        assert!(all.contains("no room for more than 2048 tiles"), "{all}");
        assert!(content.block("base:y").is_none());
        assert_eq!(content.textures.len(), MAX_TILES);
    }

    /// Mistakes with `#tag` inputs are reported.
    #[test]
    fn tagged_input_mistakes_are_reported() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "textures/x.json5",
                r##"{ layers: [
                    { image: "overhang", material: "#soil" },
                    { image: "overhang", material: "$other" },
                ] }"##,
                "all `$name` or all `#tag`",
            ),
            (
                "textures/x.json5",
                r##"{ layers: [{ image: "{other}", material: "$other" }] }"##,
                "uses `{other}`, which isn't a `#tag` input",
            ),
            (
                "textures/x.json5",
                r##"{ layers: [
                    { image: "overhang", material: "#soil" },
                    { image: "overhang", material: "$soil" },
                ] }"##,
                "can't both be inputs",
            ),
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: {
                    texture: "soil", materials: { soil: "moss" },
                } } }"##,
                "takes `#soil` materials, and `base:moss` isn't one",
            ),
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: "soil" } }"##,
                "needs a material for `#soil`",
            ),
            (
                "textures/x.json5",
                r##"{ layers: [{ image: "{soil", material: "#soil" }] }"##,
                "has a `{` with no `}`",
            ),
            (
                "textures/x.json5",
                r##"{ layers: [{ image: "Big {soil}", material: "#soil" }] }"##,
                "Big dirt",
            ),
        ];
        for &(path, text, expected) in cases {
            let content = Content::from_mods(vec![soil_mod(&[(path, text)])]);
            let all = errors(&content);
            assert_eq!(content.errors.len(), 1, "{text}\n{all}");
            assert!(all.contains(&format!("mods/base/{path}")), "{all}");
            assert!(all.contains(expected), "expected `{expected}` in: {all}");
        }
    }

    /// Each mistake is reported against its file, and only the broken
    /// definition (and what uses it) is left out.
    #[test]
    fn mistakes_name_the_file_and_skip_only_that_definition() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "blocks/x.json5",
                r#"{ model: "quad", textures: { all: "a" }, colour: 1 }"#,
                "colour",
            ),
            (
                "blocks/x.json5",
                r#"{ model: "quad", textures: { all: "a" "#,
                "line 1",
            ),
            (
                "blocks/x.json5",
                r#"{ model: "nope", textures: { all: "a" } }"#,
                "no model `test:nope`",
            ),
            (
                "blocks/x.json5",
                r#"{ model: "quad", textures: { al: "a" } }"#,
                "has a slot `al`",
            ),
            (
                "blocks/x.json5",
                r#"{ model: "quad" }"#,
                "no texture for slot `side`",
            ),
            (
                "blocks/x.json5",
                r#"{ textures: { all: "a" } }"#,
                "no `model`",
            ),
            (
                "blocks/x.json5",
                r#"{ model: "quad", textures: { all: "zz" } }"#,
                "no texture `test:zz`",
            ),
            (
                "blocks/x.json5",
                r#"{ properties: { p: ["a", "b"] }, model: "quad", textures: { all: "{p}x" } }"#,
                "state p=a: no texture `test:ax`",
            ),
            (
                "blocks/x.json5",
                r#"{ properties: { p: ["a"] }, model: "quad", textures: { all: "a" }, variants: [{ when: { p: "c" } }] }"#,
                "`p` has no value `c`",
            ),
            (
                "blocks/x.json5",
                r#"{ properties: { p: ["a"] }, model: "quad", textures: { all: "a" }, variants: [{ when: { q: "a" } }] }"#,
                "variant 1: no property `q`",
            ),
            (
                "blocks/x.json5",
                r#"{ properties: { p: ["a", "a"] }, model: "quad", textures: { all: "a" } }"#,
                "lists a value twice",
            ),
            (
                "blocks/x.json5",
                r#"{ properties: { p: ["a", "b"] }, model: "quad", textures: { all: "a" }, variants: [{ when: { p: "b" }, textures: { al: "a" } }] }"#,
                "has a slot `al`",
            ),
            ("blocks/defaria:x.json5", "{}", "must be"),
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 12 } } }"##,
                "a fluid has no `model`",
            ),
            (
                "blocks/x.json5",
                r##"{ rotate: [0, 90, 0], textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 12 } } }"##,
                "a fluid can't be rotated",
            ),
            (
                "blocks/x.json5",
                r##"{ textures: { all: "a" }, fluid: { rate: 0, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 12 } } }"##,
                "fluid `rate` is 0; it must be above 0",
            ),
            (
                "blocks/x.json5",
                r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1.5, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 12 } } }"##,
                "fluid `fall` is 1.5; it must be from 0 to 1 block",
            ),
            (
                "blocks/x.json5",
                r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: -0.1, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 12 } } }"##,
                "fluid `film` is -0.1",
            ),
            (
                "blocks/x.json5",
                r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.001, move_speed: 0.5, view: { color: "#203060", distance: 12 } } }"##,
                "fluid `ripple` is 0.001; it must be at least 0.002",
            ),
            (
                "blocks/x.json5",
                r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 2, view: { color: "#203060", distance: 12 } } }"##,
                "fluid `move_speed` is 2",
            ),
            (
                "blocks/x.json5",
                r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 0 } } }"##,
                "fluid view `distance` is 0",
            ),
            (
                "blocks/x.json5",
                r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "blue", distance: 12 } } }"##,
                "`blue` isn't a hex color",
            ),
            (
                "blocks/x.json5",
                r##"{ textures: { all: "a" }, fluid: { rate: 10, fall: 1, film: 0.06, ripple: 0.01, move_speed: 0.5, view: { color: "#203060", distance: 12 }, depth: 3 } }"##,
                "unknown field `depth`",
            ),
            (
                "models/x.json5",
                r#"{ slots: { a: null }, faces: [{ texture: "a", vertices: [
                    { pos: [0, 0, 0], uv: [0, 0] }, { pos: [1, 0, 0], uv: [0, 0] },
                    { pos: [1, 1, 0], uv: [0, 0] }, { pos: [Infinity, 1, 0], uv: [0, 0] },
                ] }] }"#,
                "vertex 4 has a number that isn't finite",
            ),
            (
                "models/x.json5",
                r#"{ slots: { a: null }, faces: [{ texture: "a", vertices: [
                    { pos: [0, 0, 0], uv: [0, 0] }, { pos: [1, 0, 0], uv: [NaN, 0] },
                    { pos: [1, 1, 0], uv: [0, 0] },
                ] }] }"#,
                "vertex 2 has a number that isn't finite",
            ),
            (
                "blocks/x.json5",
                r#"{ properties: { p: [] }, model: "quad", textures: { all: "a" } }"#,
                "property `p`: it has no values",
            ),
            (
                "models/x.json5",
                r#"{ collision: [{ from: [0, 0, 0], to: [1, 1.5, 1] }], faces: [] }"#,
                "collision box 1: it must be inside the block",
            ),
            (
                "models/x.json5",
                r#"{ collision: [{ from: [0, 0, 0], to: [1, 1, 1] }, { from: [0, 0.5, 0], to: [1, 0.5, 1] }], faces: [] }"#,
                "collision box 2: it's flat",
            ),
            (
                "blocks/x.json5",
                r#"{ model: "quad", textures: { all: "a" }, rotate: [10, 0, 0] }"#,
                "multiples of 90",
            ),
            (
                "textures/x.json5",
                r#"{ layers: [{ image: "a" }] }"#,
                "either a `material` or a `palette`",
            ),
            (
                "textures/x.json5",
                r#"{ layers: [{ image: "a", material: "nope" }] }"#,
                "no material `test:nope`",
            ),
            (
                "textures/x.json5",
                r#"{ layers: [] }"#,
                "at least one layer",
            ),
            (
                "materials/x.json5",
                r##"{ palette: ["#000"], hardness: 1, weight: 1 }"##,
                "2 to 8 colors",
            ),
            (
                "materials/x.json5",
                r#"{ palette: ["black", "white"], hardness: 1, weight: 1 }"#,
                "`black` isn't a hex color",
            ),
            (
                "models/x.json5",
                r#"{ slots: { a: "b", b: "a" }, faces: [] }"#,
                "go round in a circle",
            ),
            (
                "models/x.json5",
                r#"{ slots: { a: "z" }, faces: [] }"#,
                "`z`, which isn't a slot",
            ),
            (
                "models/x.json5",
                r#"{ slots: { a: null }, faces: [{ texture: "a", vertices: [{ pos: [0, 0, 0], uv: [0, 0] }] }] }"#,
                "face 1: a face has 3 or 4 vertices",
            ),
            (
                "models/x.json5",
                r#"{ slots: { a: null }, faces: [{ texture: "a", vertices: [
                    { pos: [0, 0, 0], uv: [0, 0] }, { pos: [1, 0, 0], uv: [0, 0] }, { pos: [2, 0, 0], uv: [0, 0] },
                ] }] }"#,
                "lie on a line",
            ),
            (
                "models/x.json5",
                r#"{ faces: [{ texture: "b", vertices: [] }] }"#,
                "isn't one of the model's slots",
            ),
            (
                "items/x.json5",
                r#"{ block: "nope" }"#,
                "no block `test:nope`",
            ),
            ("blocks/Bad Name.json5", "{}", "must be"),
            ("stuff/x.json5", "{}", "not a definition"),
        ];
        for &(path, text, expected) in cases {
            let content = content(&[(path, text)]);
            let all = errors(&content);
            assert_eq!(content.errors.len(), 1, "{path}: {text}\n{all}");
            assert!(all.contains(&format!("mods/test/{path}")), "{all}");
            assert!(all.contains(expected), "expected `{expected}` in: {all}");
            // The good definitions around it still loaded.
            assert!(content.texture("test:a").is_some(), "{all}");
            assert!(content.models.contains_key("test:quad"), "{all}");
            assert!(content.block("test:x").is_none());
        }
    }

    #[test]
    fn a_block_with_too_many_states_is_refused() {
        let values: Vec<String> = (0..17).map(|i| format!("\"v{i}\"")).collect();
        let values = values.join(", ");
        let block = format!(
            r#"{{ properties: {{ a: [{values}], b: [{values}], c: [{values}] }}, model: "quad", textures: {{ all: "a" }} }}"#
        );
        let content = content(&[("blocks/x.json5", &block)]);
        assert!(
            errors(&content).contains("too many"),
            "{}",
            errors(&content)
        );
    }

    /// Materials for the material tests: two woods (one with a `bark`
    /// palette), a rock and two ores, plus a texture with inputs.
    fn materials(files: &[(&'static str, &'static str)]) -> Content {
        let mut all = vec![
            (
                "materials/oak.json5",
                r##"{ name: "Oak", tags: ["wood"], palette: ["#100000", "#200000"],
                     palettes: { bark: ["#300000", "#400000"] }, hardness: 1, weight: 700 }"##,
            ),
            (
                "materials/birch.json5",
                r##"{ tags: ["wood", "pale"], palette: ["#001000", "#002000"], hardness: 0.5,
                     weight: 600 }"##,
            ),
            (
                "materials/rock.json5",
                r##"{ tags: ["rock"], palette: ["#000010", "#000020"], hardness: 2, weight: 2000 }"##,
            ),
            (
                "materials/iron.json5",
                r##"{ tags: ["ore", "metal"], palette: ["#101010", "#202020"], hardness: 4,
                     weight: 8000 }"##,
            ),
            (
                "materials/coal.json5",
                r##"{ tags: ["ore"], palette: ["#050505", "#060606"], hardness: 1, weight: 1400 }"##,
            ),
            (
                "textures/bark.json5",
                r##"{ layers: [{ image: "a", material: "$wood", role: "bark" }] }"##,
            ),
            (
                "textures/ore.json5",
                r##"{ layers: [{ image: "a", material: "$rock" }, { image: "a", material: "$ore" }] }"##,
            ),
        ];
        all.extend_from_slice(files);
        content(&all)
    }

    /// `default` picks a property's default value, which comes first so the
    /// block's default state has it; the other values keep their order.
    /// Without it, the default is the first value. Placement still gives
    /// the values it names.
    #[test]
    fn default_names_the_default_value() {
        let content = materials(&[
            (
                "blocks/plank.json5",
                r##"{
                    properties: {
                        wood: { materials: "#wood", default: "oak" },
                        ore: { materials: "#ore", optional: true, default: "coal" },
                        axis: { values: ["x", "y", "z"], default: "y", place: "face" },
                        size: ["small", "big"],
                    },
                    model: "quad",
                    textures: { all: "a" },
                }"##,
            ),
            (
                "blocks/plain.json5",
                r##"{
                    properties: { ore: { materials: "#ore", optional: true } },
                    model: "quad",
                    textures: { all: "a" },
                }"##,
            ),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let plank = content.block("test:plank").unwrap();
        let values = |name: &str| {
            plank
                .properties
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .values
                .clone()
        };
        // By id, birch would come first.
        assert_eq!(values("wood"), ["test:oak", "test:birch"]);
        assert_eq!(values("ore"), ["test:coal", "none", "test:iron"]);
        assert_eq!(values("axis"), ["y", "x", "z"]);
        assert_eq!(values("size"), ["small", "big"]);
        let default = content.state(plank.default_state());
        for (name, value) in [
            ("axis", "y"),
            ("ore", "test:coal"),
            ("size", "small"),
            ("wood", "test:oak"),
        ] {
            assert!(
                default.values.contains(&(name.into(), value.into())),
                "{:?}",
                default.values
            );
        }
        // Placing on a block's east side still sets `axis` to `x`.
        let placed = content.placed(plank.default_state(), Direction::East, Vec3::NEG_X);
        assert!(
            content
                .state(placed)
                .values
                .contains(&("axis".into(), "x".into()))
        );
        // An optional material defaults to `none`.
        let plain = content.block("test:plain").unwrap();
        assert_eq!(
            content.state(plain.default_state()).values,
            [("ore".to_owned(), "none".to_owned())]
        );
    }

    #[test]
    fn a_default_that_isnt_a_value_is_an_error() {
        for (property, expected) in [
            (
                r##"{ materials: "#wood", default: "rock" }"##,
                "`test:rock` isn't one",
            ),
            (r##"{ values: ["a", "b"], default: "c" }"##, "`c` isn't one"),
            (
                r##"{ materials: "#wood", default: "none" }"##,
                "`none` isn't one",
            ),
        ] {
            let block = format!(
                r#"{{ properties: {{ p: {property} }}, model: "quad", textures: {{ all: "a" }} }}"#
            );
            let content = materials(&[("blocks/x.json5", Box::leak(block.into_boxed_str()))]);
            let all = errors(&content);
            assert!(all.contains(expected), "{property}: {all}");
            assert!(content.block("test:x").is_none());
        }
    }

    /// A material property takes its values from tags and lists, in
    /// order, without repeats; `{property}` gives the material's id in
    /// texture bindings and its name in names.
    #[test]
    fn material_properties_list_materials_and_name_states() {
        let content = materials(&[(
            "blocks/log.json5",
            r##"{
                name: "{wood} Log",
                properties: { wood: { materials: ["oak", "#wood"] } },
                model: "quad",
                textures: { all: { texture: "bark", materials: { wood: "{wood}" } } },
            }"##,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let log = content.block("test:log").unwrap();
        assert_eq!(log.properties[0].values, ["test:oak", "test:birch"]);
        assert!(log.properties[0].materials && log.properties[0].item);
        let birch = log.state(&[("wood", "test:birch")]).unwrap();
        assert_eq!(content.state(log.default_state()).name, "Oak Log");
        // A material with no name is named after its id.
        assert_eq!(content.state(birch).name, "Birch Log");
        // Each wood bakes its own tile, the oak one in its bark palette and
        // birch (with no bark palette) in its main one.
        let tile = |state| &content.textures[content.state(state).faces[0].layer as usize];
        let oak_tile = tile(log.default_state());
        assert_eq!(oak_tile.id, "test:bark(wood=test:oak)");
        assert_eq!(
            oak_tile.layers[0].palette,
            content.materials["test:oak"].palettes["bark"]
        );
        assert_eq!(
            tile(birch).layers[0].palette,
            content.materials["test:birch"].palette
        );
        // One item per wood, each placing its own.
        let item = content.item("test:log").unwrap();
        assert_eq!(item.state_count, 2);
        let birch_item = item.state(&[("wood", "test:birch")]).unwrap();
        let look = content.item_state(birch_item).unwrap();
        assert_eq!((look.name.as_str(), look.block), ("Birch Log", Some(birch)));
        assert_eq!(content.item_of(birch_item).unwrap().id, "test:log");
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

    /// Items have properties of their own, fill texture inputs from them,
    /// and pass the ones a block shares on to the state they place.
    #[test]
    fn items_carry_their_property_values() {
        let content = materials(&[
            (
                "blocks/plank.json5",
                r##"{ properties: { wood: { materials: "#wood" } }, model: "quad", textures: { all: "a" } }"##,
            ),
            (
                "items/tool.json5",
                r##"{
                    name: "{head} Tool",
                    properties: { head: { materials: "#metal" }, wood: { materials: "#wood" } },
                    texture: { texture: "ore", materials: { rock: "{wood}", ore: "{head}" } },
                    block: "plank",
                    unique: true,
                }"##,
            ),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let tool = content.item("test:tool").unwrap();
        assert_eq!(tool.state_count, 2);
        // Oak, not the default (birch comes first by id), so the value
        // has to reach the block.
        let oak = tool.state(&[("wood", "test:oak")]).unwrap();
        assert_ne!(oak, tool.default_state());
        let look = content.item_state(oak).unwrap();
        assert_eq!(look.name, "Iron Tool");
        assert_eq!(
            look.values,
            [
                ("head".into(), "test:iron".into()),
                ("wood".into(), "test:oak".into())
            ]
        );
        let plank = content.block("test:plank").unwrap();
        assert_ne!(look.block, Some(plank.default_state()));
        assert_eq!(look.block, plank.state(&[("wood", "test:oak")]));
        assert_eq!(
            look.texture,
            content.texture("test:ore(ore=test:iron,rock=test:oak)")
        );

        // An item offering a value the block doesn't have is a mistake.
        let content = materials(&[
            (
                "blocks/plank.json5",
                r##"{ properties: { wood: { materials: "birch" } }, model: "quad", textures: { all: "a" } }"##,
            ),
            (
                "items/tool.json5",
                r##"{ properties: { wood: { materials: "#wood" } }, block: "plank" }"##,
            ),
        ]);
        let all = errors(&content);
        assert!(
            all.contains("items/tool.json5")
                && all.contains("block `test:plank` has no `wood` value `test:oak`"),
            "{all}"
        );
        assert!(content.item("test:tool").is_none());
    }

    /// `item: true` puts a plain property in the block's item and
    /// `item: false` leaves a material one out; the item places the block
    /// with its values and the rest at their defaults.
    #[test]
    fn the_item_flag_picks_the_block_items_properties() {
        let content = materials(&[(
            "blocks/x.json5",
            r##"{
                properties: {
                    color: { values: ["red", "blue"], item: true },
                    wood: { materials: "#wood", item: false },
                    rock: { materials: "#rock" },
                },
                model: "quad",
                textures: { all: "a" },
            }"##,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let block = content.block("test:x").unwrap();
        let item = content.item("test:x").unwrap();
        let names: Vec<&str> = item.properties.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["color", "rock"]);
        assert_eq!(item.state_count, 2);
        let blue = item.state(&[("color", "blue")]).unwrap();
        assert_eq!(
            content.item_state(blue).unwrap().block,
            block.state(&[("color", "blue")])
        );
    }

    /// A block's item file fills `{property}` in its name and texture from
    /// each item state's values.
    #[test]
    fn a_block_items_file_fills_in_its_values() {
        let content = materials(&[
            (
                "blocks/log.json5",
                r##"{ properties: { wood: { materials: "#wood" } }, model: "quad", textures: { all: "a" } }"##,
            ),
            (
                "items/log.json5",
                r##"{ name: "{wood} Thing", texture: { texture: "bark", materials: { wood: "{wood}" } } }"##,
            ),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let item = content.item("test:log").unwrap();
        let oak = content
            .item_state(item.state(&[("wood", "test:oak")]).unwrap())
            .unwrap();
        assert_eq!(oak.name, "Oak Thing");
        assert_eq!(oak.texture, content.texture("test:bark(wood=test:oak)"));
        assert_eq!(
            content.textures[oak.texture.unwrap() as usize].template,
            "test:bark"
        );
    }

    #[test]
    fn material_mistakes_are_reported() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "blocks/x.json5",
                r##"{ properties: { w: { materials: "#nothing" } }, model: "quad", textures: { all: "a" } }"##,
                "no material has tag `nothing`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: { materials: "teak" } }, model: "quad", textures: { all: "a" } }"##,
                "no material `test:teak`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: { values: ["a"], optional: true } }, model: "quad", textures: { all: "a" } }"##,
                "`optional` is for material properties",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: { materials: "#wood" } }, model: "quad", textures: { all: "bark" } }"##,
                "texture `test:bark` needs a material for `$wood`",
            ),
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: { texture: "a", materials: { wood: "oak" } } } }"##,
                "texture `test:a` has no input `wood`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: { materials: "#wood" } }, model: "quad", textures: { all: "a" },
                     variants: [{ when: { w: "#rock" } }] }"##,
                "`w` has no value `#rock`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: ["a"] }, model: "quad", textures: { all: "a" },
                     variants: [{ when: { w: { not: "b" } } }] }"##,
                "`w` has no value `b`",
            ),
            (
                "items/x.json5",
                r##"{ properties: { w: { materials: "#wood", item: true } } }"##,
                "`item` is only for block properties",
            ),
            (
                "textures/x.json5",
                r##"{ layers: [{ image: "a", palette: ["#000", "#fff"], role: "bark" }] }"##,
                "`role` is for layers with a `material`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: ["a"] }, model: "quad", textures: { all: "a" },
                     variants: [{ when: { w: [] } }] }"##,
                "`w` lists no values",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: { materials: [] } }, model: "quad", textures: { all: "a" } }"##,
                "property `w`: it lists no materials",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: { materials: "#wood", optinal: true } }, model: "quad", textures: { all: "a" } }"##,
                "unknown field `optinal`",
            ),
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: { texture: "a", material: {} } } }"##,
                "unknown field `material`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: ["a", "b"] }, model: "quad", textures: { all: "a" },
                     variants: [{ when: { w: { not: "a", also: "b" } } }] }"##,
                "unknown field `also`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { w: "a" }, model: "quad", textures: { all: "a" } }"##,
                "a list of values, or `{ values }` or `{ materials }`",
            ),
            (
                "materials/x.json5",
                r##"{ palette: ["#000", "#fff"], palettes: { bark: ["#000"] }, hardness: 1,
                     weight: 1 }"##,
                "palette `bark`: a palette needs 2 to 8 colors",
            ),
        ];
        for &(path, text, expected) in cases {
            let content = materials(&[(path, text)]);
            let all = errors(&content);
            assert_eq!(content.errors.len(), 1, "{path}: {text}\n{all}");
            assert!(all.contains(&format!("mods/test/{path}")), "{all}");
            assert!(all.contains(expected), "expected `{expected}` in: {all}");
        }
        // A block's item takes the block's properties, not its own.
        let content = materials(&[
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: "a" } }"##,
            ),
            ("items/x.json5", r##"{ properties: { p: ["a"] } }"##),
        ]);
        assert!(
            errors(&content).contains("items/x.json5"),
            "{}",
            errors(&content)
        );
        assert!(content.item("test:x").is_none());
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

    /// Every block and item state in the base game has a hardness and a
    /// weight, worked out from its materials where it has them. Fluids,
    /// which can't be mined, needn't have a hardness.
    #[test]
    fn base_states_have_hardness_and_weight() {
        let content = crate::content::base();
        let air = content.state(BlockState::AIR);
        assert_eq!((air.hardness, air.weight), (0.0, 0.0));
        for state in &content.states[1..] {
            assert!(
                state.hardness > 0.0 || state.is_fluid(),
                "{} hardness",
                state.name
            );
            assert!(state.weight > 0.0, "{} weight", state.name);
        }
        for info in &content.item_states {
            let fluid = info
                .block
                .is_some_and(|state| content.state(state).is_fluid());
            assert!(info.hardness > 0.0 || fluid, "{} hardness", info.name);
            assert!(info.weight > 0.0, "{} weight", info.name);
            if let Some(state) = info.block {
                let placed = content.state(state);
                assert_eq!(
                    (info.hardness, info.weight),
                    (placed.hardness, placed.weight)
                );
            }
        }
        let stone = content.block("defaria:stone").unwrap();
        let plain = content.state(stone.default_state());
        assert_eq!((plain.hardness, plain.weight), (1.5, 2600.0));
        // Rock counts three times as much as ore.
        let ore = content
            .state(stone.state(&[("ore", "defaria:iron")]).unwrap())
            .hardness;
        assert_eq!(ore, (3.0 * 1.5 + 3.0) / 4.0);
        let slab = content.state(content.default_state("defaria:stone_slab").unwrap());
        assert_eq!((slab.hardness, slab.weight), (1.5, 1300.0));
        let soil = content.block("defaria:soil").unwrap();
        let dirt = content.state(soil.default_state());
        assert_eq!(dirt.hardness, 0.5);
        // Soil counts three times as much as turf.
        let grass = content.state(soil.state(&[("turf", "defaria:grass")]).unwrap());
        assert_eq!(grass.hardness, (3.0 * 0.5 + 0.6) / 4.0);
        assert_eq!(grass.weight, (3.0 * 1500.0 + 1400.0) / 4.0);
        // An iron pickaxe on an oak handle weighs a kilogram or two.
        let pickaxe = content.default_item_info("defaria:pickaxe").unwrap();
        assert!((1.0..3.0).contains(&pickaxe.weight), "{}", pickaxe.weight);
    }

    /// A hardness or weight is a number, or a base times the weighted
    /// average of the materials' values, skipping `none`. Left out, it's 0,
    /// and an item that places a block takes the block's.
    #[test]
    fn hardness_and_weight_mix_materials() {
        let content = materials(&[
            (
                "blocks/rock.json5",
                r##"{
                    properties: { rock: { materials: "#rock" }, ore: { materials: "iron", optional: true } },
                    model: "quad", textures: { all: "a" },
                    hardness: { materials: { rock: 3, ore: 1 } },
                    weight: { base: 0.5, materials: { rock: 1 } },
                }"##,
            ),
            (
                "blocks/plain.json5",
                r##"{ model: "quad", textures: { all: "a" }, hardness: 0.25, weight: 10 }"##,
            ),
            (
                "blocks/nothing.json5",
                r##"{ model: "quad", textures: { all: "a" } }"##,
            ),
            // Changes its block's item's weight only.
            ("items/plain.json5", r##"{ weight: 3 }"##),
            (
                "items/tool.json5",
                r##"{
                    properties: { head: { materials: "iron" }, handle: { materials: "oak" } },
                    hardness: { materials: { head: 1 } },
                    weight: { base: 0.001, materials: { head: 1, handle: 3 } },
                }"##,
            ),
            ("items/brick.json5", r##"{ block: "plain" }"##),
            ("items/stick.json5", r##"{}"##),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let rock = content.block("test:rock").unwrap();
        let plain = content.state(rock.default_state());
        // `ore` is `none`, so only the rock counts.
        assert_eq!((plain.hardness, plain.weight), (2.0, 1000.0));
        let ore = content.state(rock.state(&[("ore", "test:iron")]).unwrap());
        assert_eq!(
            (ore.hardness, ore.weight),
            ((3.0 * 2.0 + 4.0) / 4.0, 1000.0)
        );

        let fixed = content.state(content.default_state("test:plain").unwrap());
        assert_eq!((fixed.hardness, fixed.weight), (0.25, 10.0));
        let nothing = content.state(content.default_state("test:nothing").unwrap());
        assert_eq!((nothing.hardness, nothing.weight), (0.0, 0.0));

        let info = |id: &str| content.default_item_info(id).unwrap();
        let both = |id: &str| (info(id).hardness, info(id).weight);
        // The block's item: the block's hardness, the file's weight.
        assert_eq!(both("test:plain"), (0.25, 3.0));
        let rock_item = content.item("test:rock").unwrap();
        let ore_item = rock_item.state(&[("ore", "test:iron")]).unwrap();
        assert_eq!(content.item_state(ore_item).unwrap().hardness, ore.hardness);
        let tool = info("test:tool");
        assert_eq!(tool.hardness, 4.0);
        assert!((tool.weight - 0.001 * (8000.0 + 3.0 * 700.0) / 4.0).abs() < 1e-4);
        // An item that places a block is as hard and heavy as it.
        assert_eq!(both("test:brick"), (0.25, 10.0));
        assert_eq!(both("test:stick"), (0.0, 0.0));
    }

    /// A block's item file counts the item's own properties: here only
    /// `wood`, as the plain `axis` before it isn't the item's.
    #[test]
    fn a_block_items_file_mixes_the_items_materials() {
        let content = materials(&[
            (
                "blocks/log.json5",
                r##"{
                    properties: { axis: ["x", "y", "z"], wood: { materials: "#wood" } },
                    model: "quad", textures: { all: "a" }, hardness: 9, weight: 9,
                }"##,
            ),
            (
                "items/log.json5",
                r##"{
                    hardness: { materials: { wood: 1 } },
                    weight: { base: 0.5, materials: { wood: 1 } },
                }"##,
            ),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let item = content.item("test:log").unwrap();
        assert_eq!(item.state_count, 2);
        let both = |wood: &str| {
            let info = content
                .item_state(item.state(&[("wood", wood)]).unwrap())
                .unwrap();
            (info.hardness, info.weight)
        };
        assert_eq!(both("test:oak"), (1.0, 350.0));
        assert_eq!(both("test:birch"), (0.5, 300.0));
    }

    /// A material without a hardness or weight is its own file's error;
    /// blocks counting its tag load without it.
    #[test]
    fn a_material_missing_hardness_fails_alone() {
        let content = materials(&[
            (
                "materials/marble.json5",
                r##"{ tags: ["rock"], palette: ["#000", "#fff"], weight: 2700 }"##,
            ),
            (
                "blocks/stone.json5",
                r##"{ properties: { rock: { materials: "#rock" } }, model: "quad", textures: { all: "a" },
                     hardness: { materials: { rock: 1 } } }"##,
            ),
        ]);
        let all = errors(&content);
        assert_eq!(content.errors.len(), 1, "{all}");
        assert!(all.contains("mods/test/materials/marble.json5"), "{all}");
        assert!(all.contains("missing field `hardness`"), "{all}");
        let stone = content.block("test:stone").unwrap();
        assert_eq!(stone.state_count, 1);
        assert_eq!(content.state(stone.default_state()).hardness, 2.0);
    }

    #[test]
    fn hardness_and_weight_mistakes_are_reported() {
        let cases: &[(&str, &str, &str)] = &[
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: "a" }, hardness: { materials: { wood: 1 } } }"##,
                "`hardness` counts `wood`, which isn't a property",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { axis: ["x"] }, model: "quad", textures: { all: "a" },
                     weight: { materials: { axis: 1 } } }"##,
                "`weight` counts `axis`, which isn't a material property",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { wood: { materials: "oak" } }, model: "quad", textures: { all: "a" },
                     hardness: { materials: { wood: 0 } } }"##,
                "a share must be more than 0",
            ),
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: "a" }, hardness: { materials: {} } }"##,
                "`hardness` has no `materials`",
            ),
            // `base` alone is a multiplier, not a weight.
            (
                "items/x.json5",
                r##"{ properties: { head: { materials: "iron", optional: true },
                                    handle: { materials: "oak", optional: true } },
                     weight: { base: 0.0007, materials: { head: 1, handle: 3 } } }"##,
                "`weight`: in states with handle=none,head=none, none of its counted properties has a material",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { ore: { materials: "#ore", optional: true } },
                     model: "quad", textures: { all: "a" }, hardness: { materials: { ore: 1 } } }"##,
                "`hardness`: in states with ore=none, none of its counted properties has a material",
            ),
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: "a" }, weight: -1 }"##,
                "`weight` must be a number of at least 0",
            ),
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: "a" }, weight: "heavy" }"##,
                "a number, or `{ base, stat, materials, default }`",
            ),
            (
                "materials/x.json5",
                r##"{ palette: ["#000", "#fff"], hardness: -0.5, weight: 1 }"##,
                "`hardness` must be a number of at least 0",
            ),
            // Required, so the error is the material's, not its users'.
            (
                "materials/x.json5",
                r##"{ tags: ["rock"], palette: ["#000", "#fff"], weight: 1 }"##,
                "missing field `hardness`",
            ),
            (
                "materials/x.json5",
                r##"{ tags: ["rock"], palette: ["#000", "#fff"], hardness: 1 }"##,
                "missing field `weight`",
            ),
            // An integer where a number isn't one of the forms.
            (
                "blocks/x.json5",
                r##"{ properties: { axis: ["x", "y"] }, model: "quad", textures: { all: "a" },
                     variants: [{ when: { axis: 1 }, rotate: [90, 0, 0] }] }"##,
                "invalid type: integer `1`",
            ),
        ];
        for &(path, text, expected) in cases {
            let content = materials(&[(path, text)]);
            let all = errors(&content);
            assert_eq!(content.errors.len(), 1, "{path}: {text}\n{all}");
            assert!(all.contains(&format!("mods/test/{path}")), "{all}");
            assert!(all.contains(expected), "expected `{expected}` in: {all}");
        }
    }

    /// A block's item file counting a property the block keeps off the
    /// item says so, rather than that it isn't a property at all.
    #[test]
    fn an_item_counting_a_block_only_property_is_told_why() {
        let content = materials(&[
            (
                "blocks/plank.json5",
                r##"{ properties: { wood: { materials: "#wood", item: false } },
                     model: "quad", textures: { all: "a" } }"##,
            ),
            (
                "items/plank.json5",
                r##"{ hardness: { materials: { wood: 1 } } }"##,
            ),
        ]);
        let all = errors(&content);
        assert_eq!(content.errors.len(), 1, "{all}");
        assert!(all.contains("mods/test/items/plank.json5"), "{all}");
        assert!(
            all.contains(
                "`hardness` counts `wood`, which isn't one of the item's properties: \
                 the block marks it `item: false`"
            ),
            "{all}"
        );
    }

    /// Materials with stats, on top of [`materials`]: copper and gold
    /// conduct, iron (from `materials`) doesn't say.
    fn stat_materials(files: &[(&'static str, &'static str)]) -> Content {
        let mut all = vec![
            (
                "materials/copper.json5",
                r##"{ tags: ["metal"], palette: ["#300000", "#400000"], hardness: 2, weight: 9000,
                     stats: { conductivity: 1, max_temperature: 1358 } }"##,
            ),
            (
                "materials/gold.json5",
                r##"{ tags: ["metal"], palette: ["#303000", "#404000"], hardness: 1, weight: 19000,
                     stats: { conductivity: 0.6, max_temperature: 1337 } }"##,
            ),
        ];
        all.extend_from_slice(files);
        materials(&all)
    }

    /// An attribute reads the stat it names, or the stat named like its
    /// field, averaged over its materials by share. Block states and their
    /// items get their `stats` worked out at load.
    #[test]
    fn attributes_read_material_stats() {
        let content = stat_materials(&[(
            "blocks/wire.json5",
            r##"{
                    properties: { core: { materials: ["copper", "gold"] }, coat: { materials: ["gold", "copper"] } },
                    model: "quad", textures: { all: "a" },
                    // Hardness from another stat, to show any field can.
                    hardness: { base: 2, stat: "conductivity", materials: { core: 1 } },
                    stats: {
                        capacity: { base: 500, stat: "conductivity", materials: { core: 3, coat: 1 } },
                        max_temperature: { materials: { core: 1, coat: 1 } },
                        size: 0.25,
                    },
                }"##,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let wire = content.block("test:wire").unwrap();
        let state = |core: &str, coat: &str| {
            content.state(wire.state(&[("core", core), ("coat", coat)]).unwrap())
        };
        let copper = state("test:copper", "test:copper");
        assert_eq!(copper.hardness, 2.0);
        assert_eq!(copper.stat("capacity"), Some(500.0));
        assert_eq!(copper.stat("max_temperature"), Some(1358.0));
        assert_eq!(copper.stat("size"), Some(0.25));
        assert_eq!(copper.stat("conductivity"), None);
        // Three parts copper to one of gold.
        let mixed = state("test:copper", "test:gold");
        let capacity = mixed.stat("capacity").unwrap();
        assert!(
            (capacity - 500.0 * (3.0 + 0.6) / 4.0).abs() < 1e-3,
            "{capacity}"
        );
        assert_eq!(mixed.stat("max_temperature"), Some((1358.0 + 1337.0) / 2.0));
        assert!((state("test:gold", "test:copper").hardness - 1.2).abs() < 1e-6);

        // The block's item has the stats of the state it places.
        let item = content.item("test:wire").unwrap();
        let info = content
            .item_state(
                item.state(&[("core", "test:copper"), ("coat", "test:gold")])
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(info.stat("capacity"), Some(capacity));
    }

    /// An item's own `stats` add to and replace those of the block it
    /// places.
    #[test]
    fn item_stats_replace_the_blocks() {
        let content = stat_materials(&[
            (
                "blocks/wire.json5",
                r##"{ properties: { metal: { materials: ["copper", "gold"] } },
                     model: "quad", textures: { all: "a" },
                     stats: { capacity: 10, length: 1 } }"##,
            ),
            (
                "items/wire.json5",
                r##"{ stats: { capacity: { base: 5, stat: "conductivity", materials: { metal: 1 } } } }"##,
            ),
            (
                "items/spool.json5",
                r##"{ block: "wire", stats: { length: 50 } }"##,
            ),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let item = content.item("test:wire").unwrap();
        let gold = content
            .item_state(item.state(&[("metal", "test:gold")]).unwrap())
            .unwrap();
        assert_eq!(gold.stat("capacity"), Some(3.0));
        assert_eq!(gold.stat("length"), Some(1.0));
        let spool = content.default_item_info("test:spool").unwrap();
        assert_eq!(
            (spool.stat("capacity"), spool.stat("length")),
            (Some(10.0), Some(50.0))
        );
    }

    /// `hardness` and `weight` read as stats of blocks and items too, as
    /// they do of materials.
    #[test]
    fn hardness_and_weight_are_stats() {
        let content = crate::content::base();
        let stone = content.state(content.default_state("defaria:stone").unwrap());
        assert_eq!(stone.stat("hardness"), Some(stone.hardness));
        assert_eq!(stone.stat("weight"), Some(stone.weight));
        let item = content.default_item_info("defaria:stone").unwrap();
        assert_eq!(item.stat("hardness"), Some(item.hardness));
        assert_eq!(item.stat("weight"), Some(item.weight));
    }

    /// A material without the stat counts as the attribute's `default`;
    /// without one, it's an error naming the material and the stat.
    #[test]
    fn a_missing_stat_takes_the_default() {
        let content = stat_materials(&[(
            "blocks/wire.json5",
            r##"{ properties: { metal: { materials: "#metal" } }, model: "quad", textures: { all: "a" },
                 stats: { conductivity: { materials: { metal: 1 }, default: 0.1 } } }"##,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let wire = content.block("test:wire").unwrap();
        let conductivity = |metal: &str| {
            content
                .state(wire.state(&[("metal", metal)]).unwrap())
                .stat("conductivity")
        };
        assert_eq!(conductivity("test:iron"), Some(0.1));
        assert_eq!(conductivity("test:gold"), Some(0.6));

        let content = stat_materials(&[(
            "blocks/wire.json5",
            r##"{ properties: { metal: { materials: "#metal" } }, model: "quad", textures: { all: "a" },
                 stats: { conductivity: { materials: { metal: 1 } } } }"##,
        )]);
        let all = errors(&content);
        assert_eq!(content.errors.len(), 1, "{all}");
        assert!(all.contains("mods/test/blocks/wire.json5"), "{all}");
        assert!(
            all.contains(
                "`stats.conductivity` averages stat `conductivity` of `metal`, but material \
                 `test:iron` has no `conductivity`"
            ),
            "{all}"
        );
        assert!(content.block("test:wire").is_none());
    }

    #[test]
    fn stat_mistakes_are_reported() {
        let cases: &[(&str, &str, &str)] = &[
            // No material has it: a misspelling, most likely.
            (
                "blocks/x.json5",
                r##"{ properties: { metal: { materials: "copper" } }, model: "quad", textures: { all: "a" },
                     stats: { capacity: { stat: "conductivty", materials: { metal: 1 } } } }"##,
                "`stats.capacity` averages stat `conductivty`, which no material has",
            ),
            // Named like the field, by default.
            (
                "items/x.json5",
                r##"{ properties: { metal: { materials: "copper" } },
                     stats: { capacity: { materials: { metal: 1 } } } }"##,
                "`stats.capacity` averages stat `capacity`, which no material has",
            ),
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: "a" }, stats: { Capacity: 1 } }"##,
                "stat `Capacity` must be lowercase letters, digits and `_` only",
            ),
            (
                "blocks/x.json5",
                r##"{ model: "quad", textures: { all: "a" }, stats: { weight: 1 } }"##,
                "`weight` is a field of its own, not one of the `stats`",
            ),
            (
                "blocks/x.json5",
                r##"{ properties: { metal: { materials: "copper" } }, model: "quad", textures: { all: "a" },
                     stats: { conductivity: { materials: { metal: 1 }, default: -1 } } }"##,
                "`stats.conductivity` `default` must be a number of at least 0",
            ),
            (
                "materials/x.json5",
                r##"{ palette: ["#000", "#fff"], hardness: 1, weight: 1, stats: { hardness: 2 } }"##,
                "`hardness` is a field of its own, not one of the `stats`",
            ),
            (
                "materials/x.json5",
                r##"{ palette: ["#000", "#fff"], hardness: 1, weight: 1, stats: { "max-temp": 2 } }"##,
                "stat `max-temp` must be lowercase letters, digits and `_` only",
            ),
            (
                "materials/x.json5",
                r##"{ palette: ["#000", "#fff"], hardness: 1, weight: 1, stats: { conductivity: -2 } }"##,
                "`conductivity` must be a number of at least 0",
            ),
        ];
        for &(path, text, expected) in cases {
            let content = stat_materials(&[(path, text)]);
            let all = errors(&content);
            assert_eq!(content.errors.len(), 1, "{path}: {text}\n{all}");
            assert!(all.contains(&format!("mods/test/{path}")), "{all}");
            assert!(all.contains(expected), "expected `{expected}` in: {all}");
        }
    }

    /// The base game's metals and woods have the stats wires and pipes
    /// read, and they compare as they should.
    #[test]
    fn base_materials_have_conduit_stats() {
        let content = crate::content::base();
        let stat = |material: &str, name: &str| {
            content.materials[&format!("defaria:{material}")]
                .stat(name)
                .unwrap_or_else(|| panic!("{material} has no {name}"))
        };
        for metal in ["copper", "gold", "iron"] {
            assert!(stat(metal, "conductivity") > 0.0, "{metal}");
        }
        for material in ["copper", "gold", "iron", "oak", "birch", "spruce"] {
            assert!(stat(material, "throughput") > 0.0, "{material}");
            assert!(stat(material, "max_temperature") > 0.0, "{material}");
        }
        assert!(stat("copper", "conductivity") > stat("gold", "conductivity"));
        assert!(stat("gold", "conductivity") > stat("iron", "conductivity"));
        // Iron pipes carry what melts copper; wood burns well below either.
        assert!(stat("iron", "max_temperature") > stat("copper", "max_temperature"));
        assert!(stat("oak", "max_temperature") < stat("copper", "max_temperature"));
        assert!(stat("oak", "throughput") < stat("iron", "throughput"));
    }

    /// A plate over the block's north side, covering it, with a box to
    /// bump into.
    const PLATE: &str = r#"{
        slots: { all: null },
        occludes: ["north"],
        collision: [{ from: [0, 0, 0], to: [1, 1, 0.125] }],
        faces: [{
            texture: "all", cull: "north",
            vertices: [
                { pos: [1, 0, 0], uv: [0, 1] },
                { pos: [0, 0, 0], uv: [1, 1] },
                { pos: [0, 1, 0], uv: [1, 0] },
                { pos: [1, 1, 0], uv: [0, 0] },
            ],
        }],
    }"#;

    /// A small box in the middle, drawn as one face on its top.
    const CORE: &str = r#"{
        slots: { all: null },
        collision: [{ from: [0.25, 0.25, 0.25], to: [0.75, 0.75, 0.75] }],
        faces: [{
            texture: "all",
            vertices: [
                { pos: [0.25, 0.75, 0.75], uv: [0, 1] },
                { pos: [0.75, 0.75, 0.75], uv: [1, 1] },
                { pos: [0.75, 0.75, 0.25], uv: [1, 0] },
                { pos: [0.25, 0.75, 0.25], uv: [0, 0] },
            ],
        }],
    }"#;

    /// A block of a core and a plate on each side its properties say.
    fn parts_mod(extra: &[(&str, &str)]) -> Content {
        let mut files = vec![
            ("models/plate.json5", PLATE),
            ("models/core.json5", CORE),
            (
                "blocks/boxy.json5",
                r#"{
                    properties: { north: ["false", "true"], south: ["false", "true"] },
                    textures: { all: "a" },
                    parts: [
                        { model: "core" },
                        { when: { north: "true" }, model: "plate" },
                        { when: { south: "true" }, model: "plate", rotate: [0, 180, 0] },
                    ],
                }"#,
            ),
        ];
        files.extend_from_slice(extra);
        content(&files)
    }

    fn boxy<'c>(content: &'c Content, north: &str, south: &str) -> (BlockState, &'c State) {
        let block = content.block("test:boxy").unwrap();
        let state = block.state(&[("north", north), ("south", south)]).unwrap();
        (state, content.state(state))
    }

    #[test]
    fn parts_add_to_the_states_own_model_only_where_they_match() {
        let content = parts_mod(&[(
            "blocks/flag.json5",
            r#"{
                properties: { flag: ["false", "true"] },
                model: "quad",
                textures: { all: "a" },
                parts: [{ when: { flag: "true" }, model: "plate" }],
            }"#,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let block = content.block("test:flag").unwrap();
        // No part matches: just the block's own model.
        let bare = content.state(block.state(&[("flag", "false")]).unwrap());
        assert_eq!(bare.parts.len(), 1);
        assert_eq!(bare.parts[0].model, "test:quad");
        assert_eq!(bare.faces.len(), 1);
        assert_eq!(bare.faces[0].cull, Some(Direction::Up));
        // The block's own model comes first, then the part.
        let flagged = content.state(block.state(&[("flag", "true")]).unwrap());
        let models: Vec<&str> = flagged.parts.iter().map(|p| p.model.as_str()).collect();
        assert_eq!(models, ["test:quad", "test:plate"]);
        assert_eq!(flagged.faces.len(), 2);
        assert_eq!(flagged.faces[0], bare.faces[0]);
        assert_eq!(flagged.parts[1].faces, 1..2);
        assert_eq!(flagged.faces[1].cull, Some(Direction::North));
    }

    #[test]
    fn every_matching_part_is_added_turned_as_it_says() {
        let content = parts_mod(&[]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let (_, core) = boxy(&content, "false", "false");
        assert_eq!(core.parts.len(), 1);
        assert_eq!(core.faces.len(), 1);

        let (_, both) = boxy(&content, "true", "true");
        let parts: Vec<(&str, [i32; 3])> = both
            .parts
            .iter()
            .map(|p| (p.model.as_str(), p.rotate))
            .collect();
        assert_eq!(
            parts,
            [
                ("test:core", [0, 0, 0]),
                ("test:plate", [0, 0, 0]),
                ("test:plate", [0, 180, 0]),
            ]
        );
        assert_eq!(both.faces.len(), 3);
        let south = &both.faces[both.parts[2].faces.clone()][0];
        assert_eq!(south.cull, Some(Direction::South));
        assert_eq!(south.normal, Vec3::Z);
        assert!(south.vertices.iter().all(|v| v.pos.z == 1.0));
    }

    #[test]
    fn covered_sides_and_culling_come_from_the_parts() {
        let content = parts_mod(&[]);
        let (core_state, core) = boxy(&content, "false", "false");
        assert!(Direction::ALL.iter().all(|&d| !core.occludes(d)));
        let (north_state, north) = boxy(&content, "true", "false");
        assert!(north.occludes(Direction::North));
        assert!(!north.occludes(Direction::South));
        assert_eq!(north.parts[1].occludes, Direction::North.bit());
        let (both_state, both) = boxy(&content, "true", "true");
        assert!(both.occludes(Direction::North) && both.occludes(Direction::South));
        assert!(!both.occludes(Direction::Up));
        assert!(!both.is_opaque());

        // A neighbour to the north hides a face against it only when one of
        // its parts covers its south side.
        assert!(!content.hides(core_state, north_state, Direction::North));
        assert!(content.hides(core_state, both_state, Direction::North));
        // The plate's face is hidden by a neighbour covering that side.
        assert!(content.hides(north_state, both_state, Direction::North));
        assert!(!content.hides(north_state, core_state, Direction::North));
    }

    #[test]
    fn collision_is_every_parts_boxes() {
        let content = parts_mod(&[]);
        let core = BlockBox {
            min: Vec3::splat(0.25),
            max: Vec3::splat(0.75),
        };
        let north = BlockBox {
            min: Vec3::ZERO,
            max: Vec3::new(1.0, 1.0, 0.125),
        };
        let south = BlockBox {
            min: Vec3::new(0.0, 0.0, 0.875),
            max: Vec3::ONE,
        };
        assert_eq!(boxy(&content, "false", "false").1.collision, [core]);
        assert_eq!(boxy(&content, "false", "true").1.collision, [core, south]);
        let (_, both) = boxy(&content, "true", "true");
        assert_eq!(both.collision, [core, north, south]);
        assert_eq!(both.parts[2].collision, 2..3);
    }

    #[test]
    fn a_parts_textures_go_over_the_blocks_for_that_part() {
        let content = parts_mod(&[(
            "blocks/two_tone.json5",
            r#"{
                textures: { all: "a" },
                parts: [{ model: "core" }, { model: "plate", textures: { all: "b" } }],
            }"#,
        )]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let state = content.state(content.default_state("test:two_tone").unwrap());
        let layer = |id: &str| content.texture(id).unwrap();
        assert_eq!(state.faces[0].layer, layer("test:a"));
        assert_eq!(state.faces[1].layer, layer("test:b"));
    }

    #[test]
    fn part_mistakes_are_reported() {
        let cases = [
            (
                r#"{ textures: { all: "a" }, parts: [{ when: { open: "true" }, model: "core" }] }"#,
                "part 1: no property `open`",
            ),
            (
                r#"{ textures: { all: "a" }, parts: [{ model: "core" }, { model: "nothing" }] }"#,
                "part 2: no model `test:nothing`",
            ),
            (
                r#"{ textures: { all: "a" }, parts: [{ model: "core", textures: { side: "b" } }] }"#,
                "part 1: model `test:core` has no slot `side`",
            ),
            (
                r#"{ textures: { all: "a" }, parts: [{ model: "core", rotate: [45, 0, 0] }] }"#,
                "multiples of 90",
            ),
            (r#"{ textures: { all: "a" } }"#, "no `model`"),
        ];
        for (block, error) in cases {
            let content = parts_mod(&[("blocks/broken.json5", block)]);
            let errors = errors(&content);
            assert!(
                errors.contains("broken.json5") && errors.contains(error),
                "{block}: expected `{error}`, got:\n{errors}"
            );
            assert!(content.default_state("test:broken").is_none());
        }
    }

    #[test]
    fn the_base_pipe_has_an_arm_for_each_side_it_says() {
        let content = crate::content::base();
        let pipe = content.block("defaria:pipe").unwrap();
        let core = content.state(pipe.default_state());
        let models: Vec<&str> = core.parts.iter().map(|p| p.model.as_str()).collect();
        assert_eq!(models, ["defaria:pipe_core"]);
        assert!(Direction::ALL.iter().all(|&d| !core.occludes(d)));

        for side in Direction::ALL {
            let state = pipe.state(&[(side.name(), "true")]).unwrap();
            let state = content.state(state);
            assert_eq!(state.parts.len(), 2, "{side:?}");
            let arm = &state.parts[1];
            assert_eq!(arm.model, "defaria:pipe_arm");
            // The arm's end is on that side, and hidden by what covers it.
            let ends: Vec<_> = state.faces[arm.faces.clone()]
                .iter()
                .filter_map(|f| f.cull)
                .collect();
            assert_eq!(ends, [side], "{side:?}");
            // Its box reaches that side of the block from the core.
            let reach = state.collision[arm.collision.clone()][0];
            let n = side.normal().as_vec3();
            let far = if n.max_element() > 0.0 {
                reach.max
            } else {
                reach.min
            };
            assert_eq!((far - Vec3::splat(0.5)).dot(n), 0.5, "{side:?}");
        }

        let all: Vec<(&str, &str)> = Direction::ALL.iter().map(|d| (d.name(), "true")).collect();
        let all = content.state(pipe.state(&all).unwrap());
        assert_eq!(all.parts.len(), 7);
        assert_eq!(all.collision.len(), 7);
    }
}
