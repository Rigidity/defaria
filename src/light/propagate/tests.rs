use super::*;
use crate::content::{base, block};

/// Chunks in memory, for driving the fill without an ECS world.
#[derive(Default, Clone)]
struct TestWorld {
    chunks: HashMap<IVec3, (Chunk, ChunkLight)>,
    /// Chunks with this `y` are under the sky.
    top: i32,
}

impl ChunkStore for TestWorld {
    fn with_chunk(&mut self, coord: IVec3, f: impl FnOnce(&Chunk, &mut ChunkLight)) -> bool {
        match self.chunks.get_mut(&coord) {
            Some((chunk, light)) => {
                f(chunk, light);
                true
            }
            None => false,
        }
    }
}

impl TestWorld {
    fn new(top: i32) -> Self {
        Self { top, ..default() }
    }

    fn table() -> LightTable {
        LightTable::new(base())
    }

    /// Adds a dark chunk, filled by `fill` (world position to block), and
    /// queues its light. Doesn't run the fill.
    fn load(&mut self, queue: &mut LightQueue, coord: IVec3, fill: impl Fn(IVec3) -> BlockState) {
        let mut chunk = Chunk::default();
        for index in 0..CHUNK_VOLUME {
            let local = Chunk::position_of(index);
            chunk.set_at(local, fill(coord * CHUNK_SIZE as i32 + local));
        }
        let loaded: HashSet<IVec3> = self.chunks.keys().copied().collect();
        queue.chunk_loaded(coord, &chunk, &Self::table(), coord.y == self.top, |c| {
            loaded.contains(&c)
        });
        self.chunks.insert(coord, (chunk, ChunkLight::default()));
    }

    fn run(&mut self, queue: &mut LightQueue) -> HashSet<IVec3> {
        let remesh = queue.run(self, &Self::table());
        assert!(queue.is_empty(), "work left over");
        remesh
    }

    fn block(&self, pos: IVec3) -> Option<BlockState> {
        let (coord, local) = split(pos);
        self.chunks.get(&coord).map(|(chunk, _)| chunk.at(local))
    }

    fn set(&mut self, queue: &mut LightQueue, pos: IVec3, state: BlockState) {
        let (coord, local) = split(pos);
        self.chunks.get_mut(&coord).unwrap().0.set_at(local, state);
        queue.voxel_changed(pos, coord.y == self.top);
    }

    fn light(&self, pos: IVec3) -> Light {
        let (coord, local) = split(pos);
        self.chunks[&coord].1.at(local)
    }

    /// Every voxel's light worked out from scratch, the slow obvious way:
    /// per channel, the brightest level any source reaches each voxel with.
    fn reference(&self) -> HashMap<IVec3, Light> {
        let table = Self::table();
        let size = CHUNK_SIZE as i32;
        let positions: Vec<IVec3> = self
            .chunks
            .keys()
            .flat_map(|&coord| (0..CHUNK_VOLUME).map(move |i| coord * size + Chunk::position_of(i)))
            .collect();
        let mut light: HashMap<IVec3, Light> =
            positions.iter().map(|&p| (p, Light::DARK)).collect();
        for channel in 0..CHANNELS {
            let mut buckets: Vec<Vec<IVec3>> = vec![Vec::new(); MAX_LIGHT as usize + 1];
            let level = |light: &mut HashMap<IVec3, Light>, pos: IVec3, value: u8| {
                let cell = light.get_mut(&pos).unwrap();
                if cell.get(channel) < value {
                    cell.set(channel, value);
                    true
                } else {
                    false
                }
            };
            for &pos in &positions {
                let state = self.block(pos).unwrap();
                let emitted = table.emits(state, channel);
                let sky = channel == SKY
                    && split(pos).0.y == self.top
                    && pos.y.rem_euclid(size) == size - 1
                    && !table.opaque(state);
                let source = if sky { MAX_LIGHT } else { emitted };
                if source > 0 && level(&mut light, pos, source) {
                    buckets[source as usize].push(pos);
                }
            }
            for value in (1..=MAX_LIGHT).rev() {
                while let Some(pos) = buckets[value as usize].pop() {
                    if light[&pos].get(channel) != value {
                        continue;
                    }
                    for (i, direction) in DIRECTIONS.into_iter().enumerate() {
                        let next = if channel == SKY && i == DOWN && value == MAX_LIGHT {
                            MAX_LIGHT
                        } else {
                            value - 1
                        };
                        let n = pos + direction;
                        let open = self.block(n).is_some_and(|state| !table.opaque(state));
                        if next > 0 && open && level(&mut light, n, next) {
                            buckets[next as usize].push(n);
                        }
                    }
                }
            }
        }
        light
    }

