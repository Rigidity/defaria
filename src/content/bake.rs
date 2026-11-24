//! Parses the definition files and resolves the references between them,
//! in dependency order: materials, textures, models, blocks, items.
//!
//! Each definition that fails is reported once, naming its file, and left
//! out; definitions referring to it then fail with "no such …".

use std::collections::{BTreeMap, BTreeSet};

use bevy::{platform::collections::HashMap, prelude::*};
use serde::de::DeserializeOwned;

use super::{
    AIR, BakedFace, Block, Content, Direction, Item, ModInfo, Model, ModelFace, State, Texture,
    TextureLayer, Vertex, load::LoadedMod, qualify, schema::*,
};
use crate::block_texture::Palette;

/// The folders definition files live in, by kind.
const KINDS: [&str; 5] = ["materials", "textures", "models", "blocks", "items"];
/// Most states one block may have.
const MAX_STATES_PER_BLOCK: usize = 4096;
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
            name: "Air".into(),
            properties: Vec::new(),
            first_state: 0,
            state_count: 1,
        }],
        states: vec![State::default()],
        items: Vec::new(),
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
        let result = palette(&def.def.palette).map(|palette| {
            content.materials.insert(def.id.clone(), palette);
        });
        report(&def.file, result);
    }
    for def in &defs.textures {
        let result = texture(def, &content, &images, &dirs).map(|texture| {
            let layer = content.textures.len() as u32;
            content.texture_index.insert(def.id.clone(), layer);
            content.textures.push(texture);
        });
        report(&def.file, result);
    }
    for def in &defs.models {
        let result = model(def).map(|model| {
            content.models.insert(def.id.clone(), model);
        });
        report(&def.file, result);
    }
    for def in &defs.blocks {
        report(&def.file, add_block(def, &mut content));
    }
    add_items(&defs, &mut content, &mut report);
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
    let (kind, rest) = path.split_once('/').unwrap_or(("", path));
    let name = rest
        .strip_suffix(".json5")
        .filter(|_| KINDS.contains(&kind))
        .ok_or_else(|| {
            format!(
                "not a definition: files go in {}, named <name>.json5",
                KINDS.join(", ")
            )
        })?;
    let id = qualify(name, namespace)?;
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
        "materials" => defs.materials.push(def(id, namespace, file, text)?),
        "textures" => defs.textures.push(def(id, namespace, file, text)?),
        "models" => defs.models.push(def(id, namespace, file, text)?),
        "blocks" => defs.blocks.push(def(id, namespace, file, text)?),
        _ => defs.items.push(def(id, namespace, file, text)?),
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

fn texture(
    def: &Def<TextureDef>,
    content: &Content,
    images: &BTreeSet<String>,
    dirs: &HashMap<String, String>,
) -> Result<Texture, String> {
    if def.def.layers.is_empty() {
        return Err("a texture needs at least one layer".into());
    }
    let layers = def
        .def
        .layers
        .iter()
        .enumerate()
        .map(|(i, layer)| {
            texture_layer(layer, &def.namespace, content, images, dirs)
                .map_err(|error| format!("layer {}: {error}", i + 1))
        })
        .collect::<Result<_, _>>()?;
    Ok(Texture {
        id: def.id.clone(),
        layers,
    })
}

fn texture_layer(
    layer: &LayerDef,
    namespace: &str,
    content: &Content,
    images: &BTreeSet<String>,
    dirs: &HashMap<String, String>,
) -> Result<TextureLayer, String> {
    let image_id = qualify(&layer.image, namespace)?;
    let (image_mod, name) = image_id.split_once(':').unwrap();
    let dir = dirs
        .get(image_mod)
        .ok_or_else(|| format!("image `{image_id}`: no mod `{image_mod}` is loaded"))?;
    let image = format!("{dir}/images/{name}.png");
    if !images.contains(&image) {
        return Err(format!("image `{image_id}`: there's no {image}"));
    }
    let palette = match (&layer.material, &layer.palette) {
        (Some(material), None) => {
            let id = qualify(material, namespace)?;
            *content
                .materials
                .get(&id)
                .ok_or_else(|| format!("no material `{id}`"))?
        }
        (None, Some(colors)) => palette(colors)?,
        _ => return Err("give a layer either a `material` or a `palette`".into()),
    };
    Ok(TextureLayer { image, palette })
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
            let face = ModelFace {
                slot: face.texture.clone(),
                cull: face.cull,
                vertices: face
                    .vertices
                    .iter()
                    .map(|v| Vertex {
                        pos: Vec3::from(v.pos),
                        uv: Vec2::from(v.uv),
                    })
                    .collect(),
            };
            check_face(&face, &model.slots).map_err(|error| format!("face {}: {error}", i + 1))?;
            Ok(face)
        })
        .collect::<Result<_, String>>()?;
    Ok(Model {
        id: def.id.clone(),
        slots: model.slots.clone(),
        occludes: model.occludes.clone(),
        faces,
    })
}

