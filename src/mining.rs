//! Mining: holding left click breaks the block under the crosshair after a
//! time set by its hardness.
//!
//! - A block takes [`break_time`] seconds of holding to break: its state's
//!   hardness times [`SECONDS_PER_HARDNESS`]. A block with no hardness
//!   breaks on the frame it's targeted.
//! - Releasing the button, looking at another block, the block changing
//!   (say, to another state), unlocking the cursor or opening a screen
//!   starts over.
//! - Keep holding and the next block under the crosshair starts once
//!   [`BREAK_DELAY`] has passed, so one click can't sweep through a row
//!   of blocks that break at once.
//! - The click that locks the cursor mines nothing, even if it's held.
//!
//! [`Mining`], on the local player, says which block is being mined and how far
//! along it is, for the cracks drawn on it and for `inspect`.

use std::time::Duration;

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

use crate::{
    block_edit::{BlockEditSystems, BlockOutlineSystems, crosshair_target},
    content::{BlockState, Content},
    fly_camera::FlyCameraSystems,
    game_mode::{GameMode, harvest},
    inventory::Inventory,
    meshing::RemeshChunks,
    player::{Eyes, LocalPlayer},
    ui::{UiScreen, playing},
    world::Voxels,
};

/// Seconds of mining per point of hardness. Stone, at a hardness of 1.5,
/// takes a little over a second.
pub const SECONDS_PER_HARDNESS: f32 = 0.75;

/// How long after a block breaks holding on starts on the next one.
pub const BREAK_DELAY: Duration = Duration::from_millis(250);

/// How close to a break time or the end of [`BREAK_DELAY`] counts as
/// there, so frame times that don't add up exactly (1/60 s truncated to
/// whole nanoseconds) don't cost a whole extra frame.
const TOLERANCE: Duration = Duration::from_micros(1);

/// How long a block of this hardness takes to break, in seconds.
pub fn break_time(hardness: f32) -> f32 {
    hardness.max(0.0) * SECONDS_PER_HARDNESS
}

pub struct MiningPlugin;

impl Plugin for MiningPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Mining>()
            .add_observer(give_the_player_mining)
            .add_systems(
                Update,
                (
                    // Before the fly camera, so the click that locks the
                    // cursor finds it still unlocked and arms nothing.
                    arm.before(FlyCameraSystems),
                    // After the camera turns, so the block mined is the one
                    // under the crosshair on the frame being drawn. After
                    // placing, so the two edits on one frame always land in
                    // the same order. Before the outline, so it's drawn
                    // around the next block, not one that just broke. Before
                    // remeshing, so a break shows this frame.
                    mine.in_set(MiningSystems)
                        .after(arm)
                        .after(FlyCameraSystems)
                        .after(BlockEditSystems)
                        .before(BlockOutlineSystems)
                        .before(RemeshChunks),
                ),
            );
    }
}

/// Where mining progresses and blocks break.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct MiningSystems;

/// The block the camera is mining, if any, and how far along it is.
#[derive(Component, Reflect, Debug, Clone, Copy, Default, PartialEq)]
#[reflect(Component)]
pub struct Mining {
    /// The block being mined, and its state now.
    pub target: Option<(IVec3, BlockState)>,
    /// From 0 when mining starts to 1 when the block breaks.
    pub progress: f32,
    /// Whether the held button counts: it was pressed while playing with the
    /// cursor locked.
    armed: bool,
    /// What's left of [`BREAK_DELAY`].
    delay: Duration,
    /// How long the target has been mined; `progress` is this over its
    /// break time.
    elapsed: Duration,
}

fn give_the_player_mining(add: On<Add<LocalPlayer>>, mut commands: Commands) {
    commands.entity(add.entity).insert(Mining::default());
}

/// Whether the player can mine now: the cursor is locked and no screen
/// is open.
fn can_mine(cursor: &CursorOptions, screens: Query<(), With<UiScreen>>) -> bool {
    cursor.grab_mode != CursorGrabMode::None && playing(screens)
}