    /// Asserts every voxel's light matches [`TestWorld::reference`].
    fn assert_matches_reference(&self) {
        let reference = self.reference();
        let mut wrong: Vec<_> = reference
            .iter()
            .filter(|&(&pos, &expected)| self.light(pos) != expected)
            .map(|(&pos, &expected)| (pos, self.light(pos), expected))
            .collect();
        wrong.sort_by_key(|(pos, _, _)| pos.to_array());
        assert!(
            wrong.is_empty(),
            "{} voxels differ, (pos, got, expected): {:?}",
            wrong.len(),
            &wrong[..wrong.len().min(8)]
        );
    }
}

/// A small deterministic random number generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Caves of stone with lamps of every colour in them, the same for a seed.
fn random_block(seed: u64) -> impl Fn(IVec3) -> BlockState {
    move |pos: IVec3| {
        let mut rng = Rng(seed
            ^ (pos.x as u64 * 73_856_093)
            ^ (pos.y as u64 * 19_349_663)
            ^ (pos.z as u64 * 83_492_791)
            | 1);
        rng.next();
        match rng.below(1000) {
            0..=2 => block("lamp"),
            3..=5 => block("red_lamp"),
            6..=8 => block("blue_lamp"),
            9..=11 => block("green_lamp"),
            n if n < 400 => block("stone"),
            _ => BlockState::AIR,
        }
    }
}

fn coords(size: IVec3) -> Vec<IVec3> {
    let mut coords = Vec::new();
    for x in 0..size.x {
        for y in 0..size.y {
            for z in 0..size.z {
                coords.push(IVec3::new(x, y, z));
            }
        }
    }
    coords
}

/// Stone at and below `y = 3`, air above.
fn flat(pos: IVec3) -> BlockState {
    if pos.y <= 3 {
        block("stone")
    } else {
        BlockState::AIR
    }
}

#[test]
fn open_sky_is_full_strength_all_the_way_down() {
    let (mut world, mut queue) = (TestWorld::new(1), LightQueue::default());
    for coord in [IVec3::ZERO, IVec3::Y] {
        world.load(&mut queue, coord, flat);
    }
    world.run(&mut queue);
    for y in 4..32 {
        assert_eq!(world.light(IVec3::new(5, y, 7)).sky(), MAX_LIGHT, "y {y}");
    }
    assert_eq!(
        world.light(IVec3::new(5, 3, 7)),
        Light::DARK,
        "inside the ground"
    );
    assert_eq!(world.light(IVec3::new(5, 0, 7)), Light::DARK);
}

/// Under a roof, sky light comes in from the edge and fades a level per
/// block.
#[test]
fn sky_light_spreads_in_under_a_roof() {
    let (mut world, mut queue) = (TestWorld::new(0), LightQueue::default());
    world.load(&mut queue, IVec3::ZERO, |pos| {
        if pos.y <= 3 || (pos.y == 10 && pos.x < 8) {
            block("stone")
        } else {
            BlockState::AIR
        }
    });
    world.run(&mut queue);
    assert_eq!(world.light(IVec3::new(8, 5, 4)).sky(), MAX_LIGHT);
    for x in 0..8 {
        let expected = MAX_LIGHT - (8 - x) as u8;
        assert_eq!(world.light(IVec3::new(x, 5, 4)).sky(), expected, "x {x}");
    }
    assert_eq!(
        world.light(IVec3::new(3, 11, 4)).sky(),
        MAX_LIGHT,
        "on the roof"
    );
    world.assert_matches_reference();
}

