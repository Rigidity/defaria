//! Parses the definition files and resolves the references between them,
//! in dependency order: materials, textures, models, blocks, items.
//!
//! Each definition that fails is reported once, naming its file, and left
//! out; definitions referring to it then fail with "no such …".
//!
//! Textures with `$inputs` are templates: each combination of materials a
//! block state or item uses is baked into its own tile, once, when the
//! first user asks for it.

use std::collections::{BTreeMap, BTreeSet};

use bevy::{platform::collections::HashMap, prelude::*};
use serde::de::DeserializeOwned;

use super::{
    AIR, BakedFace, Block, BlockBox, BlockState, Content, Direction, Item, ItemStateInfo,
    MAX_LIGHT, Material, ModInfo, Model, ModelFace, NONE, Property, State, Texture, TextureLayer,
    Vertex, index_of, load::LoadedMod, qualify, schema::*, state_count,
};
use crate::block_texture::Palette;

/// The kinds of definition, each in its own folder.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Materials,
    Textures,
    Models,
    Blocks,
    Items,
}

impl Kind {
    const ALL: [Kind; 5] = [
        Kind::Materials,
        Kind::Textures,
        Kind::Models,
        Kind::Blocks,
        Kind::Items,
    ];

    fn folder(self) -> &'static str {
        match self {
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
/// Item stack size when nothing says otherwise.
const DEFAULT_MAX_STACK: u32 = 64;

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
        }],
        states: vec![State {
            name: "Air".into(),
            ..default()
        }],
        items: Vec::new(),
        item_states: Vec::new(),
        errors: Vec::new(),
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
    for def in &defs.materials {
        let result = material(def).map(|material| {
            content.materials.insert(def.id.clone(), material);
        });
        report(&def.file, result);
    }
    let mut textures = Textures::default();
    for def in &defs.textures {
        match template(def, &content, &images, &dirs) {
            Ok((template, problems)) => {
                for problem in problems {
                    report(&def.file, Err(problem));
                }
                // Textures without inputs are baked now, so every one of
                // them is checked and has a tile, used or not.
                let plain = template.inputs.is_empty();
                textures.templates.insert(def.id.clone(), template);
                if plain {
                    let result = textures.bake(&def.id, &BTreeMap::new(), &mut content);
                    report(&def.file, result.map(|_| ()));
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
        Kind::Materials => defs.materials.push(def(id, namespace, file, text)?),
        Kind::Textures => defs.textures.push(def(id, namespace, file, text)?),
        Kind::Models => defs.models.push(def(id, namespace, file, text)?),
        Kind::Blocks => defs.blocks.push(def(id, namespace, file, text)?),
        Kind::Items => defs.items.push(def(id, namespace, file, text)?),
    }
    Ok(())
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
    Ok(Material {
        id: def.id.clone(),
        name: material.name.clone().unwrap_or_else(|| title_case(&def.id)),
        tags: material.tags.clone(),
        palette: palette(&material.palette)?,
        palettes,
    })
}

/// A texture file, checked, with its inputs still to fill.
struct Template {
    layers: Vec<LayerTemplate>,
    /// The `$inputs` its layers use.
    inputs: BTreeSet<String>,
}

struct LayerTemplate {
    /// The image's asset path, relative to `assets/`.
    image: String,
    color: LayerColor,
}

enum LayerColor {
    Palette(Palette),
    Material { id: String, role: Option<String> },
    Input { name: String, role: Option<String> },
}

/// A texture file as a template, and problems that don't stop it loading:
/// an image file that isn't there is reported, and the texture shows the
/// placeholder tile (see `block_texture::build`) until it is.
fn template(
    def: &Def<TextureDef>,
    content: &Content,
    images: &BTreeSet<String>,
    dirs: &HashMap<String, String>,
) -> Result<(Template, Vec<String>), String> {
    if def.def.layers.is_empty() {
        return Err("a texture needs at least one layer".into());
    }
    let mut problems = Vec::new();
    let mut layers = Vec::new();
    for (i, layer) in def.def.layers.iter().enumerate() {
        let layer = layer_template(layer, &def.namespace, content, dirs)
            .map_err(|error| format!("layer {}: {error}", i + 1))?;
        if !images.contains(&layer.image) {
            problems.push(format!(
                "layer {}: there's no {}; the texture shows a placeholder",
                i + 1,
                layer.image
            ));
        }
        layers.push(layer);
    }
    let inputs = layers
        .iter()
        .filter_map(|layer| match &layer.color {
            LayerColor::Input { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    Ok((Template { layers, inputs }, problems))
}

fn layer_template(
    layer: &LayerDef,
    namespace: &str,
    content: &Content,
    dirs: &HashMap<String, String>,
) -> Result<LayerTemplate, String> {
    let image_id = qualify(&layer.image, namespace)?;
    let (image_mod, name) = image_id.split_once(':').unwrap();
    let dir = dirs
        .get(image_mod)
        .ok_or_else(|| format!("image `{image_id}`: no mod `{image_mod}` is loaded"))?;
    let image = format!("{dir}/images/{name}.png");
    let role = layer.role.clone();
    let color = match (&layer.material, &layer.palette) {
        (Some(material), None) => match material.strip_prefix('$') {
            Some(input) => {
                super::check_name(input).map_err(|error| format!("input {error}"))?;
                LayerColor::Input {
                    name: input.into(),
                    role,
                }
            }
            None => {
                let id = qualify(material, namespace)?;
                if !content.materials.contains_key(&id) {
                    return Err(format!("no material `{id}`"));
                }
                LayerColor::Material { id, role }
            }
        },
        (None, Some(colors)) if role.is_none() => LayerColor::Palette(palette(colors)?),
        (None, Some(_)) => return Err("`role` is for layers with a `material`".into()),
        _ => return Err("give a layer either a `material` or a `palette`".into()),
    };
    Ok(LayerTemplate { image, color })
}

/// The texture templates, and the tiles baked from them so far (in
/// [`Content::textures`]).
#[derive(Default)]
struct Textures {
    templates: HashMap<String, Template>,
}

impl Textures {
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
        if let Some(input) = template.inputs.iter().find(|i| !materials.contains_key(*i)) {
            return Err(format!("texture `{id}` needs a material for `${input}`"));
        }
        if let Some(input) = materials.keys().find(|i| !template.inputs.contains(*i)) {
            return Err(format!("texture `{id}` has no input `${input}`"));
        }
        let layers = template
            .layers
            .iter()
            .map(|layer| {
                let (material, role) = match &layer.color {
                    LayerColor::Palette(palette) => {
                        return Ok(TextureLayer {
                            image: layer.image.clone(),
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
                    image: layer.image.clone(),
                    palette: material.palette_for(role.as_deref()),
                })
            })
            .collect::<Result<_, String>>()?;
        let layer = content.textures.len() as u32;
        content.textures.push(Texture {
            id: key.clone(),
            template: id.into(),
            layers,
        });
        content.texture_index.insert(key, layer);
        Ok(layer)
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
    defs.iter()
        .map(|(name, def)| {
            property(name, def, namespace, content, for_block)
                .map_err(|error| format!("property `{name}`: {error}"))
        })
        .collect()
}

fn property(
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
    match (&spec.values, &spec.materials) {
        (Some(values), None) if !spec.optional => {
            plain_property(name, values, spec.item.unwrap_or(false))
        }
        (Some(_), None) => Err("`optional` is for material properties".into()),
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

    // Each state's property values, look and model, last property fastest.
    let mut looks = Vec::with_capacity(count);
    for (index, values) in combinations(&properties) {
        let look = look(block, &whens, &index);
        let model = look
            .model(&values, &def.namespace, content)
            .map_err(|error| in_state(&values, error))?
            .clone();
        looks.push((index, values, look, model));
    }
    // A slot is skipped where the state's model doesn't have it, so a
    // variant can switch to a model with other slots; a slot no model has
    // is a mistake.
    let slots: BTreeSet<&str> = looks
        .iter()
        .flat_map(|(_, _, _, model)| model.slots.keys().map(String::as_str))
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
    for (value_index, values, look, model) in &looks {
        let state = bake_state(
            look,
            model,
            values,
            &def.namespace,
            index,
            content,
            textures,
        )
        .and_then(|mut state| {
            let name = look.name.or(block.name.as_deref());
            state.name = match name {
                Some(name) => fill_name(name, &properties, value_index, content)?,
                None => title_case(&def.id),
            };
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
    });
    content.block_index.insert(def.id.clone(), index);
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
    textures: BTreeMap<&'d str, &'d TextureRef>,
}

fn look<'d>(block: &'d BlockDef, whens: &[When], index: &[usize]) -> Look<'d> {
    let mut look = Look {
        name: None,
        model: block.model.as_deref(),
        rotate: block.rotate,
        light: block.light,
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

impl Look<'_> {
    /// The model, with `{property}` filled in.
    fn model<'a>(
        &self,
        values: &[(String, String)],
        namespace: &str,
        content: &'a Content,
    ) -> Result<&'a Model, String> {
        let model = self.model.ok_or("no `model`")?;
        let id = qualify(&fill(model, values)?, namespace)?;
        content
            .models
            .get(&id)
            .ok_or_else(|| format!("no model `{id}`"))
    }
}

/// Bakes one block state: the model's faces rotated and textured.
/// Textures for slots the model doesn't have are left out. The name is
/// left for the caller.
fn bake_state(
    look: &Look,
    model: &Model,
    values: &[(String, String)],
    namespace: &str,
    block: usize,
    content: &mut Content,
    textures: &Textures,
) -> Result<State, String> {
    let rotation = rotation(look.rotate.unwrap_or([0; 3]))?;
    let light = look.light.unwrap_or_default();
    if let Some(level) = light.iter().find(|&&level| level > MAX_LIGHT) {
        return Err(format!(
            "light level {level} is too bright; the most is {MAX_LIGHT}"
        ));
    }
    // Turns a point in the block about the block's center.
    let turn = |p: Vec3| rotation * (p - Vec3::splat(0.5)) + Vec3::splat(0.5);
    let rotate_direction = |d: Direction| {
        Direction::from_normal((rotation * d.normal().as_vec3()).round().as_ivec3()).unwrap()
    };

    let mut layers = HashMap::new();
    for (&slot, texture) in &look.textures {
        if !model.slots.contains_key(slot) {
            continue;
        }
        let layer = textures.bake_ref(texture, values, namespace, content)?;
        layers.insert(slot, layer);
    }

    let faces = model
        .faces
        .iter()
        .map(|face| {
            let layer = slot_texture(&face.slot, model, &layers)?;
            let vertices: Vec<Vertex> = face
                .vertices
                .iter()
                .map(|v| Vertex {
                    pos: turn(v.pos),
                    uv: v.uv,
                })
                .collect();
            Ok(BakedFace {
                // A quarter turn keeps it a unit vector, exactly.
                normal: rotation * face.normal,
                vertices,
                layer,
                cull: face.cull.map(rotate_direction),
            })
        })
        .collect::<Result<_, String>>()?;
    let occludes = model
        .occludes
        .iter()
        .fold(0, |bits, &d| bits | rotate_direction(d).bit());
    let collision = model
        .collision
        .iter()
        .map(|b| {
            let [a, c] = [b.min, b.max].map(turn);
            BlockBox {
                min: a.min(c),
                max: a.max(c),
            }
        })
        .collect();
    Ok(State {
        block,
        name: String::new(),
        values: values.to_vec(),
        faces,
        collision,
        occludes,
        light,
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
        let result = block_item(&def.id, file, content, textures);
        // A mistake in the item file is that file's.
        report(file.map_or(&def.file, |file| &file.file), result);
    }
    for def in &defs.items {
        let is_block_item = defs
            .blocks
            .iter()
            .any(|block| block.id == def.id && block.def.item);
        if is_block_item {
            continue;
        }
        report(&def.file, standalone_item(def, content, textures));
    }
}

/// Adds the item that places block `id`: one state per combination of the
/// block's item properties, each placing the block with those values and
/// the rest at their defaults. `file` is an item file changing it.
fn block_item(
    id: &str,
    file: Option<&Def<ItemDef>>,
    content: &mut Content,
    textures: &Textures,
) -> Result<(), String> {
    let block = content.block(id).unwrap().clone();
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
    let item = content.items.len();
    let mut looks = Vec::with_capacity(count);
    for (index, values) in combinations(&properties) {
        let state = placed_state(&block, &values)?;
        let mut look = ItemStateInfo {
            item,
            values: values.clone(),
            name: content.state(state).name.clone(),
            texture: None,
            block: Some(state),
        };
        if let Some(file) = file {
            let namespace = &file.namespace;
            if let Some(name) = &file.def.name {
                look.name = fill_name(name, &properties, &index, content)?;
            }
            if let Some(texture) = &file.def.texture {
                look.texture = Some(
                    textures
                        .bake_ref(texture, &values, namespace, content)
                        .map_err(|error| in_state(&values, error))?,
                );
            }
        }
        looks.push(look);
    }
    let max_stack = max_stack(file.and_then(|file| file.def.max_stack))?;
    push_item(id, properties, max_stack, looks, content);
    Ok(())
}

/// Adds an item from its own file.
fn standalone_item(
    def: &Def<ItemDef>,
    content: &mut Content,
    textures: &Textures,
) -> Result<(), String> {
    let item_def = &def.def;
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
    let item = content.items.len();
    let mut looks = Vec::with_capacity(count);
    for (index, values) in combinations(&properties) {
        let look = (|| {
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
            Ok::<_, String>(ItemStateInfo {
                item,
                values: values.clone(),
                name,
                texture,
                block,
            })
        })()
        .map_err(|error| in_state(&values, error))?;
        looks.push(look);
    }
    let max_stack = max_stack(item_def.max_stack)?;
    push_item(&def.id, properties, max_stack, looks, content);
    Ok(())
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

fn max_stack(max_stack: Option<u32>) -> Result<u32, String> {
    match max_stack {
        Some(0) => Err("`max_stack` must be at least 1".into()),
        other => Ok(other.unwrap_or(DEFAULT_MAX_STACK)),
    }
}

fn push_item(
    id: &str,
    properties: Vec<Property>,
    max_stack: u32,
    looks: Vec<ItemStateInfo>,
    content: &mut Content,
) {
    content.item_index.insert(id.into(), content.items.len());
    content.items.push(Item {
        id: id.into(),
        properties,
        first_state: content.item_states.len() as u32,
        state_count: looks.len() as u32,
        max_stack,
    });
    content.item_states.extend(looks);
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
        let item = content.default_item_look("test:thing").unwrap();
        assert_eq!(item.block, Some(block.default_state()));
        assert_eq!(item.name, "Thing");
        assert_eq!(
            content.item("test:thing").unwrap().max_stack,
            DEFAULT_MAX_STACK
        );
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
            ("items/plain.json5", r#"{ max_stack: 8 }"#),
            (
                "blocks/hidden.json5",
                r#"{ model: "quad", textures: { all: "a" }, item: false }"#,
            ),
            (
                "items/thing.json5",
                r#"{ name: "Special thing", max_stack: 16 }"#,
            ),
            ("items/seeds.json5", r#"{ texture: "b", block: "hidden" }"#),
        ]);
        assert!(content.errors.is_empty(), "{}", errors(&content));
        let thing = content.default_item_look("test:thing").unwrap();
        assert_eq!(thing.name, "Special thing");
        assert_eq!(content.item("test:thing").unwrap().max_stack, 16);
        assert_eq!(thing.block, content.default_state("test:thing"));
        // What an item file leaves out comes from the block.
        let plain = content.default_item_look("test:plain").unwrap();
        assert_eq!(plain.name, "Plain thing");
        assert_eq!(content.item("test:plain").unwrap().max_stack, 8);
        assert_eq!(plain.block, content.default_state("test:plain"));
        assert!(content.item("test:hidden").is_none());
        let seeds = content.default_item_look("test:seeds").unwrap();
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
                ("materials/rock.json5", r##"{ palette: ["#000", "#fff"] }"##),
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
                r##"{ palette: ["#000"] }"##,
                "2 to 8 colors",
            ),
            (
                "materials/x.json5",
                r#"{ palette: ["black", "white"] }"#,
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
            ("items/x.json5", r#"{ max_stack: 0 }"#, "at least 1"),
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
                     palettes: { bark: ["#300000", "#400000"] } }"##,
            ),
            (
                "materials/birch.json5",
                r##"{ tags: ["wood", "pale"], palette: ["#001000", "#002000"] }"##,
            ),
            (
                "materials/rock.json5",
                r##"{ tags: ["rock"], palette: ["#000010", "#000020"] }"##,
            ),
            (
                "materials/iron.json5",
                r##"{ tags: ["ore", "metal"], palette: ["#101010", "#202020"] }"##,
            ),
            (
                "materials/coal.json5",
                r##"{ tags: ["ore"], palette: ["#050505", "#060606"] }"##,
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
                    max_stack: 1,
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
                "texture `test:a` has no input `$wood`",
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
                r##"{ palette: ["#000", "#fff"], palettes: { bark: ["#000"] } }"##,
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
}