fn check_face(face: &ModelFace, slots: &BTreeMap<String, Option<String>>) -> Result<(), String> {
    if !slots.contains_key(&face.slot) {
        return Err(format!("`{}` isn't one of the model's slots", face.slot));
    }
    if !(3..=4).contains(&face.vertices.len()) {
        return Err(format!(
            "a face has 3 or 4 vertices, not {}",
            face.vertices.len()
        ));
    }
    face_normal(&face.vertices).map(|_| ())
}

/// The normal of a face whose vertices run counter-clockwise around it.
fn face_normal(vertices: &[Vertex]) -> Result<Vec3, String> {
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

/// Checks a block, bakes every one of its states, and adds them all, or
/// nothing if any state fails.
fn add_block(def: &Def<BlockDef>, content: &mut Content) -> Result<(), String> {
    let block = &def.def;
    let properties: Vec<(String, Vec<String>)> = block
        .properties
        .iter()
        .map(|(name, values)| (name.clone(), values.clone()))
        .collect();
    let mut count = 1usize;
    for (name, values) in &properties {
        if values.is_empty() {
            return Err(format!("property `{name}` has no values"));
        }
        if values.iter().collect::<BTreeSet<_>>().len() != values.len() {
            return Err(format!("property `{name}` lists a value twice"));
        }
        count = count.saturating_mul(values.len());
    }
    if count > MAX_STATES_PER_BLOCK {
        return Err(format!(
            "{count} states is too many; a block may have {MAX_STATES_PER_BLOCK}"
        ));
    }
    if content.states.len() + count > u16::MAX as usize + 1 {
        return Err("out of block states: there are too many blocks".into());
    }
    for (i, variant) in block.variants.iter().enumerate() {
        for (name, wanted) in &variant.when {
            let values = properties
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, values)| values)
                .ok_or_else(|| format!("variant {}: no property `{name}`", i + 1))?;
            if let Some(value) = wanted.values().iter().find(|v| !values.contains(v)) {
                return Err(format!(
                    "variant {}: `{name}` has no value `{value}`",
                    i + 1
                ));
            }
        }
    }

    let mut states = Vec::with_capacity(count);
    for offset in 0..count {
        // Undo `Block::state_offset`: the last property counts fastest.
        let mut rest = offset;
        let mut values = vec![(String::new(), String::new()); properties.len()];
        for (p, (name, options)) in properties.iter().enumerate().rev() {
            values[p] = (name.clone(), options[rest % options.len()].clone());
            rest /= options.len();
        }
        let state = bake_state(def, &values, content).map_err(|error| {
            if values.is_empty() {
                error
            } else {
                let state: Vec<String> = values.iter().map(|(n, v)| format!("{n}={v}")).collect();
                format!("state {}: {error}", state.join(","))
            }
        })?;
        states.push(state);
    }

    let index = content.blocks.len();
    content.blocks.push(Block {
        id: def.id.clone(),
        name: block.name.clone().unwrap_or_else(|| title_case(&def.id)),
        properties,
        first_state: content.states.len() as u16,
        state_count: count as u16,
    });
    content.block_index.insert(def.id.clone(), index);
    content.states.extend(states.into_iter().map(|state| State {
        block: index,
        ..state
    }));
    Ok(())
}