/// A lamp in the dark: each channel drops a level per block walked, around
/// obstacles, never through them.
#[test]
fn lamp_light_drops_a_level_per_block() {
    let (mut world, mut queue) = (TestWorld::new(9), LightQueue::default());
    let lamp = IVec3::new(8, 8, 8);
    world.load(&mut queue, IVec3::ZERO, |pos| {
        if pos == lamp {
            block("red_lamp")
        } else if pos.x == 11 && pos.y < 12 {
            // A wall the light has to climb over.
            block("stone")
        } else {
            BlockState::AIR
        }
    });
    world.run(&mut queue);
    let red = base().state(block("red_lamp")).light;
    assert_eq!(world.light(lamp), Light::new(0, red));
    for d in 1..3 {
        let light = world.light(lamp + IVec3::new(-d, 0, 0));
        assert_eq!(
            light.block(),
            red.map(|l| l.saturating_sub(d as u8)),
            "{d} away"
        );
        assert_eq!(light.sky(), 0);
    }
    assert_eq!(
        world.light(IVec3::new(11, 8, 8)),
        Light::DARK,
        "in the wall"
    );
    // Over the wall: up 4, across 4 and down 4 is 12 steps.
    assert_eq!(world.light(IVec3::new(12, 8, 8)).block()[0], 3);
    world.assert_matches_reference();
}

#[test]
fn removing_a_lamp_takes_its_light_away() {
    let (mut world, mut queue) = (TestWorld::new(9), LightQueue::default());
    let lamp = IVec3::new(4, 8, 8);
    world.load(&mut queue, IVec3::ZERO, |pos| {
        if pos == lamp {
            block("lamp")
        } else {
            BlockState::AIR
        }
    });
    world.load(&mut queue, IVec3::X, |_| BlockState::AIR);
    world.run(&mut queue);
    assert!(
        world.light(IVec3::new(16, 8, 8)).block()[0] > 0,
        "reaches the next chunk"
    );
    world.set(&mut queue, lamp, BlockState::AIR);
    world.run(&mut queue);
    for (coord, (_, light)) in &world.chunks {
        assert_eq!(*light, ChunkLight::default(), "chunk {coord} still lit");
    }
}

/// Opening a hole in a roof lets the sky straight down through it, and
/// closing it again takes that away.
#[test]
fn opening_and_closing_a_roof() {
    let (mut world, mut queue) = (TestWorld::new(1), LightQueue::default());
    let roof = |pos: IVec3| pos.y <= 3 || pos.y == 20;
    for coord in [IVec3::ZERO, IVec3::Y] {
        world.load(&mut queue, coord, |pos| {
            if roof(pos) {
                block("stone")
            } else {
                BlockState::AIR
            }
        });
    }
    world.run(&mut queue);
    assert_eq!(world.light(IVec3::new(8, 10, 8)).sky(), 0, "sealed in");
    let hole = IVec3::new(8, 20, 8);
    world.set(&mut queue, hole, BlockState::AIR);
    world.run(&mut queue);
    assert_eq!(world.light(IVec3::new(8, 4, 8)).sky(), MAX_LIGHT);
    assert_eq!(world.light(IVec3::new(5, 4, 8)).sky(), MAX_LIGHT - 3);
    world.assert_matches_reference();
    world.set(&mut queue, hole, block("stone"));
    world.run(&mut queue);
    assert_eq!(world.light(IVec3::new(8, 10, 8)).sky(), 0);
    world.assert_matches_reference();
}

/// However the chunks arrive (all at once, or one at a time in any order,
/// with the fill running in between), the light comes out the same, and
/// right.
#[test]
fn load_order_doesnt_change_the_light() {
    let size = IVec3::new(3, 2, 2);
    let fill = random_block(7);
    let (mut all, mut queue) = (TestWorld::new(size.y - 1), LightQueue::default());
    for coord in coords(size) {
        all.load(&mut queue, coord, &fill);
    }
    all.run(&mut queue);
    all.assert_matches_reference();

    for seed in [1, 2, 3] {
        let mut order = coords(size);
        let mut rng = Rng(seed * 0x9E37_79B9);
        for i in (1..order.len()).rev() {
            order.swap(i, rng.below(i as u64 + 1) as usize);
        }
        let mut world = TestWorld::new(size.y - 1);
        for coord in order {
            world.load(&mut queue, coord, &fill);
            world.run(&mut queue);
        }
        for (coord, (_, light)) in &world.chunks {
            assert!(
                *light == all.chunks[coord].1,
                "seed {seed}: chunk {coord} differs"
            );
        }
    }
}

