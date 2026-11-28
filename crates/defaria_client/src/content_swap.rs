//! Playing with other content: a server's mods, for as long as the client
//! plays there, and its own again after.
//!
//! The client loads its own mods at startup (`ContentPlugin`), for single
//! player. Joining a server, it builds [`Content`] from that server's
//! mods instead (`defaria::mod_transfer`), and [`swap_content`] puts it
//! in place before the world starts: the block texture is built again
//! from its images, the item icons are drawn again, and the scripts load
//! afresh from its mods. Everything else reads [`Content`] as it goes.
//! Meshing in the background takes the new one as it changes.

use bevy::prelude::*;
use defaria_core::{content::Content, script::ScriptRuntime};

use crate::{
    ui::icon::ItemIcons,
    voxel_material::{self, BlockTiles},
};

/// Puts `content` in place of the client's, and returns the one it
/// replaced. Meant for between worlds (no chunks drawn with the old
/// one).
pub fn swap_content(world: &mut World, content: Content) -> Option<Content> {
    let old = world.remove_resource::<Content>();
    world.insert_resource(content);
    // Tiles of the old textures mean nothing now; the new ones are built
    // once the new layers have loaded.
    if let Some(mut tiles) = world.get_resource_mut::<BlockTiles>() {
        tiles.0.clear();
    }
    voxel_material::reload_layers(world);
    // Each item keeps its image handle (nodes may show it); there may be
    // more or fewer items.
    let count = world.resource::<Content>().item_states.len();
    if let Some(mut icons) = world.remove_resource::<ItemIcons>() {
        let images = world.resource::<Assets<Image>>();
        icons.0.truncate(count);
        while icons.0.len() < count {
            icons.0.push(images.reserve_handle());
        }
        world.insert_resource(icons);
    }
    if world.contains_resource::<ScriptRuntime>() {
        let runtime = ScriptRuntime::from_world(world);
        world.insert_resource(runtime);
    }
    old
}
