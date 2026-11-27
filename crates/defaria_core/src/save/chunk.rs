//! The on-disk form of one chunk. See `docs/world-format.md`.
//!
//! Block states are saved by name (`defaria:stone`, `defaria:log[axis=x]`),
//! not by their index in [`Content::states`], since that index changes
//! whenever a mod adds, removes or reorders blocks. Each chunk has a
//! palette of the names it uses and stores every voxel as a bit-packed
//! index into it, using as few bits as the palette needs (none for a chunk
//! of one block, such as all air or all stone). That's then
//! zlib-compressed. Each voxel's fluid is saved the same way: a palette of
//! the fluids the chunk holds, after "none", an index per voxel into it,
//! and then the amounts of the fluid that isn't a full block.
//!
//! The blob starts with a format byte, so a future format can still read
//! chunks saved by this one.

use bevy::{platform::collections::HashMap, prelude::*};

use crate::{
    block_entity::{BlockEntityData, SavedPart, read_text, read_value, write_text, write_value},
    content::{BlockState, COMBINED, Content, FULL_FLUID, PART_SEPARATOR, State, StateName},
    world::{CHUNK_VOLUME, Chunk, VoxelFluid},
};

/// The chunk format this build writes.
pub const CHUNK_FORMAT: u8 = 5;
/// How hard zlib tries: 0 (store) to 10. 6 is its usual default; it's
/// nearly as small as the slowest levels on voxel data and much faster.
const COMPRESSION_LEVEL: u8 = 6;

/// Converts between block states and the names they're saved under.
#[derive(Debug, Clone)]
pub struct StateNames {
    /// By state index.
    names: Vec<String>,
    by_name: HashMap<String, BlockState>,
    /// Each block's default state, by block id, for a saved state whose
    /// properties don't exist any more.
    by_block: HashMap<String, BlockState>,
    /// Whether each state is a fluid's, by state index.
    fluids: Vec<bool>,
    /// Each state's `fluid_room`, by state index.
    rooms: Vec<u16>,
    /// Each combined state, by its two halves, the low one first.
    combinations: HashMap<(BlockState, BlockState), BlockState>,
}

impl StateNames {
    pub fn new(content: &Content) -> Self {
        let name = |state: &State| {
            StateName::new(
                &content.blocks[state.block].id,
                state.values.iter().cloned(),
            )
            .to_string()
        };
        // A combined state is named by its halves.
        let names: Vec<String> = content
            .states
            .iter()
            .map(|state| match state.combined {
                Some([low, high]) => format!(
                    "{}{PART_SEPARATOR}{}",
                    name(content.state(low)),
                    name(content.state(high))
                ),
                None => name(state),
            })
            .collect();
        let by_name = names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), BlockState(i as u16)))
            .collect();
        let by_block = content
            .blocks
            .iter()
            .filter(|block| block.id != COMBINED)
            .map(|block| (block.id.clone(), block.default_state()))
            .collect();
        Self {
            names,
            by_name,
            by_block,
            fluids: content
                .states
                .iter()
                .map(|state| state.is_fluid())
                .collect(),
            rooms: content
                .states
                .iter()
                .map(|state| state.fluid_room)
                .collect(),
            combinations: content.combinations().clone(),
        }
    }

    /// How much fluid `state` leaves room for (see `fluid_room`).
    pub fn fluid_room(&self, state: BlockState) -> u16 {
        self.rooms.get(state.0 as usize).copied().unwrap_or(0)
    }

    /// Whether `state` is a fluid's.
    pub fn is_fluid(&self, state: BlockState) -> bool {
        self.fluids.get(state.0 as usize).copied().unwrap_or(false)
    }

    /// Whether a block with this id is defined.
    pub fn has_block(&self, id: &str) -> bool {
        self.by_block.contains_key(id)
    }

    pub fn name(&self, state: BlockState) -> &str {
        self.names.get(state.0 as usize).unwrap_or(&self.names[0])
    }

    /// The id of the block of `state`.
    pub fn block_id(&self, state: BlockState) -> &str {
        StateName::id_of(self.name(state))
    }

    /// The state saved as `name`. A block that's gone (its mod was removed)
    /// comes back as air; a state whose properties changed comes back as its
    /// block's default state. Of a voxel of two halves, a half that's gone
    /// leaves the other, as do two that don't fit together any more.
    pub fn state(&self, name: &str) -> BlockState {
        if let Some(&state) = self.by_name.get(name) {
            return state;
        }
        if let Some((low, high)) = name.split_once(PART_SEPARATOR) {
            let (low, high) = (self.state(low), self.state(high));
            return self
                .combinations
                .get(&(low, high))
                .copied()
                .unwrap_or(if low.is_air() { high } else { low });
        }
        self.by_block
            .get(StateName::id_of(name))
            .copied()
            .unwrap_or(BlockState::AIR)
    }
}

