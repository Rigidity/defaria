//! Right click on scripted blocks and with scripted items, as uses of them
//! (see `block_edit`): a block whose script defines `on_used` is used
//! instead of placing against it (unless the player sneaks with something
//! in hand), and an item whose script defines `on_use` is used instead of
//! placed. Each takes the click, so nothing else acts on it, and sends a
//! [`BlockUsed`] or [`ItemUsed`] for [`hooks`](super::hooks) to call the
//! hook on the next tick.
//!
//! Using a block is in [`BlockUseSystems`], after the wrench and before
//! machine screens (so a scripted machine's `on_used` replaces its
//! screen); using an item comes after all of them, so a block that does
//! something when used (a machine's screen) comes before the item in hand.

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use super::{BlockUsed, ItemUsed, ScriptIndex, api::PlayerView};
use crate::{
    block_edit::{BlockEditSystems, BlockUseSystems, crosshair_target},
    content::{Content, Direction},
    fly_camera::FlyCameraSystems,
    game_mode::GameMode,
    hotbar::{ActiveSlot, Hotbar, HotbarInput},
    machine::screen::open_machine_screen,
    player::{Eyes, LocalPlayer, Player},
    ui::playing,
    world::Voxels,
    wrench::use_wrench,
};

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            use_scripted_block
                .in_set(BlockUseSystems)
                .after_ignore_deferred(use_wrench)
                .before_ignore_deferred(open_machine_screen),
            use_scripted_item
                .run_if(playing)
                .after(HotbarInput)
                // Without a sync point, as block uses have none.
                .after_ignore_deferred(BlockUseSystems)
                .before(BlockEditSystems)
                .before(FlyCameraSystems),
        ),
    );
}

/// Whether this frame has a right click to use, with the cursor locked
/// (the click that locks it uses nothing).
fn clicked(mouse: &ButtonInput<MouseButton>, cursor: &CursorOptions) -> bool {
    cursor.grab_mode != CursorGrabMode::None && mouse.just_pressed(MouseButton::Right)
}

/// What a script sees of the player using something.
fn view(
    camera: &Transform,
    content: &Content,
    voxels: &Voxels,
    mode: Option<&GameMode>,
    walker: Option<&Player>,
) -> PlayerView {
    let hit = crosshair_target(camera, content, voxels);
    PlayerView {
        pos: camera.translation,
        look: *camera.forward(),
        mode: mode.copied().unwrap_or_default(),
        sneaking: walker.is_some_and(|walker| walker.sneaking),
        target: hit.map(|hit| (hit.block, Direction::from_normal(hit.normal))),
    }
}

/// Uses the block under the crosshair if its script defines `on_used`,
/// taking the click; sneaking with something in hand passes it by, to
/// place against it.
pub(crate) fn use_scripted_block(
    // Left out in apps without input, such as the scripts' tests.
    mouse: Option<ResMut<ButtonInput<MouseButton>>>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    eyes: Single<Eyes, With<LocalPlayer>>,
    player: Option<
        Single<
            (
                Entity,
                &Hotbar,
                &ActiveSlot,
                Option<&GameMode>,
                Option<&Player>,
            ),
            With<LocalPlayer>,
        >,
    >,
    content: Res<Content>,
    scripts: Option<Res<ScriptIndex>>,
    voxels: Voxels,
    mut uses: MessageWriter<BlockUsed>,
) {
    let camera = eyes.transform();
    let (Some(mut mouse), Some(player), Some(scripts)) = (mouse, player, scripts) else {
        return;
    };
    if !clicked(&mouse, &cursor) {
        return;
    }
    let (_, hotbar, active, mode, walker) = player.into_inner();
    let view = view(&camera, &content, &voxels, mode, walker);
    let Some((pos, _)) = view.target else {
        return;
    };
    if view.sneaking && hotbar.item(*active).is_some() {
        return;
    }
    if !voxels
        .get(pos)
        .is_some_and(|state| scripts.is_usable(&content, state))
    {
        return;
    }
    mouse.clear_just_pressed(MouseButton::Right);
    uses.write(BlockUsed {
        pos,
        player: Some(view),
    });
}

/// Uses the item in hand if its script defines `on_use`, taking the click,
/// when nothing used the block under the crosshair.
pub(crate) fn use_scripted_item(
    // Left out in apps without input, such as the scripts' tests.
    mouse: Option<ResMut<ButtonInput<MouseButton>>>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    eyes: Single<Eyes, With<LocalPlayer>>,
    player: Option<
        Single<
            (
                Entity,
                &Hotbar,
                &ActiveSlot,
                Option<&GameMode>,
                Option<&Player>,
            ),
            With<LocalPlayer>,
        >,
    >,
    content: Res<Content>,
    scripts: Option<Res<ScriptIndex>>,
    voxels: Voxels,
    mut uses: MessageWriter<ItemUsed>,
) {
    let camera = eyes.transform();
    let (Some(mut mouse), Some(player), Some(scripts)) = (mouse, player, scripts) else {
        return;
    };
    if !clicked(&mouse, &cursor) {
        return;
    }
    let (entity, hotbar, active, mode, walker) = player.into_inner();
    let Some(item) = hotbar.item(*active) else {
        return;
    };
    if scripts.item_use(&content, item).is_none() {
        return;
    }
    mouse.clear_just_pressed(MouseButton::Right);
    uses.write(ItemUsed {
        player: entity,
        item,
        view: view(&camera, &content, &voxels, mode, walker),
    });
}