/// Arms mining on a left press made while the player can mine, before the
/// fly camera can lock the cursor for it.
fn arm(
    mouse: Res<ButtonInput<MouseButton>>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    screens: Query<(), With<UiScreen>>,
    mut mining: Single<&mut Mining, With<LocalPlayer>>,
) {
    if mouse.just_pressed(MouseButton::Left) {
        mining.armed = can_mine(&cursor, screens);
    }
}

/// Mines the block under the crosshair while the armed button is held, and
/// breaks it once its break time is up.
fn mine(
    time: Res<Time>,
    mouse: Res<ButtonInput<MouseButton>>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    screens: Query<(), With<UiScreen>>,
    content: Res<Content>,
    miner: Single<(Eyes, &mut Mining), With<LocalPlayer>>,
    // What it gets for it, in survival.
    carrier: Option<Single<(&mut Inventory, &GameMode), With<LocalPlayer>>>,
    mut voxels: Voxels,
) {
    let (eyes, mut mining) = miner.into_inner();
    let transform = &eyes.transform();
    if !(can_mine(&cursor, screens) && mining.armed && mouse.pressed(MouseButton::Left)) {
        mining.set_if_neq(Mining::default());
        return;
    }
    if !mining.delay.is_zero() {
        mining.delay = mining.delay.saturating_sub(time.delta());
        if mining.delay > TOLERANCE {
            return;
        }
        mining.delay = Duration::ZERO;
    }
    let Some(hit) = crosshair_target(transform, &content, &voxels) else {
        mining.target = None;
        mining.progress = 0.0;
        mining.elapsed = Duration::ZERO;
        return;
    };
    let target = (hit.block, voxels.get(hit.block).unwrap_or(BlockState::AIR));
    let seconds = break_time(content.state(target.1).hardness);
    // The engine updating its properties (a pipe joining one placed next
    // to it) doesn't make it another target.
    let same = mining.target.is_some_and(|(pos, state)| {
        pos == target.0 && content.state(state).block == content.state(target.1).block
    });
    if same {
        mining.target = Some(target);
        mining.elapsed += time.delta();
    } else {
        // This frame's time went by before it was the target.
        mining.target = Some(target);
        mining.elapsed = Duration::ZERO;
    }
    let needed = Duration::from_secs_f32(seconds);
    if mining.elapsed + TOLERANCE >= needed {
        // In survival the block's item has to fit in the miner's
        // inventory, or it stays put.
        if let Some(carrier) = carrier
            && let (mut inventory, mode) = carrier.into_inner()
            && !harvest(*mode, &mut inventory, &content, target.1)
        {
            mining.progress = 1.0;
            return;
        }
        voxels.set(hit.block, BlockState::AIR);
        *mining = Mining {
            armed: true,
            delay: BREAK_DELAY,
            ..default()
        };
    } else {
        mining.progress = (mining.elapsed.as_secs_f32() / seconds).min(1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::tests::{local_eyes, put_local_eyes};
    use crate::{
        content::{base, block},
        world::{Chunk, ChunkCoord, ChunkMap, VoxelChanged},
    };
    use bevy::time::TimeUpdateStrategy;

    /// Seconds per frame in these tests.
    const STEP: f32 = 0.05;

    /// One chunk with a stone at (5, 1, 5) and a dirt at (5, 1, 7), and a
    /// locked camera looking at the stone from three blocks away.
    fn app() -> App {
        app_with(base().clone(), Duration::from_secs_f32(STEP))
    }

    /// [`app`], with these blocks and this long a frame.
    fn app_with(content: Content, step: Duration) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(step))
            .insert_resource(content)
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ChunkMap>()
            .add_message::<VoxelChanged>()
            .add_message::<crate::world::PropertiesUpdated>()
            .add_message::<crate::world::FluidChanged>()
            .add_observer(give_the_player_mining)
            .add_systems(Update, (arm, mine).chain());
        app.world_mut().spawn((
            Window::default(),
            CursorOptions {
                grab_mode: CursorGrabMode::Locked,
                ..default()
            },
            PrimaryWindow,
        ));
        let mut chunk = Chunk::default();
        chunk.set(5, 1, 5, block("stone"));
        chunk.set(5, 1, 7, block("soil"));
        let entity = app.world_mut().spawn((ChunkCoord(IVec3::ZERO), chunk)).id();
        app.world_mut()
            .resource_mut::<ChunkMap>()
            .insert(ChunkCoord(IVec3::ZERO), entity);
        app.world_mut().spawn(crate::player::tests::local_player_at(
            Transform::from_xyz(2.5, 1.5, 5.5).looking_to(Vec3::X, Vec3::Y),
        ));
        // Lets the first frame, which has no time step, go by.
        app.update();
        app
    }

    fn at(app: &mut App, pos: IVec3) -> BlockState {
        let mut chunks = app.world_mut().query::<&Chunk>();
        chunks.single(app.world()).unwrap().at(pos)
    }

    fn set(app: &mut App, pos: IVec3, state: BlockState) {
        let mut chunks = app.world_mut().query::<&mut Chunk>();
        let mut chunk = chunks.single_mut(app.world_mut()).unwrap();
        chunk.set(pos.x as usize, pos.y as usize, pos.z as usize, state);
    }

    fn mining(app: &mut App) -> Mining {
        let mut miners = app.world_mut().query::<&Mining>();
        *miners.single(app.world()).unwrap()
    }

    fn mouse(app: &mut App) -> Mut<'_, ButtonInput<MouseButton>> {
        app.world_mut().resource_mut::<ButtonInput<MouseButton>>()
    }

    /// Runs a frame, with the button just pressed on it if `press`.
    fn frame(app: &mut App, press: bool) {
        if press {
            mouse(app).press(MouseButton::Left);
        }
        app.update();
        mouse(app).clear();
    }

    /// Frames, after the one the button goes down on, that a block of this
    /// state takes to break.
    fn frames_to_break(state: BlockState) -> usize {
        let seconds = break_time(base().state(state).hardness);
        (seconds / STEP - 1e-3).ceil() as usize
    }

    fn look_at(app: &mut App, target: Vec3) {
        let eyes = local_eyes(app).looking_at(target, Vec3::Y);
        put_local_eyes(app, eyes);
    }

    #[test]
    fn stone_takes_about_a_second_per_hardness_point() {
        assert!(base().state(block("stone")).hardness > 0.0);
        assert_eq!(break_time(2.0), 1.5);
        assert_eq!(break_time(0.0), 0.0);
    }

    #[test]
    fn holding_breaks_the_block_after_its_break_time_and_not_before() {
        let mut app = app();
        let stone = IVec3::new(5, 1, 5);
        let frames = frames_to_break(block("stone"));
        assert!(frames > 1);
        frame(&mut app, true);
        assert_eq!(mining(&mut app).target, Some((stone, block("stone"))));
        for i in 1..frames {
            frame(&mut app, false);
            assert_eq!(at(&mut app, stone), block("stone"), "broke at frame {i}");
            let progress = mining(&mut app).progress;
            assert!(
                (progress - i as f32 / frames as f32).abs() < 0.05,
                "{progress} at {i}"
            );
        }
        frame(&mut app, false);
        assert_eq!(at(&mut app, stone), BlockState::AIR);
        assert_eq!(mining(&mut app).target, None);
    }

    /// A pipe being mined that joins a pipe placed next to it is the same
    /// target: mining carries on rather than starting over.
    #[test]
    fn a_property_update_keeps_the_progress() {
        let mut app = app();
        let pos = IVec3::new(5, 1, 5);
        let content = base();
        let lone = content.default_state("defaria:pipe").unwrap();
        let joined = content
            .block_state("defaria:pipe", &[("south", "true")])
            .unwrap();
        set(&mut app, pos, lone);
        frame(&mut app, true);
        frame(&mut app, false);
        frame(&mut app, false);
        let before = mining(&mut app).progress;
        assert!(before > 0.0);
        set(&mut app, pos, joined);
        frame(&mut app, false);
        let after = mining(&mut app);
        assert!(after.progress > before, "{} after {before}", after.progress);
        assert_eq!(after.target, Some((pos, joined)));
    }

    #[test]
    fn releasing_starts_over() {
        let mut app = app();
        let stone = IVec3::new(5, 1, 5);
        let frames = frames_to_break(block("stone"));
        frame(&mut app, true);
        for _ in 1..frames {
            frame(&mut app, false);
        }
        mouse(&mut app).release(MouseButton::Left);
        frame(&mut app, false);
        assert_eq!(mining(&mut app), Mining::default());
        // Pressing again needs the whole time again.
        frame(&mut app, true);
        for _ in 1..frames {
            frame(&mut app, false);
        }
        assert_eq!(at(&mut app, stone), block("stone"));
        frame(&mut app, false);
        assert_eq!(at(&mut app, stone), BlockState::AIR);
    }

    #[test]
    fn looking_at_another_block_starts_over() {
        let mut app = app();
        let (stone, dirt) = (IVec3::new(5, 1, 5), IVec3::new(5, 1, 7));
        let frames = frames_to_break(block("stone"));
        frame(&mut app, true);
        for _ in 1..frames {
            frame(&mut app, false);
        }
        assert!(mining(&mut app).progress > 0.5);
        look_at(&mut app, dirt.as_vec3() + Vec3::splat(0.5));
        frame(&mut app, false);
        assert_eq!(mining(&mut app).target, Some((dirt, block("soil"))));
        assert_eq!(mining(&mut app).progress, 0.0);
        // Back to the stone: it starts over too.
        look_at(&mut app, stone.as_vec3() + Vec3::splat(0.5));
        frame(&mut app, false);
        assert_eq!(mining(&mut app).target, Some((stone, block("stone"))));
        assert_eq!(mining(&mut app).progress, 0.0);
        for _ in 1..frames {
            frame(&mut app, false);
        }
        assert_eq!(at(&mut app, stone), block("stone"));
        frame(&mut app, false);
        assert_eq!(at(&mut app, stone), BlockState::AIR);
    }

    /// Keep holding after a block breaks and the one behind it starts
    /// after the delay.
    #[test]
    fn holding_on_mines_the_next_block_after_a_delay() {
        let mut app = app();
        set(&mut app, IVec3::new(6, 1, 5), block("soil"));
        let frames = frames_to_break(block("stone"));
        frame(&mut app, true);
        for _ in 0..frames {
            frame(&mut app, false);
        }
        assert_eq!(at(&mut app, IVec3::new(5, 1, 5)), BlockState::AIR);
        let delay = (BREAK_DELAY.as_secs_f32() / STEP).round() as usize;
        for _ in 1..delay {
            frame(&mut app, false);
            assert_eq!(mining(&mut app).target, None);
        }
        frame(&mut app, false);
        assert_eq!(
            mining(&mut app).target,
            Some((IVec3::new(6, 1, 5), block("soil")))
        );
        for _ in 0..frames_to_break(block("soil")) {
            frame(&mut app, false);
        }
        assert_eq!(at(&mut app, IVec3::new(6, 1, 5)), BlockState::AIR);
    }

    /// The block changing under the crosshair, say to another state,
    /// starts over on the new block's whole time.
    #[test]
    fn the_block_changing_starts_over() {
        let mut app = app();
        let pos = IVec3::new(5, 1, 5);
        let frames = frames_to_break(block("stone"));
        frame(&mut app, true);
        for _ in 1..frames {
            frame(&mut app, false);
        }
        assert!(mining(&mut app).progress > 0.5);
        set(&mut app, pos, block("soil"));
        frame(&mut app, false);
        assert_eq!(mining(&mut app).target, Some((pos, block("soil"))));
        assert_eq!(mining(&mut app).progress, 0.0);
        let frames = frames_to_break(block("soil"));
        assert!(frames > 1);
        for _ in 1..frames {
            frame(&mut app, false);
        }
        assert_eq!(at(&mut app, pos), block("soil"));
        frame(&mut app, false);
        assert_eq!(at(&mut app, pos), BlockState::AIR);
    }

    /// A block with no hardness breaks on the frame it's targeted, and
    /// holding on still waits out the delay before the next one.
    #[test]
    fn a_block_with_no_hardness_breaks_at_once() {
        let mut content = base().clone();
        content.states[block("stone").0 as usize].hardness = 0.0;
        let mut app = app_with(content, Duration::from_secs_f32(STEP));
        let (first, second) = (IVec3::new(5, 1, 5), IVec3::new(6, 1, 5));
        set(&mut app, second, block("stone"));
        frame(&mut app, true);
        assert_eq!(at(&mut app, first), BlockState::AIR);
        assert_eq!(at(&mut app, second), block("stone"));
        let delay = (BREAK_DELAY.as_secs_f32() / STEP).round() as usize;
        for i in 1..delay {
            frame(&mut app, false);
            assert_eq!(at(&mut app, second), block("stone"), "broke at frame {i}");
        }
        frame(&mut app, false);
        assert_eq!(at(&mut app, second), BlockState::AIR);
    }

    /// At the headless runner's frame time (1/60 s, truncated to whole
    /// nanoseconds), a block that takes 0.75 s breaks on the 45th frame
    /// after the press, and the next starts 15 frames (0.25 s) later, not
    /// a frame late from rounding.
    #[test]
    fn breaks_on_the_frame_its_time_is_up() {
        let mut content = base().clone();
        content.states[block("stone").0 as usize].hardness = 1.0;
        let mut app = app_with(content, crate::headless::FRAME_TIME);
        let (stone, next) = (IVec3::new(5, 1, 5), IVec3::new(6, 1, 5));
        set(&mut app, next, block("stone"));
        frame(&mut app, true);
        for i in 1..45 {
            frame(&mut app, false);
            assert_eq!(at(&mut app, stone), block("stone"), "broke at frame {i}");
            assert!(mining(&mut app).progress < 1.0);
        }
        frame(&mut app, false);
        assert_eq!(at(&mut app, stone), BlockState::AIR);
        for i in 1..15 {
            frame(&mut app, false);
            assert_eq!(mining(&mut app).target, None, "started at frame {i}");
        }
        frame(&mut app, false);
        assert_eq!(mining(&mut app).target, Some((next, block("stone"))));
    }

    /// A press made while the cursor is free (the click that locks it)
    /// mines nothing, however long it's held.
    #[test]
    fn the_locking_click_mines_nothing() {
        let mut app = app();
        let mut cursors = app.world_mut().query::<&mut CursorOptions>();
        cursors.single_mut(app.world_mut()).unwrap().grab_mode = CursorGrabMode::None;
        frame(&mut app, true);
        let mut cursors = app.world_mut().query::<&mut CursorOptions>();
        cursors.single_mut(app.world_mut()).unwrap().grab_mode = CursorGrabMode::Locked;
        for _ in 0..frames_to_break(block("stone")) * 2 {
            frame(&mut app, false);
        }
        assert_eq!(mining(&mut app), Mining::default());
        assert_eq!(at(&mut app, IVec3::new(5, 1, 5)), block("stone"));
    }

    /// Opening a screen mid-mine starts over.
    #[test]
    fn opening_a_screen_starts_over() {
        let mut app = app();
        frame(&mut app, true);
        frame(&mut app, false);
        assert!(mining(&mut app).progress > 0.0);
        app.world_mut().spawn(UiScreen);
        frame(&mut app, false);
        assert_eq!(mining(&mut app), Mining::default());
    }
}
