use std::sync::Arc;

use super::*;
use crate::light::sky::AssumeOpenSky;
use defaria_core::content::{FULL_FLUID, base, block};

/// Chunks in memory, for driving the fill without an ECS world.
#[derive(Clone)]
struct TestWorld {
    chunks: HashMap<IVec3, (Chunk, ChunkLight)>,
    sky: Arc<dyn OutsideSky>,
}

/// Sky shines in from outside only at `y` and above: a world with nothing
/// below `y` lit from above but what's loaded. The tests written before the
/// world had no top use it to keep a fixed top.
struct SkyFrom(i32);

impl OutsideSky for SkyFrom {
    fn shines_into(&self, chunk: IVec3, _: i32, _: i32) -> bool {
        chunk.y >= self.0
    }
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

    fn is_loaded(&self, coord: IVec3) -> bool {
        self.chunks.contains_key(&coord)
    }
}

impl TestWorld {
    /// A world whose top chunks are at `y = top`: sky light comes from
    /// outside only into those.
    fn new(top: i32) -> Self {
        Self::with_sky(SkyFrom(top))
    }

    /// A world with open sky above whatever's loaded, as the game has.
    fn open() -> Self {
        Self::with_sky(AssumeOpenSky)
    }

    fn with_sky(sky: impl OutsideSky) -> Self {
        Self {
            chunks: default(),
            sky: Arc::new(sky),
        }
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
            chunk.put(local, fill(coord * CHUNK_SIZE as i32 + local));
        }
        queue.chunk_loaded(coord, &chunk, &Self::table(), &*self.sky, |c| {
            self.chunks.get(&c).map(|(chunk, light)| (chunk, light))
        });
        self.chunks.insert(coord, (chunk, ChunkLight::default()));
    }

    /// Adds a chunk lit by itself, as loading tasks do, and queues joining
    /// it to the world.
    fn load_lit(
        &mut self,
        queue: &mut LightQueue,
        coord: IVec3,
        fill: impl Fn(IVec3) -> BlockState,
    ) {
        let mut chunk = Chunk::default();
        for index in 0..CHUNK_VOLUME {
            let local = Chunk::position_of(index);
            chunk.put(local, fill(coord * CHUNK_SIZE as i32 + local));
        }
        let light = light_alone(&[(coord, &chunk)], &Self::table(), &*self.sky)
            .pop()
            .unwrap();
        queue.lit_chunk_loaded(coord, &chunk, &light, &Self::table(), &*self.sky, |c| {
            self.chunks.get(&c).map(|(chunk, light)| (chunk, light))
        });
        self.chunks.insert(coord, (chunk, light));
    }

    /// Takes a chunk away and queues what that does to the light.
    fn unload(&mut self, queue: &mut LightQueue, coord: IVec3) {
        let (_, light) = self.chunks.remove(&coord).unwrap();
        queue.chunk_unloaded(coord, &light, &*self.sky, |c| self.chunks.contains_key(&c));
    }

    fn run(&mut self, queue: &mut LightQueue) -> HashSet<IVec3> {
        let sky = self.sky.clone();
        let remesh = queue.run(self, &Self::table(), &*sky);
        assert!(queue.is_empty(), "work left over");
        remesh
    }

    /// What light sees at `pos`: its block and its fluid.
    fn voxel(&self, pos: IVec3) -> Option<Lit> {
        let (coord, local) = split(pos);
        let (chunk, _) = self.chunks.get(&coord)?;
        Some(Lit::at(chunk, Chunk::index_of(local)))
    }

    fn set(&mut self, queue: &mut LightQueue, pos: IVec3, state: BlockState) {
        let (coord, local) = split(pos);
        self.chunks.get_mut(&coord).unwrap().0.put(local, state);
        queue.voxel_changed(pos);
    }

    /// Puts a full voxel of the fluid `kind` in with the block at `pos`.
    fn pour(&mut self, queue: &mut LightQueue, pos: IVec3, kind: BlockState) {
        let (coord, local) = split(pos);
        self.chunks
            .get_mut(&coord)
            .unwrap()
            .0
            .fill_fluid_at(local, kind);
        queue.voxel_changed(pos);
    }

    /// Leaves `amount` of the fluid `kind` in the voxel at `pos`, as flow
    /// does.
    fn pour_some(&mut self, queue: &mut LightQueue, pos: IVec3, kind: BlockState, amount: u16) {
        let (coord, local) = split(pos);
        let chunk = &mut self.chunks.get_mut(&coord).unwrap().0;
        let room = base().state(chunk.at(local)).fluid_room;
        chunk.set_fluid_at(
            local,
            defaria_core::world::VoxelFluid::of(kind, amount),
            room,
        );
        queue.voxel_changed(pos);
    }

    /// Takes the fluid out of the voxel at `pos`, as flow draining it does.
    fn drain(&mut self, queue: &mut LightQueue, pos: IVec3) {
        let (coord, local) = split(pos);
        let chunk = &mut self.chunks.get_mut(&coord).unwrap().0;
        chunk.set_fluid_at(local, None, defaria_core::content::FULL_FLUID);
        queue.voxel_changed(pos);
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
                let state = self.voxel(pos).unwrap();
                let emitted = table.emits(state, channel);
                let (coord, local) = split(pos);
                let sky = channel == SKY
                    && local.y == size - 1
                    && !self.chunks.contains_key(&(coord + IVec3::Y))
                    && self.sky.shines_into(coord, local.x, local.z)
                    && !table.closed(state, UP);
                // The sky from outside comes into the top voxel from above,
                // so it loses what that voxel filters out.
                let source = if sky {
                    table.entering(state, MAX_LIGHT)
                } else {
                    emitted
                };
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
                        let from = self.voxel(pos).unwrap();
                        let next = table.offered(from, value, channel, i);
                        if next == 0 {
                            continue;
                        }
                        let n = pos + direction;
                        let next = self
                            .voxel(n)
                            .map_or(0, |to| table.passing(from, to, i, channel, next));
                        if next > 0 && level(&mut light, n, next) {
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

/// Caves of stone with lamps of every colour, slabs, water and leaves in
/// them, the same for a seed.
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
            12..=40 => block("stone_slab"),
            41..=70 => block("water"),
            71..=100 => block("leaves"),
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

/// The [`DIRECTIONS`] index of up.
const UP: usize = 2;

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

/// A slab covers only its bottom: sky light comes into it from above and
/// the sides, but not through it to what's below, so a slab roof shades
/// like a full one, and the slab's own voxel (which lights its top) is lit.
#[test]
fn slabs_stop_light_only_at_their_bottom() {
    let (mut world, mut queue) = (TestWorld::new(0), LightQueue::default());
    world.load(&mut queue, IVec3::ZERO, |pos| {
        if pos.y <= 3 {
            block("stone")
        } else if pos.y == 10 && pos.x < 8 {
            block("stone_slab")
        } else {
            BlockState::AIR
        }
    });
    world.run(&mut queue);
    // In the slabs: full sky, straight down from above.
    assert_eq!(world.light(IVec3::new(3, 10, 4)).sky(), MAX_LIGHT);
    // Under them, as under a full roof: in from the open side.
    for x in 0..8 {
        let expected = MAX_LIGHT - (8 - x) as u8;
        assert_eq!(world.light(IVec3::new(x, 5, 4)).sky(), expected, "x {x}");
    }
    world.assert_matches_reference();

    // Light from below doesn't come up through a slab's bottom either.
    let lamp = IVec3::new(3, 9, 4);
    world.set(&mut queue, lamp, block("lamp"));
    world.run(&mut queue);
    assert_eq!(world.light(IVec3::new(3, 8, 4)).block()[0], 14);
    // Only the long way round, out from under the roof at x = 8 and back
    // along the slabs: 11 blocks, not 1.
    assert_eq!(world.light(IVec3::new(3, 10, 4)).block()[0], 4);
    world.assert_matches_reference();

    // Taking a slab away lets the sky straight down again.
    world.set(&mut queue, IVec3::new(5, 10, 4), BlockState::AIR);
    world.run(&mut queue);
    assert_eq!(world.light(IVec3::new(5, 5, 4)).sky(), MAX_LIGHT);
    world.assert_matches_reference();
}

/// Two halves in a voxel stop light like a full block: a roof of double
/// slabs (a stone bottom under a planks top) is dark inside and shades
/// what's under it as a stone roof does, and a wall of vertical slab pairs
/// stops a lamp's light from crossing it, which one slab alone doesn't.
#[test]
fn two_halves_in_a_voxel_stop_light_like_a_block() {
    let content = base();
    let half = |id: &str, half: &str| content.block_state(id, &[("half", half)]).unwrap();
    let double = content
        .combine(
            half("defaria:stone_slab", "bottom"),
            half("defaria:planks_slab", "top"),
        )
        .unwrap();
    let roofed = |roof: BlockState| {
        move |pos: IVec3| {
            if pos.y <= 3 {
                block("stone")
            } else if pos.y == 10 && pos.x < 8 {
                roof
            } else {
                BlockState::AIR
            }
        }
    };
    let (mut world, mut queue) = (TestWorld::new(0), LightQueue::default());
    world.load(&mut queue, IVec3::ZERO, roofed(double));
    world.run(&mut queue);
    let (mut stone, mut stone_queue) = (TestWorld::new(0), LightQueue::default());
    stone.load(&mut stone_queue, IVec3::ZERO, roofed(block("stone")));
    stone.run(&mut stone_queue);
    assert_eq!(world.light(IVec3::new(3, 10, 4)).sky(), 0);
    for x in 0..16 {
        for y in 4..16 {
            let pos = IVec3::new(x, y, 4);
            assert_eq!(world.light(pos), stone.light(pos), "{pos}");
        }
    }
    world.assert_matches_reference();

    // A lamp west of a wall at x = 8: an east slab alone lets its light
    // in through its open west side and on east across the voxel, but a
    // west slab put in front of it closes the wall.
    let (mut world, mut queue) = (TestWorld::new(0), LightQueue::default());
    world.load(&mut queue, IVec3::ZERO, |pos| {
        if pos.x == 8 {
            half("defaria:stone_vertical_slab", "east")
        } else if pos.y == 0 || pos.y == 15 || pos.z == 0 || pos.z == 15 {
            block("stone")
        } else {
            BlockState::AIR
        }
    });
    world.set(&mut queue, IVec3::new(5, 7, 7), block("lamp"));
    world.run(&mut queue);
    // Into the slab's voxel through its open side, but not out east.
    assert!(world.light(IVec3::new(8, 7, 7)).block()[0] > 0);
    assert_eq!(world.light(IVec3::new(9, 7, 7)).block()[0], 0);
    world.assert_matches_reference();
    world.set(
        &mut queue,
        IVec3::new(8, 7, 7),
        content
            .combine(
                half("defaria:stone_vertical_slab", "west"),
                half("defaria:stone_vertical_slab", "east"),
            )
            .unwrap(),
    );
    world.run(&mut queue);
    assert_eq!(world.light(IVec3::new(8, 7, 7)).block()[0], 0);
    world.assert_matches_reference();
}

/// See-through blocks with no light filter (glass, plants) let light
/// through as if they were air, though they cover their sides: a roof of
/// them shades nothing, and a lamp shines through them.
#[test]
fn see_through_blocks_let_light_through() {
    for name in ["glass", "tuft"] {
        let (mut world, mut queue) = (TestWorld::new(0), LightQueue::default());
        world.load(&mut queue, IVec3::ZERO, |pos| {
            if pos.y <= 3 {
                block("stone")
            } else if (8..=10).contains(&pos.y) {
                block(name)
            } else {
                BlockState::AIR
            }
        });
        world.run(&mut queue);
        for y in [9, 5] {
            let at = IVec3::new(3, y, 4);
            assert_eq!(world.light(at).sky(), MAX_LIGHT, "{name} at {at}");
        }
        let lamp = IVec3::new(3, 6, 4);
        world.set(&mut queue, lamp, block("lamp"));
        world.run(&mut queue);
        assert_eq!(world.light(IVec3::new(3, 9, 4)).block()[0], 12, "{name}");
        world.assert_matches_reference();
    }
}

/// The base mod's filters: water dims light two levels a block more than
/// air, leaves one, and glass and plants none.
#[test]
fn base_light_filters() {
    let table = TestWorld::table();
    for (name, filter) in [("water", 2), ("leaves", 1), ("glass", 0), ("tuft", 0)] {
        assert_eq!(
            table.entering(Lit::from(block(name)), MAX_LIGHT),
            MAX_LIGHT - filter,
            "{name}"
        );
    }
    assert_eq!(
        table.entering(Lit::from(block("water")), 1),
        0,
        "never below 0"
    );
    // Water in with a slab dims light as water alone does.
    let slab_in_water = Lit {
        block: block("stone_slab"),
        fluid: block("water"),
        amount: 500,
    };
    assert_eq!(table.entering(slab_in_water, MAX_LIGHT), MAX_LIGHT - 2);
    // A quarter block in the slab's half doesn't fill it: the light
    // comes in whole, and leaves dimmed as it would from a full one.
    let part_full = Lit {
        amount: 250,
        ..slab_in_water
    };
    assert_eq!(table.entering(part_full, MAX_LIGHT), MAX_LIGHT);
    assert_eq!(
        table.offered(part_full, MAX_LIGHT, SKY, DOWN),
        MAX_LIGHT - 2,
        "half full, it passes on a level less"
    );
    // Nearly full, it passes on what a full one would; a film, all of it.
    let nearly = Lit {
        amount: 450,
        ..slab_in_water
    };
    assert_eq!(
        table.offered(nearly, MAX_LIGHT, SKY, DOWN),
        table.offered(slab_in_water, MAX_LIGHT - 2, SKY, DOWN),
    );
    let film = Lit {
        amount: 50,
        ..slab_in_water
    };
    assert_eq!(table.offered(film, MAX_LIGHT, SKY, DOWN), MAX_LIGHT);
    assert!(
        !table.same_light(block("water"), block("glass")),
        "light treats them differently, by their filters alone"
    );
}

/// Sky light falling straight down through a lake loses water's filter
/// as it comes into each voxel, and once it's below full strength, the
/// level per block as well: a lake darkens with depth, also where it
/// crosses a chunk border.
#[test]
fn sky_light_darkens_down_through_water() {
    let (mut world, mut queue) = (TestWorld::new(1), LightQueue::default());
    let lake = |pos: IVec3| match pos.y {
        ..=3 => block("stone"),
        4..=18 => block("water"),
        _ => BlockState::AIR,
    };
    for coord in [IVec3::ZERO, IVec3::Y] {
        world.load(&mut queue, coord, lake);
    }
    world.run(&mut queue);
    let column = IVec3::new(5, 0, 7);
    assert_eq!(world.light(column.with_y(19)).sky(), MAX_LIGHT, "above");
    for (y, expected) in [(18, 13), (17, 10), (16, 7), (15, 4), (14, 1), (13, 0)] {
        assert_eq!(world.light(column.with_y(y)).sky(), expected, "y {y}");
    }
    world.assert_matches_reference();
}

/// Water that doesn't fill its voxel leaves that voxel's light alone, so
/// what sticks out of it is lit as the air beside it, and dims everything
/// beyond as a full voxel would: the lake under it is lit the same. Filling
/// it again dims it again.
#[test]
fn part_full_water_lights_its_voxel_and_dims_beyond() {
    let (mut world, mut queue) = (TestWorld::new(1), LightQueue::default());
    let lake = |pos: IVec3| match pos.y {
        ..=3 => block("stone"),
        4..=18 => block("water"),
        _ => BlockState::AIR,
    };
    for coord in [IVec3::ZERO, IVec3::Y] {
        world.load(&mut queue, coord, lake);
    }
    world.run(&mut queue);
    let column = IVec3::new(5, 0, 7);
    let below = |world: &TestWorld| {
        (4..=17)
            .map(|y| world.light(column.with_y(y)))
            .collect::<Vec<_>>()
    };
    let full = below(&world);
    let surface = column.with_y(18);
    world.pour_some(&mut queue, surface, block("water"), 900);
    world.run(&mut queue);
    assert_eq!(world.light(surface).sky(), MAX_LIGHT, "lit as the air");
    assert_eq!(below(&world), full, "the lake under it is lit the same");
    world.assert_matches_reference();
    world.pour_some(&mut queue, surface, block("water"), FULL_FLUID);
    world.run(&mut queue);
    assert_eq!(world.light(surface).sky(), 13);
    world.assert_matches_reference();
}

/// A plant standing in a film of water on the ground is lit as one on
/// dry ground, and so is the film: two rows of plants each lit by a lamp
/// at its end, one row in a film, are lit the same. Filling and draining
/// a voxel of the film keeps the light right.
#[test]
fn a_plant_in_a_film_is_lit_as_on_dry_ground() {
    let (mut world, mut queue) = (TestWorld::new(1), LightQueue::default());
    let ground = |pos: IVec3| {
        if pos.y <= 3 {
            block("stone")
        } else {
            BlockState::AIR
        }
    };
    world.load(&mut queue, IVec3::ZERO, ground);
    world.run(&mut queue);
    let (wet, dry) = (4, 12);
    for z in [wet, dry] {
        world.set(&mut queue, IVec3::new(2, 4, z), block("lamp"));
        for x in (4..=9).step_by(2) {
            world.set(&mut queue, IVec3::new(x, 4, z), block("tuft"));
        }
    }
    for x in 4..=9 {
        world.pour_some(&mut queue, IVec3::new(x, 4, wet), block("water"), 100);
    }
    world.run(&mut queue);
    for x in 4..=9 {
        assert_eq!(
            world.light(IVec3::new(x, 4, wet)),
            world.light(IVec3::new(x, 4, dry)),
            "x {x}"
        );
    }
    world.assert_matches_reference();
    world.pour_some(
        &mut queue,
        IVec3::new(6, 4, wet),
        block("water"),
        FULL_FLUID,
    );
    world.drain(&mut queue, IVec3::new(8, 4, wet));
    world.run(&mut queue);
    world.assert_matches_reference();
}

/// Water in the top layer filters the sky coming in from outside the
/// loaded chunks too.
#[test]
fn water_filters_the_sky_from_outside() {
    let (mut world, mut queue) = (TestWorld::new(0), LightQueue::default());
    world.load(&mut queue, IVec3::ZERO, |pos| {
        if pos.y >= 14 {
            block("water")
        } else {
            BlockState::AIR
        }
    });
    world.run(&mut queue);
    let column = IVec3::new(5, 0, 7);
    assert_eq!(world.light(column.with_y(15)).sky(), 13);
    assert_eq!(world.light(column.with_y(14)).sky(), 10);
    assert_eq!(world.light(column.with_y(13)).sky(), 9, "air under it");
    world.assert_matches_reference();
}

/// A removal coming from beside a top voxel that filters the sky from
/// outside (here, light taken away by a block placed next to it) clears it,
/// and the outside sky comes back into it, not just its neighbours' light.
#[test]
fn the_outside_sky_comes_back_into_filtering_voxels() {
    let (mut world, mut queue) = (TestWorld::open(), LightQueue::default());
    world.load(&mut queue, IVec3::ZERO, |pos| {
        if pos.y == 15 && pos.x == 5 {
            block("water")
        } else {
            BlockState::AIR
        }
    });
    world.run(&mut queue);
    let water = IVec3::new(5, 15, 5);
    assert_eq!(world.light(water).sky(), 13);
    world.set(&mut queue, water + IVec3::X, block("stone"));
    world.run(&mut queue);
    assert_eq!(world.light(water).sky(), 13);
    world.assert_matches_reference();
}

/// Light going sideways through a hedge of leaves loses a level more per
/// block, in one chunk and across into the next.
#[test]
fn light_dims_sideways_through_leaves() {
    // No sky: only y = 1 and up gets it, and nothing's loaded there.
    let (mut world, mut queue) = (TestWorld::new(1), LightQueue::default());
    let hedge = |pos: IVec3| {
        if (14..=17).contains(&pos.x) {
            block("leaves")
        } else {
            BlockState::AIR
        }
    };
    for coord in [IVec3::ZERO, IVec3::X] {
        world.load(&mut queue, coord, hedge);
    }
    world.set(&mut queue, IVec3::new(12, 8, 8), block("lamp"));
    world.run(&mut queue);
    for (x, expected) in [(13, 14), (14, 12), (15, 10), (16, 8), (17, 6), (18, 5)] {
        let at = IVec3::new(x, 8, 8);
        assert_eq!(world.light(at).block()[0], expected, "x {x}");
    }
    world.assert_matches_reference();
}

/// Placing a filtering block dims what's under it, swapping it for one
/// that filters less (or glass, which doesn't) brightens it, and taking it
/// away does too.
#[test]
fn edits_add_and_remove_filtering() {
    let (mut world, mut queue) = (TestWorld::new(1), LightQueue::default());
    // A pit one voxel wide, so light reaches its floor only from above.
    let pit = |pos: IVec3| {
        let hole = pos.x == 5 && pos.z == 7 && pos.y >= 8;
        if pos.y <= 20 && !hole {
            block("stone")
        } else {
            BlockState::AIR
        }
    };
    for coord in [IVec3::ZERO, IVec3::Y] {
        world.load(&mut queue, coord, pit);
    }
    world.run(&mut queue);
    let floor = IVec3::new(5, 8, 7);
    assert_eq!(world.light(floor).sky(), MAX_LIGHT);
    let top = IVec3::new(5, 20, 7);
    world.set(&mut queue, top, block("water"));
    world.run(&mut queue);
    assert_eq!(world.light(top).sky(), 13);
    assert_eq!(world.light(floor).sky(), 1, "12 blocks down");
    world.assert_matches_reference();
    world.set(&mut queue, top, block("leaves"));
    world.run(&mut queue);
    assert_eq!(world.light(top).sky(), 14);
    assert_eq!(world.light(floor).sky(), 2);
    world.assert_matches_reference();
    world.set(&mut queue, top, block("glass"));
    world.run(&mut queue);
    assert_eq!(world.light(floor).sky(), MAX_LIGHT);
    world.assert_matches_reference();
    world.set(&mut queue, top, block("water"));
    world.run(&mut queue);
    world.set(&mut queue, top, BlockState::AIR);
    world.run(&mut queue);
    assert_eq!(world.light(floor).sky(), MAX_LIGHT);
    world.assert_matches_reference();
}

/// A side that's covered stops light across chunk borders too, and the
/// sky from outside the loaded chunks stops at a covered top.
#[test]
fn covered_sides_stop_light_across_chunk_borders() {
    let content = defaria_core::content::test_mod(&[(
        "blocks/lid.json5",
        r#"{ model: "quad", textures: { all: "a" } }"#,
    )]);
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    let table = LightTable::new(&content);
    let lid = content.default_state("test:lid").unwrap();
    // Covers its top only.
    let (lit, air) = (Lit::from(lid), Lit::from(BlockState::AIR));
    assert!(table.closed(lit, UP) && !table.closed(lit, DOWN));
    let through = |from, to, direction| table.passing(from, to, direction, SKY, 9);
    assert_eq!(through(air, lit, DOWN + 1), 9);
    assert_eq!(through(air, lit, DOWN), 0);
    assert_eq!(through(lit, air, UP), 0);

    let mut store = TestWorld::new(1);
    let mut queue = LightQueue::default();
    for coord in [IVec3::ZERO, IVec3::Y] {
        let mut chunk = Chunk::default();
        // A lid at the top of each chunk: under the outside sky at x = 0,
        // and under the chunk above's bottom layer at x = 1.
        if coord == IVec3::Y {
            chunk.set_at(IVec3::new(0, 15, 0), lid);
        } else {
            chunk.set_at(IVec3::new(1, 15, 0), lid);
        }
        queue.chunk_loaded(coord, &chunk, &table, &*store.sky, |c| {
            store.chunks.get(&c).map(|(chunk, light)| (chunk, light))
        });
        store.chunks.insert(coord, (chunk, ChunkLight::default()));
    }
    let sky = store.sky.clone();
    queue.run(&mut store, &table, &*sky);
    let sky = |pos: IVec3| store.light(pos).sky();
    // The covered top keeps the outside sky out of its voxel, but it
    // isn't a full block: light comes in at the sides.
    assert_eq!(sky(IVec3::new(0, 31, 0)), MAX_LIGHT - 1);
    // The full sky coming down the column stops at the lid over the
    // border, which is lit from the sides instead.
    assert_eq!(sky(IVec3::new(1, 16, 0)), MAX_LIGHT);
    assert_eq!(sky(IVec3::new(1, 15, 0)), MAX_LIGHT - 1);
}

/// A light source that covers a side shines its own light out through it,
/// but not light that came in from elsewhere.
#[test]
fn light_sources_shine_only_their_own_light_through_covered_sides() {
    let content = defaria_core::content::test_mod(&[
        (
            "models/cube.json5",
            r#"{ slots: { all: null }, occludes: ["up", "down", "north", "south", "east", "west"], faces: [] }"#,
        ),
        ("blocks/wall.json5", r#"{ model: "cube" }"#),
        (
            "blocks/lamp.json5",
            r#"{ model: "cube", light: [15, 0, 0] }"#,
        ),
        // Covers its top, and glows a dimmer red.
        (
            "blocks/glow.json5",
            r#"{ model: "quad", textures: { all: "a" }, light: [10, 0, 0] }"#,
        ),
    ]);
    assert!(content.errors.is_empty(), "{:?}", content.errors);
    let table = LightTable::new(&content);
    let state = |id: &str| content.default_state(&format!("test:{id}")).unwrap();
    let mut chunk = Chunk::default();
    let glow = IVec3::new(5, 5, 5);
    let above = glow + IVec3::Y;
    chunk.set_at(glow, state("glow"));
    // The lamp lights the glow block through its open west side.
    chunk.set_at(glow - IVec3::X, state("lamp"));
    // Walls round the voxel above it, so light only gets there through the
    // glow block's covered top.
    for d in [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z, IVec3::Y] {
        chunk.set_at(above + d, state("wall"));
    }
    let mut store = TestWorld::new(1);
    let mut queue = LightQueue::default();
    queue.chunk_loaded(IVec3::ZERO, &chunk, &table, &*store.sky, |_| None);
    store
        .chunks
        .insert(IVec3::ZERO, (chunk, ChunkLight::default()));
    let sky = store.sky.clone();
    queue.run(&mut store, &table, &*sky);
    assert_eq!(store.light(glow).block()[0], 14);
    // Its own 10, less one: not the lamp's 14, less one.
    assert_eq!(store.light(above).block()[0], 9);
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
        block("stone_slab"),
        block("water"),
        block("leaves"),
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
        block("stone_slab"),
        block("water"),
        block("leaves"),
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

/// A column lit by itself on the side (as loading does on a task) and then
/// joined to the world comes out the same as one lit in place: light
/// spreads both ways across its borders.
#[test]
fn chunks_lit_alone_join_the_world_right() {
    let fill = random_block(11);
    let table = TestWorld::table();
    let (mut world, mut queue) = (TestWorld::new(1), LightQueue::default());
    // Columns two chunks tall: one loaded the usual way, then two more lit
    // alone and joined beside it, the second also touching the first.
    for coord in [IVec3::ZERO, IVec3::Y] {
        world.load(&mut queue, coord, &fill);
    }
    world.run(&mut queue);
    for column in [IVec3::X, IVec3::new(0, 0, 1)] {
        let coords = [column, column + IVec3::Y];
        let chunks: Vec<Chunk> = coords
            .iter()
            .map(|&coord| {
                let mut chunk = Chunk::default();
                for index in 0..CHUNK_VOLUME {
                    let local = Chunk::position_of(index);
                    chunk.put(local, fill(coord * CHUNK_SIZE as i32 + local));
                }
                chunk
            })
            .collect();
        let pairs: Vec<(IVec3, &Chunk)> = coords.iter().copied().zip(&chunks).collect();
        let lights = light_alone(&pairs, &table, &SkyFrom(1));
        for ((coord, chunk), light) in coords.into_iter().zip(chunks.clone()).zip(lights) {
            world.chunks.insert(coord, (chunk, light));
        }
        let all = world.chunks.clone();
        for coord in coords {
            let (chunk, light) = &all[&coord];
            queue.lit_chunk_loaded(coord, chunk, light, &table, &SkyFrom(1), |c| {
                all.get(&c).map(|(chunk, light)| (chunk, light))
            });
        }
        world.run(&mut queue);
        world.assert_matches_reference();
    }
}

/// With open sky above whatever's loaded, as in the game: a chunk loading
/// on top shades the one below where it doesn't pass full sky down, and
/// unloading it lets the sky back in.
#[test]
fn the_sky_comes_in_above_whatever_is_loaded() {
    let roofed = |pos: IVec3| {
        if pos.y <= 3 || (pos.y == 20 && pos.x < 12) {
            block("stone")
        } else {
            BlockState::AIR
        }
    };
    let (mut world, mut queue) = (TestWorld::open(), LightQueue::default());
    world.load(&mut queue, IVec3::ZERO, roofed);
    world.run(&mut queue);
    let under = IVec3::new(4, 8, 4);
    assert_eq!(world.light(under).sky(), MAX_LIGHT, "nothing above yet");
    world.assert_matches_reference();

    world.load(&mut queue, IVec3::Y, roofed);
    world.run(&mut queue);
    assert_eq!(world.light(under).sky(), MAX_LIGHT - 8, "under the roof");
    assert_eq!(
        world.light(IVec3::new(14, 8, 4)).sky(),
        MAX_LIGHT,
        "beside it"
    );
    world.assert_matches_reference();

    world.unload(&mut queue, IVec3::Y);
    world.run(&mut queue);
    assert_eq!(world.light(under).sky(), MAX_LIGHT, "the roof's gone");
    world.assert_matches_reference();

    // The same with the chunk above lit by itself, as loading does.
    world.load_lit(&mut queue, IVec3::Y, roofed);
    world.run(&mut queue);
    assert_eq!(world.light(under).sky(), MAX_LIGHT - 8);
    world.assert_matches_reference();
}

/// Random worlds under open sky, with chunks loading (lit in place or by
/// themselves), unloading and being edited in random order, several at a
/// time between runs: the light always matches a fresh fill of whatever's
/// loaded.
#[test]
fn random_loads_unloads_and_edits_under_open_sky_keep_the_light_right() {
    let blocks = [
        BlockState::AIR,
        BlockState::AIR,
        block("stone"),
        block("stone"),
        block("water"),
        block("leaves"),
        block("lamp"),
        block("blue_lamp"),
    ];
    let size = IVec3::new(2, 3, 2);
    let all = coords(size);
    for seed in 1..12u64 {
        let mut rng = Rng((seed * 0x9E37_79B9) | 1);
        let fill = random_block(seed);
        let (mut world, mut queue) = (TestWorld::open(), LightQueue::default());
        for step in 0..30 {
            let coord = all[rng.below(all.len() as u64) as usize];
            match rng.below(4) {
                _ if !world.chunks.contains_key(&coord) => {
                    if rng.below(2) == 0 {
                        world.load(&mut queue, coord, &fill);
                    } else {
                        world.load_lit(&mut queue, coord, &fill);
                    }
                }
                0 => world.unload(&mut queue, coord),
                _ => {
                    let local = IVec3::new(
                        rng.below(16) as i32,
                        [0, 1, 14, 15][rng.below(4) as usize],
                        rng.below(16) as i32,
                    );
                    let state = blocks[rng.below(blocks.len() as u64) as usize];
                    world.set(&mut queue, coord * CHUNK_SIZE as i32 + local, state);
                }
            }
            if rng.below(2) == 0 {
                continue;
            }
            world.run(&mut queue);
            let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                world.assert_matches_reference()
            }));
            assert!(checked.is_ok(), "seed {seed}, step {step}");
        }
        for &coord in &all {
            if !world.chunks.contains_key(&coord) {
                world.load_lit(&mut queue, coord, &fill);
            }
        }
        world.run(&mut queue);
        let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            world.assert_matches_reference()
        }));
        assert!(checked.is_ok(), "seed {seed}, all loaded again");
    }
}