/// After any series of edits (digging, building, placing and breaking
/// lamps), the light is what a fresh fill of the edited world gives.
#[test]
fn edits_keep_the_light_right() {
    let size = IVec3::new(2, 2, 2);
    let (mut world, mut queue) = (TestWorld::new(size.y - 1), LightQueue::default());
    for coord in coords(size) {
        world.load(&mut queue, coord, random_block(11));
    }
    world.run(&mut queue);
    let mut rng = Rng(0x0DEF_A41A);
    let blocks = [
        BlockState::AIR,
        BlockState::AIR,
        block("stone"),
        block("lamp"),
        block("red_lamp"),
        block("blue_lamp"),
    ];
    for _ in 0..12 {
        // Several edits per run, some next to each other.
        let edits = 1 + rng.below(4);
        let mut at = IVec3::new(
            rng.below(32) as i32,
            rng.below(32) as i32,
            rng.below(32) as i32,
        );
        for _ in 0..edits {
            let state = blocks[rng.below(blocks.len() as u64) as usize];
            world.set(&mut queue, at, state);
            at = (at + DIRECTIONS[rng.below(6) as usize]).clamp(IVec3::ZERO, IVec3::splat(31));
        }
        world.run(&mut queue);
        world.assert_matches_reference();
    }
}

/// The fill reports the chunks whose meshes show changed light: the chunk
/// itself, plus the chunks across any border where light changed.
#[test]
fn reports_the_chunks_to_remesh() {
    let (mut world, mut queue) = (TestWorld::new(9), LightQueue::default());
    // Solid stone, with a one-voxel hole in the middle and one on the -X
    // face.
    let (middle, edge) = (IVec3::new(8, 8, 8), IVec3::new(0, 8, 8));
    let size = IVec3::new(2, 1, 1);
    for coord in coords(size) {
        world.load(&mut queue, coord - IVec3::X, |pos| {
            if pos == middle || pos == edge {
                BlockState::AIR
            } else {
                block("stone")
            }
        });
    }
    assert!(world.run(&mut queue).is_empty(), "nothing lit");
    world.set(&mut queue, middle, block("lamp"));
    assert_eq!(world.run(&mut queue), HashSet::from([IVec3::ZERO]));
    world.set(&mut queue, edge, block("lamp"));
    assert_eq!(
        world.run(&mut queue),
        HashSet::from([IVec3::ZERO, IVec3::NEG_X])
    );
    // Setting it to what it already is changes nothing.
    world.set(&mut queue, edge, block("lamp"));
    assert!(world.run(&mut queue).is_empty());
}

/// Light changing at a voxel on a chunk's corner reports all eight chunks
/// that meet there: the corner shades faces in each of them.
#[test]
fn light_on_a_chunk_corner_remeshes_every_chunk_there() {
    let (mut world, mut queue) = (TestWorld::new(9), LightQueue::default());
    let corner = IVec3::new(0, 15, 15);
    let chunks: Vec<IVec3> = coords(IVec3::splat(2))
        .into_iter()
        .map(|c| c - IVec3::X)
        .collect();
    for &coord in &chunks {
        world.load(&mut queue, coord, |pos| {
            if pos == corner {
                BlockState::AIR
            } else {
                block("stone")
            }
        });
    }
    world.run(&mut queue);
    world.set(&mut queue, corner, block("lamp"));
    let remesh = world.run(&mut queue);
    assert_eq!(remesh, chunks.into_iter().collect::<HashSet<_>>());
}

