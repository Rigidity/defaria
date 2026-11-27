//! Texture files: their layers and inputs, and the tiles baked from them.

use std::collections::{BTreeMap, BTreeSet};

use bevy::platform::collections::HashMap;

use super::{
    Def,
    fill::{fill, fill_with},
    material::palette,
};
use crate::{
    block_texture::Palette,
    content::{
        Animation, Content, Texture, TextureLayer, check_name, qualify,
        schema::{AnimationDef, LayerDef, TextureDef, TextureRef},
    },
};

/// A texture file, checked, with its inputs still to fill.
pub(super) struct Template {
    layers: Vec<LayerTemplate>,
    animation: Option<Animation>,
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
pub(super) fn template(
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
    let animation = def.def.animation.map(animation).transpose()?;
    Ok((
        Template {
            layers,
            inputs,
            animation,
        },
        problems,
    ))
}

/// A texture's `animation`, checked.
fn animation(def: AnimationDef) -> Result<Animation, String> {
    if let Some(frame_time) = def.frame_time
        && !(frame_time.is_finite() && frame_time > 0.0)
    {
        return Err(format!(
            "an animation's `frame_time` is more than 0 seconds, not {frame_time}"
        ));
    }
    if !(def.scroll.is_finite() && def.scroll >= 0.0) {
        return Err(format!(
            "an animation's `scroll` is 0 or more, not {}",
            def.scroll
        ));
    }
    Ok(Animation {
        frame_time: def.frame_time,
        blend: def.blend,
        scroll: def.scroll,
    })
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
        check_name(name).map_err(|error| format!("input {error}"))?;
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
pub(super) struct Textures {
    pub(super) templates: HashMap<String, Template>,
    /// Every image, by asset path.
    pub(super) images: BTreeSet<String>,
    /// Each mod's folder, by namespace.
    pub(super) dirs: HashMap<String, String>,
}

impl Textures {
    /// Bakes texture `id` for every combination its inputs can take: none
    /// for a texture without inputs, every combination of tagged materials
    /// for one whose inputs are `#tag`s. Every tile is made or none is.
    /// Images that are missing (so their tiles show the placeholder) come
    /// back with the result, each once.
    pub(super) fn bake_all(
        &self,
        id: &str,
        content: &mut Content,
    ) -> Result<BTreeSet<String>, String> {
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
            animation: template.animation,
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
    pub(super) fn bake_ref(
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

#[cfg(test)]
mod tests;