/// Light treats a voxel as its block and its fluid: lava in with a slab
/// shines as lava does, and lava in with a tuft shuts out the sky as lava
/// alone does.
#[test]
fn a_voxel_is_lit_as_its_block_and_its_fluid() {
    let (mut world, mut queue) = (TestWorld::new(0), LightQueue::default());
    world.load(&mut queue, IVec3::ZERO, |pos| {
        if pos.y <= 3 {
            block("stone")
        } else {
            BlockState::AIR
        }
    });
    world.run(&mut queue);
    let slab = IVec3::new(3, 4, 4);
    world.set(&mut queue, slab, block("stone_slab"));
    world.pour(&mut queue, slab, block("lava"));
    let tuft = IVec3::new(8, 10, 8);
    world.set(&mut queue, tuft, block("tuft"));
    world.pour(&mut queue, tuft, block("lava"));
    world.run(&mut queue);
    let lava = base().state(block("lava")).light;
    assert_eq!(world.light(slab + IVec3::Y).block()[0], lava[0] - 1);
    assert!(
        world.light(tuft - IVec3::Y).sky() < MAX_LIGHT,
        "the sky is shut out"
    );
    world.assert_matches_reference();
}

/// A lake drained a layer at a time, across a chunk's top and bottom, as
/// flow empties it: the sky light comes back down where the water was, as
/// lighting the drained world from scratch gives it, and a lamp's light
/// spreads further, undimmed by the water that's gone.
#[test]
fn draining_a_lake_relights_where_the_water_was() {
    let (mut world, mut queue) = (TestWorld::new(1), LightQueue::default());
    // Two chunks stacked: a stone floor, a lake from y 4 to y 24 across
    // the border at 16, walled in, and open sky above.
    let lake = |pos: IVec3| (2..14).contains(&pos.x) && (2..14).contains(&pos.z);
    let fill = move |pos: IVec3| {
        if pos.y < 4 || (pos.y < 25 && !lake(pos)) {
            block("stone")
        } else if pos.y < 25 {
            block("water")
        } else {
            BlockState::AIR
        }
    };
    for coord in [IVec3::ZERO, IVec3::Y] {
        world.load(&mut queue, coord, fill);
    }
    world.set(&mut queue, IVec3::new(3, 4, 3), block("lamp"));
    world.run(&mut queue);
    world.assert_matches_reference();
    for y in (10..25).rev() {
        for x in 2..14 {
            for z in 2..14 {
                world.drain(&mut queue, IVec3::new(x, y, z));
            }
        }
        world.run(&mut queue);
        world.assert_matches_reference();
    }
    assert_eq!(
        world.light(IVec3::new(8, 10, 8)).sky(),
        MAX_LIGHT,
        "the sky comes down"
    );
}

