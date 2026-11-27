//! Mining: holding left click breaks the block under the crosshair after a
//! time set by its hardness (see `defaria_core::mining` for the rules,
//! and [`Mining`], which both sides know).

use std::time::Duration;

use bevy::prelude::*;

use defaria_core::mining::{BREAK_DELAY, Mining, MiningSystems, break_time};

use crate::{block_edit::BlockEditSystems, support::unsupported};
use defaria_core::{
    block_edit::crosshair_target,
    content::{BlockState, Content},
    game_mode::{GameMode, harvest},
    inventory::Inventory,
    player::{Eyes, Player},
    world::{RemeshChunks, Voxels},
};

/// How close to a break time or the end of [`BREAK_DELAY`] counts as
/// there, so frame times that don't add up exactly (1/60 s truncated to
/// whole nanoseconds) don't cost a whole extra frame.
const TOLERANCE: Duration = Duration::from_micros(1);

pub struct MiningPlugin;

impl Plugin for MiningPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Mining>()
            .add_observer(give_the_player_mining)
            .add_systems(
                Update,
                // After placing, so the two edits on one frame always land
                // in the same order. Before remeshing, so a break shows
                // this frame.
                mine.in_set(MiningSystems)
                    .after(BlockEditSystems)
                    .before(RemeshChunks),
            );
    }
}

fn give_the_player_mining(add: On<Add<Player>>, mut commands: Commands) {
    commands.entity(add.entity).insert(Mining::default());
}

/// Mines the block under each player's crosshair while it holds the
/// button, and breaks it once its break time is up.
fn mine(
    time: Res<Time>,
    content: Res<Content>,
    // What a miner gets for it, in survival.
    mut miners: Query<(Eyes, &mut Mining, Option<(&mut Inventory, &GameMode)>)>,
    mut voxels: Voxels,
) {
    for (eyes, mut mining, carrier) in &mut miners {
        mine_one(
            &time,
            &content,
            &eyes.transform(),
            &mut mining,
            carrier,
            &mut voxels,
        );
    }
}

