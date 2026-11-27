//! Placing blocks: the last step of a use ([`UseStep::Place`]), placing
//! the block in hand where [`placing_for`] says. What the crosshair
//! targets and where a block goes are `defaria_core::block_edit`, which
//! the client predicts by.

use bevy::prelude::*;
use defaria_core::{
    block_edit::{Placing, placing_for},
    content::Content,
    game_mode::{GameMode, harvest, use_up},
    hotbar::{ActiveSlot, Hotbar},
    inventory::Inventory,
    physics::Body,
    use_action::{UseAction, UseStep},
    world::{RemeshChunks, Voxels},
};

pub struct BlockEditPlugin;

impl Plugin for BlockEditPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            edit_blocks
                .in_set(BlockEditSystems)
                .in_set(UseStep::Place)
                .before(RemeshChunks),
        );
    }
}

/// Where blocks are placed.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockEditSystems;

/// Places the block in hand against the face aimed at, for each use no
/// earlier step claimed, if it can go there and, in survival, the player
/// has one. As the last step, it claims every use it sees, so none
/// outlasts its frame.
fn edit_blocks(
    mut players: Query<(
        &mut UseAction,
        &Hotbar,
        &ActiveSlot,
        Option<&mut Inventory>,
        Option<&GameMode>,
    )>,
    content: Res<Content>,
    bodies: Query<&Body>,
    mut voxels: Voxels,
) {
    for (mut action, hotbar, active, inventory, mode) in &mut players {
        let Some(using) = action.pending() else {
            continue;
        };
        action.claim();
        let Some(item) = hotbar.item(*active) else {
            continue;
        };
        let mode = mode.copied().unwrap_or_default();
        let boxes = bodies.iter().map(Body::aabb);
        let Some(Placing {
            pos,
            block,
            replaced,
            ..
        }) = placing_for(&content, &voxels, &using, hotbar, *active, mode, boxes)
        else {
            continue;
        };
        let mut inventory = inventory.map(Mut::into_inner);
        if !use_up(mode, inventory.as_deref_mut(), item) {
            continue;
        }
        // A plant built over goes into the inventory in survival, if it
        // fits (otherwise it's lost, as when it loses its support).
        if let Some(inventory) = inventory {
            harvest(mode, inventory, &content, replaced);
        }
        voxels.set(pos, block);
    }
}
