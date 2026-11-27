//! Placing blocks at once. When a right click would place the block in
//! hand (no earlier step of a use would claim it, and it can go there:
//! `block_edit::placing_for`, as the server decides), the client puts it
//! in its own copy of the world straight away and numbers the edit, which
//! goes with the use. Until the server says it's acted on that use
//! ([`ServerMessage::EditDone`]), the client keeps its block there over
//! whatever the server sends for that voxel, noting what the server has
//! there; then it takes the server's. If the server placed it too, nothing
//! changes; if it didn't (someone stood there, a script took the click),
//! the block goes: the edit is rolled back.
//!
//! Breaking isn't predicted: it takes time the server keeps, and its
//! cracks show how far along it is.

use bevy::prelude::*;
use defaria_core::{
    block_edit::{claimed_before_placing, placing_for},
    content::{BlockState, Content},
    game_mode::GameMode,
    hotbar::{ActiveSlot, Hotbar},
    inventory::Inventory,
    net::ServerMessage,
    physics::Body,
    player::LocalPlayer,
    use_action::{UseAction, UseStep},
    world::{ChunkCoord, Voxels},
};

use crate::{
    net::{ChunkReceived, FromServer, ReceiveSystems},
    session::GameState,
};

pub struct EditPredictionPlugin;

impl Plugin for EditPredictionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PredictedEdits>()
            .add_systems(OnExit(GameState::Playing), forget)
            .add_systems(Update, predict_placing.in_set(UseStep::Place))
            .add_systems(PreUpdate, settle_edits.after(ReceiveSystems));
    }
}

/// The edits placed at once that the server hasn't answered yet.
#[derive(Resource, Debug, Default)]
pub struct PredictedEdits {
    /// The last edit numbered.
    last: u32,
    pending: Vec<Edit>,
    /// The edit this frame's use carries, for the use to take.
    pub this_use: Option<u32>,
    /// How many edits the server didn't make.
    pub rolled_back: u32,
}

impl PredictedEdits {
    /// How many are waiting for the server.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

/// A block placed at once: where, what, and what the server has there as
/// far as the client knows.
#[derive(Debug, Clone, Copy)]
struct Edit {
    number: u32,
    pos: IVec3,
    placed: BlockState,
    server: BlockState,
}

/// A world left: the next starts afresh.
fn forget(mut edits: ResMut<PredictedEdits>) {
    *edits = PredictedEdits::default();
}

/// Places the block in hand at once, for a use the server would place it
/// for.
fn predict_placing(
    content: Res<Content>,
    mut voxels: Voxels,
    mut edits: ResMut<PredictedEdits>,
    player: Query<
        (
            &UseAction,
            &Hotbar,
            &ActiveSlot,
            Option<&Inventory>,
            Option<&GameMode>,
            &Body,
        ),
        With<LocalPlayer>,
    >,
) {
    edits.this_use = None;
    let Ok((action, hotbar, active, inventory, mode, body)) = player.single() else {
        return;
    };
    let Some(using) = action.pending() else {
        return;
    };
    let Some(item) = hotbar.item(*active) else {
        return;
    };
    if claimed_before_placing(&content, &voxels, &using, Some(item)) {
        return;
    }
    let mode = mode.copied().unwrap_or_default();
    let has_one =
        mode.is_creative() || inventory.is_some_and(|inventory| inventory.count(item) > 0);
    if !has_one {
        return;
    }
    let boxes = std::iter::once(body.aabb());
    let Some(placing) = placing_for(&content, &voxels, &using, hotbar, *active, mode, boxes) else {
        return;
    };
    let Some(server) = voxels.get(placing.pos) else {
        return;
    };
    edits.last += 1;
    let number = edits.last;
    edits.pending.push(Edit {
        number,
        pos: placing.pos,
        placed: placing.block,
        server,
    });
    edits.this_use = Some(number);
    voxels.set(placing.pos, placing.block);
}

/// Keeps the blocks placed at once over the chunks the server sends,
/// noting what it has there, and takes the server's once it's answered.
fn settle_edits(
    mut received: MessageReader<ChunkReceived>,
    mut messages: MessageReader<FromServer>,
    mut voxels: Voxels,
    mut edits: ResMut<PredictedEdits>,
) {
    let edits = &mut *edits;
    for ChunkReceived(coord) in received.read() {
        for edit in &mut edits.pending {
            if ChunkCoord::split(edit.pos).0.0 != *coord {
                continue;
            }
            let Some(now) = voxels.get(edit.pos) else {
                continue;
            };
            edit.server = now;
            if now != edit.placed {
                voxels.set(edit.pos, edit.placed);
            }
        }
    }
    // Gone with their chunks.
    edits.pending.retain(|edit| voxels.get(edit.pos).is_some());
    let Some(done) = messages
        .read()
        .filter_map(|FromServer(message)| match message {
            ServerMessage::EditDone(number) => Some(*number),
            _ => None,
        })
        .max()
    else {
        return;
    };
    for edit in edits.pending.extract_if(.., |edit| edit.number <= done) {
        if edit.server != edit.placed {
            edits.rolled_back += 1;
        }
        if voxels.get(edit.pos) != Some(edit.server) {
            voxels.set(edit.pos, edit.server);
        }
    }
}