/// One state of a block: its look with every matching variant applied,
/// `{property}` filled in, and the model's faces rotated and textured.
fn bake_state(
    def: &Def<BlockDef>,
    values: &[(String, String)],
    content: &Content,
) -> Result<State, String> {
    let mut look = def.def.look();
    for variant in &def.def.variants {
        let matches = variant.when.iter().all(|(name, wanted)| {
            values
                .iter()
                .any(|(n, v)| n == name && wanted.values().contains(v))
        });
        if matches {
            let over = variant.look();
            look.model = over.model.or(look.model);
            look.textures.extend(over.textures);
            look.rotate = over.rotate.or(look.rotate);
        }
    }

    let model_ref = look.model.ok_or("no `model`")?;
    let model_id = qualify(&fill(&model_ref, values)?, &def.namespace)?;
    let model = content
        .models
        .get(&model_id)
        .ok_or_else(|| format!("no model `{model_id}`"))?;
    let rotation = rotation(look.rotate.unwrap_or([0; 3]))?;
    let rotate_direction = |d: Direction| {
        Direction::from_normal((rotation * d.normal().as_vec3()).round().as_ivec3()).unwrap()
    };

    let mut textures = HashMap::new();
    for (slot, texture) in &look.textures {
        if !model.slots.contains_key(slot) {
            return Err(format!("`{model_id}` has no slot `{slot}`"));
        }
        let id = qualify(&fill(texture, values)?, &def.namespace)?;
        let layer = content
            .texture(&id)
            .ok_or_else(|| format!("no texture `{id}`"))?;
        textures.insert(slot.as_str(), layer);
    }

    let faces = model
        .faces
        .iter()
        .map(|face| {
            let layer = slot_texture(&face.slot, model, &textures)?;
            let vertices: Vec<Vertex> = face
                .vertices
                .iter()
                .map(|v| Vertex {
                    pos: rotation * (v.pos - Vec3::splat(0.5)) + Vec3::splat(0.5),
                    uv: v.uv,
                })
                .collect();
            Ok(BakedFace {
                normal: face_normal(&vertices)?,
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
    Ok(State {
        block: 0,
        values: values.to_vec(),
        faces,
        occludes,
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

/// Fills `{property}` in `text` with the state's value of that property.
fn fill(text: &str, values: &[(String, String)]) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('}')
            .ok_or_else(|| format!("`{text}` has a `{{` with no `}}`"))?;
        let name = &rest[start + 1..start + end];
        let (_, value) = values
            .iter()
            .find(|(n, _)| n == name)
            .ok_or_else(|| format!("`{text}` uses `{{{name}}}`, which isn't a property"))?;
        out.push_str(value);
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    Ok(out)
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
    report: &mut impl FnMut(&str, Result<(), String>),
) {
    for def in &defs.blocks {
        let Some(block) = content.block(&def.id) else {
            continue;
        };
        if def.def.item {
            let item = Item {
                id: def.id.clone(),
                name: block.name.clone(),
                texture: None,
                block: Some(block.default_state()),
                max_stack: DEFAULT_MAX_STACK,
            };
            content
                .item_index
                .insert(item.id.clone(), content.items.len());
            content.items.push(item);
        }
    }
    for def in &defs.items {
        let result = item(def, content).map(|item| match content.item_index.get(&def.id) {
            Some(&i) => content.items[i] = item,
            None => {
                content
                    .item_index
                    .insert(def.id.clone(), content.items.len());
                content.items.push(item);
            }
        });
        report(&def.file, result);
    }
}

fn item(def: &Def<ItemDef>, content: &Content) -> Result<Item, String> {
    // A block's own item, if this file changes one.
    let base = content.item(&def.id);
    let texture = match &def.def.texture {
        Some(texture) => {
            let id = qualify(texture, &def.namespace)?;
            Some(
                content
                    .texture(&id)
                    .ok_or_else(|| format!("no texture `{id}`"))?,
            )
        }
        None => base.and_then(|item| item.texture),
    };
    let block = match &def.def.block {
        Some(block) => {
            let id = qualify(block, &def.namespace)?;
            Some(
                content
                    .default_state(&id)
                    .filter(|state| !state.is_air())
                    .ok_or_else(|| format!("no block `{id}`"))?,
            )
        }
        None => base.and_then(|item| item.block),
    };
    let max_stack = def
        .def
        .max_stack
        .or(base.map(|item| item.max_stack))
        .unwrap_or(DEFAULT_MAX_STACK);
    if max_stack == 0 {
        return Err("`max_stack` must be at least 1".into());
    }
    Ok(Item {
        id: def.id.clone(),
        name: def
            .def
            .name
            .clone()
            .or(base.map(|item| item.name.clone()))
            .unwrap_or_else(|| title_case(&def.id)),
        texture,
        block,
        max_stack,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::load::ModFiles;

    const MANIFEST: &str = r#"{ id: "test", name: "Test" }"#;

    const QUAD: &str = r#"{
        slots: { all: null, side: "all" },
        occludes: ["up"],
        faces: [{
            texture: "side", cull: "up",
            vertices: [
                { pos: [0, 1, 1], uv: [0, 1] },
                { pos: [1, 1, 1], uv: [1, 1] },
                { pos: [1, 1, 0], uv: [1, 0] },
                { pos: [0, 1, 0], uv: [0, 0] },
            ],
        }],
    }"#;

    /// A mod with a material, two textures, the one-quad model and `files`.
    fn content(files: &[(&str, &str)]) -> Content {
        let mut all = vec![
            ("images/a.png", ""),
            (
                "materials/grey.json5",
                r##"{ palette: ["#000000", "#ffffff"] }"##,
            ),
            (
                "textures/a.json5",
                r#"{ layers: [{ image: "a", material: "grey" }] }"#,
            ),
            (
                "textures/b.json5",
                r##"{ layers: [{ image: "a", palette: ["#ff0000", "#00ff00", "#0000ff"] }] }"##,
            ),
            ("models/quad.json5", QUAD),
        ];
        all.extend_from_slice(files);
        Content::from_mods(vec![ModFiles::new("mods/test", MANIFEST, &all)])
    }

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
        assert_eq!(block.name, "Thing");
        let state = content.state(block.default_state());
        assert_eq!(state.faces.len(), 1);
        assert_eq!(state.faces[0].layer, content.texture("test:a").unwrap());
        assert_eq!(state.faces[0].normal, Vec3::Y);
        assert!(state.occludes(Direction::Up));
        assert!(!state.occludes(Direction::Down));
        let item = content.item("test:thing").unwrap();
        assert_eq!(item.block, Some(block.default_state()));
        assert_eq!(item.max_stack, DEFAULT_MAX_STACK);
    }

    #[test]
    fn textures_use_materials_or_inline_palettes() {
        let content = content(&[]);
        let [a, b] =
            ["test:a", "test:b"].map(|id| &content.textures[content.texture(id).unwrap() as usize]);
        assert_eq!(a.layers[0].image, "mods/test/images/a.png");
        assert_eq!(a.layers[0].palette, content.materials["test:grey"]);
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
        // The second variant's `side` beats the block's `all`.
        assert_eq!(turned.layer, a);
        let state = content.state(block.state(&[("turn", "yes")]).unwrap());
        assert!(state.occludes(Direction::West) && !state.occludes(Direction::Up));
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
                r#"{ model: "quad", textures: { all: "a" } }"#,
            ),
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
        let thing = content.item("test:thing").unwrap();
        assert_eq!(thing.name, "Special thing");
        assert_eq!(thing.max_stack, 16);
        assert_eq!(thing.block, content.default_state("test:thing"));
        assert!(content.item("test:hidden").is_none());
        let seeds = content.item("test:seeds").unwrap();
        assert_eq!(seeds.name, "Seeds");
        assert_eq!(seeds.block, content.default_state("test:hidden"));
        assert_eq!(seeds.texture, content.texture("test:b"));
        assert_eq!(content.items.len(), 2);
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
                "",
            ),
            (
                "blocks/x.json5",
                r#"{ model: "nope", textures: { all: "a" } }"#,
                "no model `test:nope`",
            ),
            (
                "blocks/x.json5",
                r#"{ model: "quad", textures: { al: "a" } }"#,
                "no slot `al`",
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
                r#"{ model: "quad", textures: { all: "a" }, rotate: [10, 0, 0] }"#,
                "multiples of 90",
            ),
            (
                "textures/x.json5",
                r#"{ layers: [{ image: "missing", material: "grey" }] }"#,
                "there's no mods/test/images/missing.png",
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

    #[test]
    fn names_default_to_the_id_in_title_case() {
        assert_eq!(title_case("defaria:grass_top"), "Grass top");
        assert_eq!(title_case("defaria:ores/iron_ore"), "Iron ore");
    }
}