/// The top layer of the world gets its sky from above the world: breaking
/// a block there, or a hole in a roof there, lets the sky straight back in.
#[test]
fn the_sky_comes_back_into_the_top_layer() {
    let (mut world, mut queue) = (TestWorld::new(0), LightQueue::default());
    world.load(&mut queue, IVec3::ZERO, |pos| {
        if pos.y <= 3 || pos.y == 15 {
            block("stone")
        } else {
            BlockState::AIR
        }
    });
    world.run(&mut queue);
    assert_eq!(world.light(IVec3::new(8, 8, 8)).sky(), 0, "under the roof");
    let hole = IVec3::new(8, 15, 8);
    world.set(&mut queue, hole, BlockState::AIR);
    world.run(&mut queue);
    assert_eq!(world.light(hole).sky(), MAX_LIGHT);
    assert_eq!(world.light(IVec3::new(8, 4, 8)).sky(), MAX_LIGHT);
    world.assert_matches_reference();
    // Place and break a block in open sky at the top.
    let open = IVec3::new(3, 14, 3);
    world.set(&mut queue, hole, block("stone"));
    world.set(&mut queue, IVec3::new(3, 15, 3), BlockState::AIR);
    world.run(&mut queue);
    world.assert_matches_reference();
    world.set(&mut queue, IVec3::new(3, 15, 3), block("stone"));
    world.run(&mut queue);
    world.set(&mut queue, IVec3::new(3, 15, 3), BlockState::AIR);
    world.run(&mut queue);
    assert_eq!(world.light(open).sky(), MAX_LIGHT);
    world.assert_matches_reference();
}

/// Many random worlds with chunks loading in random order while voxels
/// change in the same runs, checked against the reference after every run.
#[test]
fn random_loads_and_edits_keep_the_light_right() {
    let blocks = [
        BlockState::AIR,
        BlockState::AIR,
        BlockState::AIR,
        block("stone"),
        block("stone"),
        block("lamp"),
        block("red_lamp"),
        block("blue_lamp"),
    ];
    let size = IVec3::new(3, 2, 2);
    for seed in 1..10u64 {
        let mut rng = Rng((seed * 0x9E37_79B9) | 1);
        let fill = random_block(seed);
        let (mut world, mut queue) = (TestWorld::new(size.y - 1), LightQueue::default());
        let mut order = coords(size);
        for i in (1..order.len()).rev() {
            order.swap(i, rng.below(i as u64 + 1) as usize);
        }
        let rest = order.split_off(order.len() / 2);
        for &coord in &order {
            world.load(&mut queue, coord, &fill);
        }
        world.run(&mut queue);
        let mut rest = rest.into_iter();
        for step in 0..20 {
            if rng.below(3) == 0
                && let Some(coord) = rest.next()
            {
                world.load(&mut queue, coord, &fill);
            }
            let loaded: Vec<IVec3> = world.chunks.keys().copied().collect();
            for _ in 0..1 + rng.below(8) {
                let coord = loaded[rng.below(loaded.len() as u64) as usize];
                let local = IVec3::new(
                    rng.below(16) as i32,
                    rng.below(16) as i32,
                    rng.below(16) as i32,
                );
                let state = blocks[rng.below(blocks.len() as u64) as usize];
                world.set(&mut queue, coord * CHUNK_SIZE as i32 + local, state);
            }
            world.run(&mut queue);
            let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                world.assert_matches_reference()
            }));
            assert!(checked.is_ok(), "seed {seed}, step {step}");
        }
    }
}

/// Light doesn't cross into a chunk that isn't loaded, and fills in when it
/// loads.
#[test]
fn light_waits_for_chunks_to_load() {
    let (mut world, mut queue) = (TestWorld::new(1), LightQueue::default());
    world.load(&mut queue, IVec3::ZERO, flat);
    world.run(&mut queue);
    assert_eq!(
        world.light(IVec3::new(4, 8, 4)).sky(),
        0,
        "nothing above yet"
    );
    world.load(&mut queue, IVec3::Y, flat);
    world.run(&mut queue);
    assert_eq!(world.light(IVec3::new(4, 8, 4)).sky(), MAX_LIGHT);
    world.assert_matches_reference();
}