/// Encodes a chunk. See the module docs.
pub fn encode(chunk: &Chunk, names: &StateNames) -> Vec<u8> {
    let mut palette: Vec<BlockState> = Vec::new();
    let mut palette_index: HashMap<BlockState, u32> = HashMap::new();
    let indices: Vec<u32> = chunk
        .voxels()
        .iter()
        .map(|&state| {
            *palette_index.entry(state).or_insert_with(|| {
                palette.push(state);
                palette.len() as u32 - 1
            })
        })
        .collect();

    let mut payload = Vec::new();
    write_varint(&mut payload, palette.len() as u64);
    for &state in &palette {
        let name = names.name(state).as_bytes();
        write_varint(&mut payload, name.len() as u64);
        payload.extend_from_slice(name);
    }
    pack(&mut payload, &indices, bits_for(palette.len()));

    // Fluid: its palette, after "none" at 0, and an index per voxel into
    // it; then the amounts that aren't full, in voxel order.
    let mut fluids: Vec<BlockState> = Vec::new();
    let mut fluid_indices = vec![0; CHUNK_VOLUME];
    let mut partial = Vec::new();
    for (index, fluid) in chunk.fluids() {
        let entry = match fluids.iter().position(|&kind| kind == fluid.kind) {
            Some(entry) => entry,
            None => {
                fluids.push(fluid.kind);
                fluids.len() - 1
            }
        };
        fluid_indices[index] = entry as u32 + 1;
        if fluid.amount < FULL_FLUID {
            partial.push((index, fluid.amount));
        }
    }
    write_varint(&mut payload, fluids.len() as u64);
    for &kind in &fluids {
        write_text(&mut payload, names.name(kind));
    }
    pack(&mut payload, &fluid_indices, bits_for(fluids.len() + 1));
    write_varint(&mut payload, partial.len() as u64);
    for (index, amount) in partial {
        write_varint(&mut payload, index as u64);
        write_varint(&mut payload, amount as u64);
    }

    // Block entities, in voxel order too.
    let mut entities: Vec<(usize, &BlockEntityData)> = chunk.block_entities().collect();
    entities.sort_unstable_by_key(|&(index, _)| index);
    write_varint(&mut payload, entities.len() as u64);
    for (index, data) in entities {
        write_varint(&mut payload, index as u64);
        write_text(&mut payload, &data.kind);
        write_varint(&mut payload, data.parts.len() as u64);
        for (key, part) in &data.parts {
            write_text(&mut payload, key);
            write_varint(&mut payload, part.version as u64);
            write_value(&mut payload, &part.value);
        }
    }

    let mut blob = vec![CHUNK_FORMAT];
    blob.extend(miniz_oxide::deflate::compress_to_vec_zlib(
        &payload,
        COMPRESSION_LEVEL,
    ));
    blob
}

/// Whether `blob` is a chunk in a newer format than this build reads.
pub fn is_newer(blob: &[u8]) -> bool {
    blob.first().is_some_and(|&format| format > CHUNK_FORMAT)
}