/// Amounts of water a voxel is left with, from a film to a full voxel,
/// around the steps that part-full water dims light by.
const SOME_WATER: [u16; 10] = [30, 100, 250, 400, 470, 500, 700, 900, 960, FULL_FLUID];

/// Caves of stone with lamps, leaves, slabs and tufts in them, and water
/// in the air, slabs and tufts, much of it not filling its voxel: a block
/// and how much water it holds, the same for a seed.
fn random_wet_voxel(seed: u64) -> impl Fn(IVec3) -> (BlockState, u16) {
    move |pos: IVec3| {
        let mut rng = Rng(seed
            ^ (pos.x as u64 * 73_856_093)
            ^ (pos.y as u64 * 19_349_663)
            ^ (pos.z as u64 * 83_492_791)
            | 1);
        rng.next();
        let water = SOME_WATER[rng.below(SOME_WATER.len() as u64) as usize];
        let maybe = |rng: &mut Rng| if rng.below(2) == 0 { 0 } else { water };
        match rng.below(1000) {
            0..=4 => (block("lamp"), 0),
            5..=8 => (block("blue_lamp"), 0),
            9..=60 => (block("stone_slab"), maybe(&mut rng)),
            61..=200 => (BlockState::AIR, water),
            201..=230 => (block("leaves"), 0),
            231..=260 => (block("tuft"), maybe(&mut rng)),
            n if n < 450 => (block("stone"), 0),
            _ => (BlockState::AIR, 0),
        }
    }
}

