//! What block entities are made of, as both sides know them: a part's
//! saved form ([`Value`], [`SavedPart`]), a block entity as a chunk keeps
//! it while it isn't spawned ([`BlockEntityData`]), and the trait parts
//! implement ([`BlockEntityPart`]), which the machine parts the client is
//! sent do too. Spawning, indexing and saving block entities is the
//! server's (`defaria_server::block_entity`), which the module docs there
//! describe.

use std::collections::BTreeMap;

use bevy::prelude::*;

use crate::content::{BlockState, Content};

mod value;

pub use value::Value;

/// A block entity as it's kept in a chunk that isn't spawned, and saved.
#[derive(Reflect, Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockEntityData {
    /// The id of the block that has it. On loading, one whose voxel isn't
    /// that block any more is dropped.
    pub kind: String,
    /// Each part's saved value, by its [`BlockEntityPart::KEY`].
    pub parts: BTreeMap<String, SavedPart>,
}

/// A part of a block entity, saved.
#[derive(Reflect, Debug, Clone, PartialEq, Eq)]
pub struct SavedPart {
    /// The [`BlockEntityPart::VERSION`] that saved it.
    pub version: u32,
    pub value: Value,
}

/// A component a feature adds to block entities, saved with them.
pub trait BlockEntityPart: Component + Sized {
    /// What it's saved under, unique among parts.
    const KEY: &'static str;
    /// Its saved form's version, passed back to [`BlockEntityPart::load`].
    /// Bump it when the form changes.
    const VERSION: u32 = 1;
    /// Whether it's saved. A part that's worked out from the voxel's state
    /// (a machine's ports) isn't: it's made with [`BlockEntityPart::new`]
    /// every time its block entity is, and never written to the save.
    const SAVED: bool = true;

    /// What a new block entity in a voxel of `state` starts with: `None`
    /// if that block's block entities don't have this part.
    fn new(state: BlockState, content: &Content) -> Option<Self>;

    /// Its saved form. `content` names what needs naming (items, say),
    /// since indices into it aren't kept between runs.
    fn save(&self, content: &Content) -> Value;

    /// Reads back what [`BlockEntityPart::save`] saved, at `version`, for a
    /// voxel of `state`. `None` if it can't, and the block entity gets a
    /// new part instead.
    fn load(value: &Value, version: u32, state: BlockState, content: &Content) -> Option<Self>;
}
