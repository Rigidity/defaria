//! A block state's look: the block's own, with its matching variants applied.

use std::collections::BTreeMap;

use super::when::When;
use crate::content::schema::{BlockDef, Render, TextureRef};

/// A block state's look: the block's own, with every matching variant
/// applied in order. The name, model and rotation are replaced; textures
/// merge slot by slot.
pub(super) struct Look<'d> {
    pub(super) name: Option<&'d str>,
    pub(super) model: Option<&'d str>,
    pub(super) rotate: Option<[i32; 3]>,
    pub(super) light: Option<[u8; 3]>,
    pub(super) render: Option<Render>,
    pub(super) textures: BTreeMap<&'d str, &'d TextureRef>,
}

pub(super) fn look<'d>(block: &'d BlockDef, whens: &[When], index: &[usize]) -> Look<'d> {
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