impl TestWorld {
    /// Adds a chunk filled by `fill` (world position to a block and how
    /// much water it holds, as much as fits), dark or lit by itself as
    /// loading tasks do, and queues its light. Doesn't run the fill.
    fn load_wet(
        &mut self,
        queue: &mut LightQueue,
        coord: IVec3,
        fill: &impl Fn(IVec3) -> (BlockState, u16),
        lit: bool,
    ) {
        let mut chunk = Chunk::default();
        for index in 0..CHUNK_VOLUME {
            let local = Chunk::position_of(index);
            let (state, water) = fill(coord * CHUNK_SIZE as i32 + local);
            chunk.put(local, state);
            let room = base().state(state).fluid_room;
            let fluid = defaria_core::world::VoxelFluid::of(block("water"), water.min(room));
            chunk.set_fluid_at(local, fluid, room);
        }
        let table = Self::table();
        let neighbour = |c| self.chunks.get(&c).map(|(chunk, light)| (chunk, light));
        let light = if lit {
            let light = light_alone(&[(coord, &chunk)], &table, &*self.sky)
                .pop()
                .unwrap();
            queue.lit_chunk_loaded(coord, &chunk, &light, &table, &*self.sky, neighbour);
            light
        } else {
            queue.chunk_loaded(coord, &chunk, &table, &*self.sky, neighbour);
            ChunkLight::default()
        };
        self.chunks.insert(coord, (chunk, light));
    }