/// Decodes a chunk saved by [`encode`].
pub fn decode(blob: &[u8], names: &StateNames) -> Result<Chunk, String> {
    let (&format, compressed) = blob.split_first().ok_or("empty chunk")?;
    if format != CHUNK_FORMAT {
        return Err(format!(
            "chunk format {format} isn't one this build reads (it reads {CHUNK_FORMAT})"
        ));
    }
    let payload = miniz_oxide::inflate::decompress_to_vec_zlib(compressed)
        .map_err(|error| format!("chunk doesn't decompress: {error:?}"))?;
    let mut reader = Reader::new(&payload);

    let palette_len = reader.length(CHUNK_VOLUME)?;
    if palette_len == 0 {
        return Err("the chunk's palette is empty".into());
    }
    let mut palette = Vec::with_capacity(palette_len);
    for _ in 0..palette_len {
        let len = reader.length(1024)?;
        let name =
            std::str::from_utf8(reader.bytes(len)?).map_err(|_| "a block name isn't UTF-8")?;
        palette.push(names.state(name));
    }
    let bits = bits_for(palette_len);
    let packed = reader.bytes(packed_len(bits))?;
    let mut voxels = Vec::with_capacity(CHUNK_VOLUME);
    for index in unpack(packed, bits) {
        voxels.push(
            *palette
                .get(index as usize)
                .ok_or("a voxel is off the palette")?,
        );
    }
    let mut chunk = Chunk::from_voxels(voxels).expect("unpack yields a chunk's worth");

    let fluid_count = reader.length(CHUNK_VOLUME)?;
    let mut fluids = vec![None];
    for _ in 0..fluid_count {
        let name = read_text(&mut reader)?;
        // A fluid that's gone, or isn't one any more, is gone from the
        // voxels it was in.
        let kind = names.state(&name);
        fluids.push(names.is_fluid(kind).then_some(kind));
    }
    let bits = bits_for(fluids.len());
    let entries: Vec<u32> = unpack(reader.bytes(packed_len(bits))?, bits).collect();
    let mut amounts: Vec<Option<VoxelFluid>> = Vec::with_capacity(CHUNK_VOLUME);
    for &entry in &entries {
        let kind = *fluids
            .get(entry as usize)
            .ok_or("a voxel's fluid is off the palette")?;
        amounts.push(kind.map(VoxelFluid::full));
    }
    let partial = reader.length(CHUNK_VOLUME)?;
    for _ in 0..partial {
        let index = reader.length(CHUNK_VOLUME - 1)?;
        let amount = reader.length(FULL_FLUID as usize - 1)?;
        if amount == 0 {
            return Err("a voxel has no fluid in it".into());
        }
        if entries[index] == 0 {
            return Err("a voxel without fluid has an amount of it".into());
        }
        // Nothing to set for a fluid that's gone.
        if let Some(fluid) = &mut amounts[index] {
            fluid.amount = amount as u16;
        }
    }
    // A block that's a fluid now (a mod gave it `fluid`) becomes air
    // holding a full voxel of it, unless the voxel has a fluid already.
    if palette.iter().any(|&state| names.is_fluid(state)) {
        for (index, fluid) in amounts.iter_mut().enumerate() {
            let pos = Chunk::position_of(index);
            let state = chunk.at(pos);
            if names.is_fluid(state) {
                chunk.set_at(pos, BlockState::AIR);
                fluid.get_or_insert(VoxelFluid::full(state));
            }
        }
    }
    for (index, fluid) in amounts.into_iter().enumerate() {
        let Some(fluid) = fluid else {
            continue;
        };
        // No more than its block leaves room for now: a mod may have
        // given it less since it was saved.
        let pos = Chunk::position_of(index);
        let room = names.fluid_room(chunk.at(pos));
        let fluid = VoxelFluid::of(fluid.kind, fluid.amount.min(room));
        chunk.start_fluid_at(pos, fluid, room);
    }

    let entities = reader.length(CHUNK_VOLUME)?;
    for _ in 0..entities {
        let index = reader.length(CHUNK_VOLUME - 1)?;
        let kind = read_text(&mut reader)?;
        let count = reader.length(usize::MAX)?;
        let mut data = BlockEntityData {
            kind,
            parts: default(),
        };
        for _ in 0..count {
            let key = read_text(&mut reader)?;
            let version = reader.length(u32::MAX as usize)? as u32;
            let value = read_value(&mut reader)?;
            data.parts.insert(key, SavedPart { version, value });
        }
        let pos = Chunk::position_of(index);
        // Kept only while its voxel is still the block that had it: a
        // block that's gone has become air.
        if !names.has_block(&data.kind) {
            warn!("dropped a `{}` block entity: no mod defines it", data.kind);
        } else if names.block_id(chunk.at(pos)) != data.kind {
            warn!(
                "dropped a `{}` block entity: its voxel is another block",
                data.kind
            );
        } else {
            chunk.set_block_entity_at(pos, data);
        }
    }
    if !reader.is_done() {
        return Err("chunk has trailing bytes".into());
    }
    Ok(chunk)
}