fn mine_one(
    time: &Time,
    content: &Content,
    transform: &Transform,
    mining: &mut Mut<Mining>,
    mut carrier: Option<(Mut<Inventory>, &GameMode)>,
    voxels: &mut Voxels,
) {
    if !mining.holding {
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
    let Some(hit) = crosshair_target(transform, content, voxels) else {
        mining.target = None;
        mining.progress = 0.0;
        mining.elapsed = Duration::ZERO;
        return;
    };
    let whole = voxels.get(hit.block).unwrap_or(BlockState::AIR);
    // In a voxel of two halves, the half under the crosshair.
    let target = (hit.block, content.half_at(whole, hit.shape_box));
    let seconds = break_time(content.state(target.1).hardness);
    // The engine updating its properties (a pipe joining one placed next
    // to it) doesn't make it another target, but the other half of the
    // voxel is.
    let same = mining.target.is_some_and(|(pos, state)| {
        pos == target.0
            && content.state(state).block == content.state(target.1).block
            && content.half(state) == content.half(target.1)
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
        if let Some((inventory, mode)) = &mut carrier
            && !harvest(**mode, inventory, content, target.1)
        {
            mining.progress = 1.0;
            return;
        }
        // The other half stays.
        voxels.set(hit.block, content.without(whole, target.1));
        // What stood on it and needed it (a flower on grass) breaks with
        // it, into the same inventory if it fits. (Otherwise `support`
        // breaks it, giving nothing.)
        let above = hit.block + IVec3::Y;
        if let Some(plant) = unsupported(content, voxels, above) {
            if let Some((inventory, mode)) = &mut carrier {
                harvest(**mode, inventory, content, plant);
            }
            voxels.set(above, BlockState::AIR);
        }
        **mining = Mining {
            holding: true,
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
    use defaria_core::player::tests::{local_eyes, put_local_eyes};
    use defaria_core::{
        content::{base, block},
        testing::{chunk_app, insert_chunk},
        world::Chunk,
    };

    /// Seconds per frame in these tests.
    const STEP: f32 = 0.05;

    /// One chunk with a stone at (5, 1, 5) and a dirt at (5, 1, 7), and a
    /// locked camera looking at the stone from three blocks away.
    fn app() -> App {
        app_with(base().clone(), Duration::from_secs_f32(STEP))
    }

    /// [`app`], with these blocks and this long a frame.
    fn app_with(content: Content, step: Duration) -> App {
        let mut app = chunk_app(content, step);
        app.init_resource::<Held>()
            .add_observer(give_the_player_mining)
            .add_systems(Update, (hold_the_button, mine).chain());
        let mut chunk = Chunk::default();
        chunk.set(5, 1, 5, block("stone"));
        chunk.set(5, 1, 7, block("soil"));
        insert_chunk(&mut app, IVec3::ZERO, chunk);
        app.world_mut()
            .spawn(defaria_core::player::tests::local_player_at(
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

    /// Whether the player holds the break button, as input would say.
    #[derive(Resource, Default)]
    struct Held(bool);

    fn hold_the_button(held: Res<Held>, mut miners: Query<&mut Mining>) {
        for mut mining in &mut miners {
            mining.holding = held.0;
        }
    }

    fn release(app: &mut App) {
        app.world_mut().resource_mut::<Held>().0 = false;
    }

    /// Runs a frame, with the button pressed on it if `press`.
    fn frame(app: &mut App, press: bool) {
        if press {
            app.world_mut().resource_mut::<Held>().0 = true;
        }
        app.update();
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
        release(&mut app);
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
        // The headless runner's frame: 1/60 s, truncated to whole nanoseconds.
        let mut app = app_with(content, Duration::from_nanos(1_000_000_000 / 60));
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

    /// Mining the grass under a flower in survival gives the flower too;
    /// in creative it breaks with the grass, giving nothing.
    #[test]
    fn mining_the_ground_under_a_plant_harvests_it_too() {
        for mode in [GameMode::Survival, GameMode::Creative] {
            let mut app = app();
            let ground = IVec3::new(5, 1, 7);
            let flower = ground + IVec3::Y;
            set(&mut app, ground, defaria_core::content::grass());
            set(&mut app, flower, block("daisy"));
            let player = defaria_core::player::tests::local(&mut app);
            app.world_mut()
                .entity_mut(player)
                .insert((Inventory::new(), mode));
            look_at(&mut app, ground.as_vec3() + Vec3::new(0.5, 0.9, 0.5));
            frame(&mut app, true);
            for _ in 0..frames_to_break(defaria_core::content::grass()) {
                frame(&mut app, false);
            }
            assert_eq!(at(&mut app, ground), BlockState::AIR, "{mode:?}");
            assert_eq!(at(&mut app, flower), BlockState::AIR, "{mode:?}");
            let daisy = defaria_core::game_mode::item_for_block(base(), block("daisy")).unwrap();
            let inventory = app.world().get::<Inventory>(player).unwrap();
            let expected = if mode.is_creative() { 0 } else { 1 };
            assert_eq!(inventory.count(daisy), expected, "{mode:?}");
        }
    }

    /// In survival, a plant whose item doesn't fit breaks anyway, giving
    /// nothing, as long as the ground under it fits.
    #[test]
    fn a_plant_that_doesnt_fit_breaks_without_its_item() {
        let mut app = app();
        let ground = IVec3::new(5, 1, 7);
        let flower = ground + IVec3::Y;
        set(&mut app, ground, defaria_core::content::grass());
        set(&mut app, flower, block("daisy"));
        let grass = defaria_core::game_mode::item_for_block(base(), defaria_core::content::grass())
            .unwrap();
        let daisy = defaria_core::game_mode::item_for_block(base(), block("daisy")).unwrap();
        // Room for the grass and not a gram more.
        let room = defaria_core::inventory::model::item_weight(base(), grass);
        let player = defaria_core::player::tests::local(&mut app);
        app.world_mut()
            .entity_mut(player)
            .insert((Inventory::new().with_max_weight(room), GameMode::Survival));
        look_at(&mut app, ground.as_vec3() + Vec3::new(0.5, 0.9, 0.5));
        frame(&mut app, true);
        for _ in 0..frames_to_break(defaria_core::content::grass()) {
            frame(&mut app, false);
        }
        assert_eq!(at(&mut app, ground), BlockState::AIR);
        assert_eq!(at(&mut app, flower), BlockState::AIR);
        let inventory = app.world().get::<Inventory>(player).unwrap();
        assert_eq!(inventory.count(grass), 1);
        assert_eq!(inventory.count(daisy), 0);
    }

    /// A block on the one mined that doesn't need support (a log on the
    /// grass) stays, and isn't harvested.
    #[test]
    fn mining_leaves_blocks_that_need_no_support() {
        let mut app = app();
        let ground = IVec3::new(5, 1, 7);
        let log = ground + IVec3::Y;
        set(&mut app, ground, defaria_core::content::grass());
        set(&mut app, log, block("log"));
        let player = defaria_core::player::tests::local(&mut app);
        app.world_mut()
            .entity_mut(player)
            .insert((Inventory::new(), GameMode::Survival));
        look_at(&mut app, ground.as_vec3() + Vec3::new(0.5, 0.9, 0.5));
        frame(&mut app, true);
        for _ in 0..frames_to_break(defaria_core::content::grass()) {
            frame(&mut app, false);
        }
        assert_eq!(at(&mut app, ground), BlockState::AIR);
        assert_eq!(at(&mut app, log), block("log"));
        let inventory = app.world().get::<Inventory>(player).unwrap();
        assert_eq!(inventory.entries().len(), 1, "only the grass");
    }
    /// Mining a voxel of two halves breaks the half under the crosshair,
    /// taking that half's time and giving its item in survival, and
    /// leaves the other. Turning to the other half starts over.
    #[test]
    fn mining_a_double_slab_breaks_the_half_under_the_crosshair() {
        let content = base();
        let half = |id: &str, values: &[(&str, &str)]| content.block_state(id, values).unwrap();
        let bottom = half("defaria:stone_slab", &[("half", "bottom")]);
        let top = half("defaria:thatch_slab", &[("half", "top")]);
        let both = content.combine(bottom, top).unwrap();
        let mut app = app();
        let pos = IVec3::new(5, 1, 7);
        set(&mut app, pos, both);
        let player = defaria_core::player::tests::local(&mut app);
        app.world_mut()
            .entity_mut(player)
            .insert((Inventory::new(), GameMode::Survival));
        // The bottom half, then a frame on the top half: that's a new
        // target, its progress from nothing.
        look_at(&mut app, pos.as_vec3() + Vec3::new(0.5, 0.25, 0.0));
        frame(&mut app, true);
        frame(&mut app, false);
        assert_eq!(mining(&mut app).target, Some((pos, bottom)));
        assert!(mining(&mut app).progress > 0.0);
        look_at(&mut app, pos.as_vec3() + Vec3::new(0.5, 0.75, 0.0));
        frame(&mut app, false);
        assert_eq!(mining(&mut app).target, Some((pos, top)));
        assert_eq!(mining(&mut app).progress, 0.0);
        // The thatch takes its own time, and leaves the stone.
        for _ in 0..frames_to_break(top) {
            frame(&mut app, false);
        }
        assert_eq!(at(&mut app, pos), bottom);
        let item = |state| defaria_core::game_mode::item_for_block(base(), state).unwrap();
        let inventory = app.world().get::<Inventory>(player).unwrap();
        assert_eq!(inventory.count(item(top)), 1);
        assert_eq!(inventory.count(item(bottom)), 0);
    }

    /// Two halves of the same block are still two targets: turning from
    /// one to the other starts over, so the second can't break early.
    #[test]
    fn halves_of_one_block_are_mined_separately() {
        let content = base();
        let half = |h: &str| {
            content
                .block_state("defaria:stone_slab", &[("half", h)])
                .unwrap()
        };
        let (bottom, top) = (half("bottom"), half("top"));
        let mut app = app();
        let pos = IVec3::new(5, 1, 7);
        set(&mut app, pos, content.combine(bottom, top).unwrap());
        look_at(&mut app, pos.as_vec3() + Vec3::new(0.5, 0.25, 0.0));
        frame(&mut app, true);
        for _ in 0..frames_to_break(bottom) - 1 {
            frame(&mut app, false);
        }
        assert!(mining(&mut app).progress > 0.5);
        look_at(&mut app, pos.as_vec3() + Vec3::new(0.5, 0.75, 0.0));
        frame(&mut app, false);
        assert_eq!(mining(&mut app).target, Some((pos, top)));
        assert_eq!(mining(&mut app).progress, 0.0);
        frame(&mut app, false);
        assert_eq!(at(&mut app, pos), content.combine(bottom, top).unwrap());
    }
}