    /// Sets the block at `pos`, keeping as much of its fluid as the new
    /// block has room for, as placing and breaking do.
    fn set_keeping_fluid(&mut self, queue: &mut LightQueue, pos: IVec3, state: BlockState) {
        let (coord, local) = split(pos);
        let chunk = &mut self.chunks.get_mut(&coord).unwrap().0;
        let fluid = chunk.fluid_at(local);
        chunk.put(local, state);
        let room = base().state(state).fluid_room;
        let kept = fluid.and_then(|fluid| {
            defaria_core::world::VoxelFluid::of(fluid.kind, fluid.amount.min(room))
        });
        chunk.start_fluid_at(local, kept, room);
        queue.voxel_changed(pos);
    }

    /// Leaves `amount` of water in the voxel at `pos` (none for 0), as flow
    /// does, and queues light work only if light sees the change, as the
    /// game does (see [`LightTable::same_fluid_light`]).
    fn flow(&mut self, queue: &mut LightQueue, pos: IVec3, amount: u16) {
        let (coord, local) = split(pos);
        let chunk = &mut self.chunks.get_mut(&coord).unwrap().0;
        let block = chunk.at(local);
        let room = base().state(block).fluid_room;
        let water = |chunk: &Chunk| chunk.fluid_at(local).map(|f| (f.kind, f.amount));
        let before = water(chunk);
        let fluid = defaria_core::world::VoxelFluid::of(self::block("water"), amount.min(room));
        chunk.set_fluid_at(local, fluid, room);
        if !Self::table().same_fluid_light(block, before, water(chunk)) {
            queue.voxel_changed(pos);
        }
    }
}