/// Bits per voxel for a palette of `len` entries: 0 for one entry.
fn bits_for(len: usize) -> u32 {
    usize::BITS - (len.max(1) - 1).leading_zeros()
}

fn packed_len(bits: u32) -> usize {
    (CHUNK_VOLUME * bits as usize).div_ceil(8)
}

/// Appends `values`, `bits` each, least significant bit first.
fn pack(out: &mut Vec<u8>, values: &[u32], bits: u32) {
    if bits == 0 {
        return;
    }
    let start = out.len();
    out.resize(start + packed_len(bits), 0);
    let packed = &mut out[start..];
    let mut bit = 0usize;
    for &value in values {
        for b in 0..bits {
            if value & (1 << b) != 0 {
                packed[bit / 8] |= 1 << (bit % 8);
            }
            bit += 1;
        }
    }
}

fn unpack(packed: &[u8], bits: u32) -> impl Iterator<Item = u32> + '_ {
    (0..CHUNK_VOLUME).map(move |i| {
        let mut value = 0;
        for b in 0..bits as usize {
            let bit = i * bits as usize + b;
            value |= (((packed[bit / 8] >> (bit % 8)) & 1) as u32) << b;
        }
        value
    })
}

/// LEB128: seven bits a byte, lowest first, the top bit set on all but the
/// last.
pub fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Reads what the `write_*` functions wrote, failing on anything truncated
/// or out of range instead of panicking.
pub struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    pub fn is_done(&self) -> bool {
        self.at == self.data.len()
    }

    pub fn bytes(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(len)
            .filter(|&end| end <= self.data.len());
        let end = end.ok_or("data ends early")?;
        let bytes = &self.data[self.at..end];
        self.at = end;
        Ok(bytes)
    }

    pub fn varint(&mut self) -> Result<u64, String> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.bytes(1)?[0];
            value |= ((byte & 0x7F) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("a number is too long".into())
    }

    /// A varint no larger than `max`.
    pub fn length(&mut self, max: usize) -> Result<usize, String> {
        let value = self.varint()?;
        if value > max as u64 {
            return Err(format!("{value} is out of range (at most {max})"));
        }
        Ok(value as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{base, block, grass};

    fn names() -> StateNames {
        StateNames::new(base())
    }

    #[test]
    fn chunks_round_trip() {
        let names = names();
        let mut busy = Chunk::default();
        // Every state there is, so the palette needs several bits.
        let states = base().states.len();
        for (i, pos) in (0..CHUNK_VOLUME).map(Chunk::position_of).enumerate() {
            // A fluid in the voxel's fluid, in air.
            busy.put(pos, BlockState((i * 7 % states) as u16));
        }
        let mut sparse = Chunk::default();
        sparse.set(3, 4, 5, block("lamp"));
        // Water, some of it not full, and some of it in slabs; lava.
        let mut pond = Chunk::default();
        let water = |amount| VoxelFluid::of(block("water"), amount);
        for x in 0..16 {
            pond.fill_fluid_at(IVec3::new(x, 0, 0), block("water"));
        }
        pond.set_fluid_at(IVec3::new(2, 0, 0), water(1), FULL_FLUID);
        pond.set_fluid_at(IVec3::new(9, 0, 0), water(FULL_FLUID - 1), FULL_FLUID);
        pond.set(4, 0, 0, block("stone_slab"));
        pond.start_fluid_at(IVec3::new(4, 0, 0), water(FULL_FLUID / 2), FULL_FLUID / 2);
        pond.fill_fluid_at(IVec3::new(0, 5, 0), block("lava"));
        // A chunk that once had fluid, and has none now.
        let mut drained = Chunk::default();
        drained.fill_fluid_at(IVec3::ONE, block("water"));
        drained.set_fluid_at(IVec3::ONE, None, FULL_FLUID);
        let decoded = decode(&encode(&drained, &names), &names).unwrap();
        assert_eq!(decoded.fluids().count(), 0);
        assert_eq!(
            decoded.fluid_layer().heap_bytes(),
            0,
            "saved without the layer"
        );
        for chunk in [
            busy,
            sparse,
            pond,
            Chunk::default(),
            Chunk::filled(block("stone")),
        ] {
            assert_eq!(decode(&encode(&chunk, &names), &names).unwrap(), chunk);
        }
    }

    /// A voxel of two halves is saved by their names and comes back as
    /// the same two; a half that's gone leaves the other, and two that
    /// don't fit together any more leave the first.
    #[test]
    fn double_slabs_are_saved_as_their_halves() {
        let names = names();
        let content = base();
        let slab = |id: &str, values: &[(&str, &str)]| content.block_state(id, values).unwrap();
        let bottom = slab(
            "defaria:stone_slab",
            &[("half", "bottom"), ("rock", "defaria:granite")],
        );
        let top = slab(
            "defaria:planks_slab",
            &[("half", "top"), ("wood", "defaria:birch")],
        );
        let both = content.combine(bottom, top).unwrap();
        assert_eq!(
            names.name(both),
            "defaria:stone_slab[half=bottom,rock=defaria:granite]\
             +defaria:planks_slab[half=top,wood=defaria:birch]"
        );
        let mut chunk = Chunk::default();
        chunk.set(1, 2, 3, both);
        chunk.set(2, 2, 3, bottom);
        chunk.set(3, 2, 3, top);
        assert_eq!(decode(&encode(&chunk, &names), &names).unwrap(), chunk);

        assert_eq!(
            names.state(
                "defaria:gone[half=bottom]+defaria:planks_slab[half=top,wood=defaria:birch]"
            ),
            top
        );
        assert_eq!(
            names.state("defaria:stone_slab[half=bottom,rock=defaria:granite]+defaria:gone"),
            bottom
        );
        // Both bottoms: they don't fit, so the first stays.
        assert_eq!(
            names.state(
                "defaria:stone_slab[half=bottom,rock=defaria:granite]\
                 +defaria:planks_slab[half=bottom,wood=defaria:birch]"
            ),
            bottom
        );
        // The engine's block isn't a name of its own.
        assert_eq!(names.state(crate::content::COMBINED), BlockState::AIR);
    }

    /// A chunk of one block costs a few bytes, and terrain-like chunks
    /// compress to a small fraction of their 8 KiB in memory.
    #[test]
    fn chunks_are_small() {
        let names = names();
        for chunk in [Chunk::default(), Chunk::filled(block("stone"))] {
            let blob = encode(&chunk, &names);
            assert!(
                blob.len() < 64,
                "{} bytes for a one-block chunk",
                blob.len()
            );
        }

        let mut ground = Chunk::default();
        for pos in (0..CHUNK_VOLUME).map(Chunk::position_of) {
            let height = 6 + (pos.x + pos.z) % 3;
            let state = match pos.y {
                y if y < height - 3 => block("stone"),
                y if y < height - 1 => block("soil"),
                y if y < height => grass(),
                _ => BlockState::AIR,
            };
            ground.set_at(pos, state);
        }
        let blob = encode(&ground, &names);
        assert!(blob.len() < 400, "{} bytes for a ground chunk", blob.len());
    }

    /// States are saved by name, so they survive their index changing.
    #[test]
    fn states_are_saved_by_name() {
        let names = names();
        assert_eq!(names.name(BlockState::AIR), "air");
        assert_eq!(
            names.name(block("soil")),
            "defaria:soil[soil=defaria:dirt,turf=none]"
        );
        assert_eq!(
            names.name(block("stone")),
            "defaria:stone[ore=none,rock=defaria:stone]"
        );
        for i in 0..base().states.len() {
            let state = BlockState(i as u16);
            assert_eq!(names.state(names.name(state)), state);
        }
        let log = base().block("defaria:log").unwrap();
        let x = log.state(&[("axis", "x")]).unwrap();
        assert_eq!(names.name(x), "defaria:log[axis=x,wood=defaria:oak]");
        // Gone, and changed.
        assert_eq!(names.state("othermod:thing"), BlockState::AIR);
        assert_eq!(
            names.state("defaria:log[axis=sideways]"),
            log.default_state()
        );
    }

    #[test]
    fn bad_chunks_are_errors_not_panics() {
        let names = names();
        let mut lamp = Chunk::default();
        lamp.set(1, 1, 1, block("lamp"));
        let good = encode(&lamp, &names);
        assert!(decode(&[], &names).is_err());
        assert!(decode(&[CHUNK_FORMAT + 1], &names).is_err());
        assert!(is_newer(&[CHUNK_FORMAT + 1]));
        assert!(!is_newer(&good));
        assert!(decode(&good[..good.len() - 2], &names).is_err());
        // Valid zlib around a truncated payload.
        let mut payload = Vec::new();
        write_varint(&mut payload, 3);
        let mut blob = vec![CHUNK_FORMAT];
        blob.extend(miniz_oxide::deflate::compress_to_vec_zlib(&payload, 6));
        assert!(decode(&blob, &names).is_err());
    }

    /// A fluid amount out of range, for a voxel without fluid, or off the
    /// end, is an error; a fluid that's gone is dropped, and one over the
    /// room its block leaves now is cut down to it.
    #[test]
    fn bad_fluid_amounts_are_errors() {
        let names = names();
        let in_block = |block: &str, fluid: &str, at: usize, amounts: &[(u64, u64)]| {
            let mut payload = Vec::new();
            write_varint(&mut payload, 1);
            write_text(&mut payload, block);
            // One fluid, at voxel `at` (a bit a voxel).
            write_varint(&mut payload, 1);
            write_text(&mut payload, fluid);
            let mut bits = vec![0; CHUNK_VOLUME];
            bits[at] = 1;
            pack(&mut payload, &bits, 1);
            write_varint(&mut payload, amounts.len() as u64);
            for &(index, amount) in amounts {
                write_varint(&mut payload, index);
                write_varint(&mut payload, amount);
            }
            // No block entities.
            write_varint(&mut payload, 0);
            let mut blob = vec![CHUNK_FORMAT];
            blob.extend(miniz_oxide::deflate::compress_to_vec_zlib(&payload, 6));
            decode(&blob, &names)
        };
        let chunk = |fluid: &str, at: usize, amounts: &[(u64, u64)]| {
            in_block(names.name(BlockState::AIR), fluid, at, amounts)
        };
        let water = |amounts: &[(u64, u64)]| chunk("defaria:water", 5, amounts);
        let ok = water(&[(5, 300)]).unwrap();
        let at = Chunk::position_of(5);
        assert_eq!(ok.fluid_at(at), VoxelFluid::of(block("water"), 300));
        assert_eq!(water(&[]).unwrap().fluid_amount_at(at), FULL_FLUID);
        assert!(water(&[(5, 0)]).is_err(), "no fluid");
        assert!(water(&[(5, FULL_FLUID as u64)]).is_err(), "full isn't kept");
        assert!(water(&[(6, 300)]).is_err(), "no fluid there");
        assert!(water(&[(CHUNK_VOLUME as u64, 1)]).is_err(), "off the end");
        let gone = chunk("gone:goo", 5, &[(5, 300)]).unwrap();
        assert_eq!(gone.fluids().count(), 0, "a fluid that's gone");
        let stone = chunk("defaria:stone", 5, &[]).unwrap();
        assert_eq!(stone.fluids().count(), 0, "not a fluid");
        let slab = names.name(block("stone_slab"));
        let capped = in_block(slab, "defaria:water", 5, &[]).unwrap();
        assert_eq!(capped.fluid_amount_at(at), FULL_FLUID / 2, "a slab's half");
        let solid = in_block("defaria:stone", "defaria:water", 5, &[]).unwrap();
        assert_eq!(solid.fluids().count(), 0, "no room in stone");
        // A block that's a fluid now is air with it in.
        let lava = in_block("defaria:lava", "defaria:water", 5, &[]).unwrap();
        let other = Chunk::position_of(6);
        assert_eq!(lava.at(other), BlockState::AIR);
        assert_eq!(lava.fluid_at(other), Some(VoxelFluid::full(block("lava"))));
        assert_eq!(lava.fluid_at(at), Some(VoxelFluid::full(block("water"))));
    }

    #[test]
    fn varints_round_trip() {
        let mut out = Vec::new();
        let values = [0, 1, 127, 128, 300, u32::MAX as u64, u64::MAX];
        for v in values {
            write_varint(&mut out, v);
        }
        let mut reader = Reader::new(&out);
        for v in values {
            assert_eq!(reader.varint().unwrap(), v);
        }
        assert!(reader.is_done());
        assert_eq!(bits_for(1), 0);
        assert_eq!(bits_for(2), 1);
        assert_eq!(bits_for(5), 3);
        assert_eq!(bits_for(256), 8);
        assert_eq!(bits_for(257), 9);
    }
}