/// Water that doesn't fill its voxel keeps the light right however the
/// world changes around it: chunks with part-full water on their borders
/// loading dark or lit by themselves and unloading, blocks placed and
/// broken in it, and water flowing in and draining away, relit only when
/// light sees the change, as the game does. The light always matches the
/// light worked out from scratch, under open sky and under a fixed top.
#[test]
fn part_full_water_keeps_the_light_right() {
    let size = IVec3::new(2, 2, 2);
    let all = coords(size);
    let blocks = [
        BlockState::AIR,
        block("stone"),
        block("stone_slab"),
        block("tuft"),
        block("lamp"),
    ];
    for open in [true, false] {
        for seed in 1..=8u64 {
            let mut rng = Rng((seed * 0x9E37_79B9) | 1);
            let fill = random_wet_voxel(seed);
            let mut world = if open {
                TestWorld::open()
            } else {
                TestWorld::new(1)
            };
            let mut queue = LightQueue::default();
            let check = |world: &TestWorld, when: &str| {
                let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    world.assert_matches_reference()
                }));
                assert!(checked.is_ok(), "open sky {open}, seed {seed}, {when}");
            };
            for step in 0..12 {
                let coord = all[rng.below(all.len() as u64) as usize];
                if !world.chunks.contains_key(&coord) {
                    let lit = rng.below(2) == 0;
                    world.load_wet(&mut queue, coord, &fill, lit);
                } else if rng.below(6) == 0 {
                    world.unload(&mut queue, coord);
                } else {
                    for _ in 0..4 {
                        let local = IVec3::new(
                            rng.below(16) as i32,
                            [0, 1, 14, 15][rng.below(4) as usize],
                            rng.below(16) as i32,
                        );
                        let pos = coord * CHUNK_SIZE as i32 + local;
                        match rng.below(4) {
                            0 => {
                                let state = blocks[rng.below(blocks.len() as u64) as usize];
                                world.set_keeping_fluid(&mut queue, pos, state);
                            }
                            1 => world.flow(&mut queue, pos, 0),
                            _ => {
                                let water = SOME_WATER[rng.below(SOME_WATER.len() as u64) as usize];
                                world.flow(&mut queue, pos, water);
                            }
                        }
                    }
                }
                if rng.below(2) == 0 {
                    continue;
                }
                world.run(&mut queue);
                check(&world, &format!("step {step}"));
            }
            for &coord in &all {
                if !world.chunks.contains_key(&coord) {
                    world.load_wet(&mut queue, coord, &fill, true);
                }
            }
            world.run(&mut queue);
            check(&world, "all loaded again");
        }
    }
}
