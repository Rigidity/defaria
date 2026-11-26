//! The on-disk form of one chunk. See `docs/world-format.md`.
//!
//! Block states are saved by name (`defaria:stone`, `defaria:log[axis=x]`),
//! not by their index in [`Content::states`], since that index changes
//! whenever a mod adds, removes or reorders blocks. Each chunk has a
//! palette of the names it uses and stores every voxel as a bit-packed
//! index into it, using as few bits as the palette needs (none for a chunk
//! of one block, such as all air or all stone). That's then
//! zlib-compressed.
//!
//! The blob starts with a format byte, so a future format can still read
//! chunks saved by this one.

use bevy::{platform::collections::HashMap, prelude::*};

use crate::{
    content::{BlockState, Content, FULL_FLUID},
    voxel::{CHUNK_VOLUME, Chunk},
};

/// The chunk format this build writes.
pub const CHUNK_FORMAT: u8 = 2;
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
}

impl StateNames {
    pub fn new(content: &Content) -> Self {
        let names: Vec<String> = content
            .states
            .iter()
            .map(|state| {
                let id = &content.blocks[state.block].id;
                if state.values.is_empty() {
                    id.clone()
                } else {
                    let values: Vec<String> = state
                        .values
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect();
                    format!("{id}[{}]", values.join(","))
                }
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
            .map(|block| (block.id.clone(), block.default_state()))
            .collect();
        Self {
            names,
            by_name,
            by_block,
        }
    }

    pub fn name(&self, state: BlockState) -> &str {
        self.names.get(state.0 as usize).unwrap_or(&self.names[0])
    }

    /// The state saved as `name`. A block that's gone (its mod was removed)
    /// comes back as air; a state whose properties changed comes back as its
    /// block's default state.
    pub fn state(&self, name: &str) -> BlockState {
        if let Some(&state) = self.by_name.get(name) {
            return state;
        }
        let block = name.split_once('[').map_or(name, |(id, _)| id);
        self.by_block.get(block).copied().unwrap_or(BlockState::AIR)
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

    // Fluid that isn't full, in voxel order so the same chunk always
    // encodes the same.
    let mut partial: Vec<(usize, u16)> = chunk.partial_fluid().collect();
    partial.sort_unstable();
    write_varint(&mut payload, partial.len() as u64);
    for (index, amount) in partial {
        write_varint(&mut payload, index as u64);
        write_varint(&mut payload, amount as u64);
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

    let partial = reader.length(CHUNK_VOLUME)?;
    for _ in 0..partial {
        let index = reader.length(CHUNK_VOLUME - 1)?;
        let amount = reader.length(FULL_FLUID as usize - 1)?;
        if amount == 0 {
            return Err("a voxel has no fluid in it".into());
        }
        chunk.start_fluid_at(Chunk::position_of(index), amount as u16);
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
            busy.set_at(pos, BlockState((i * 7 % states) as u16));
        }
        let mut sparse = Chunk::default();
        sparse.set(3, 4, 5, block("lamp"));
        // Water, some of it not full.
        let mut pond = Chunk::default();
        for x in 0..16 {
            pond.set(x, 0, 0, block("water"));
        }
        pond.set_fluid_at(IVec3::new(2, 0, 0), 1);
        pond.set_fluid_at(IVec3::new(9, 0, 0), FULL_FLUID - 1);
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

    /// A fluid amount out of range, or a voxel off the end, is an error.
    #[test]
    fn bad_fluid_amounts_are_errors() {
        let names = names();
        let water = |fluid: &[(u64, u64)]| {
            let mut payload = Vec::new();
            write_varint(&mut payload, 1);
            let name = names.name(block("water")).as_bytes();
            write_varint(&mut payload, name.len() as u64);
            payload.extend_from_slice(name);
            write_varint(&mut payload, fluid.len() as u64);
            for &(index, amount) in fluid {
                write_varint(&mut payload, index);
                write_varint(&mut payload, amount);
            }
            let mut blob = vec![CHUNK_FORMAT];
            blob.extend(miniz_oxide::deflate::compress_to_vec_zlib(&payload, 6));
            decode(&blob, &names)
        };
        let ok = water(&[(5, 300)]).unwrap();
        assert_eq!(ok.partial_at(Chunk::position_of(5)), Some(300));
        assert!(water(&[(5, 0)]).is_err(), "no fluid");
        assert!(water(&[(5, FULL_FLUID as u64)]).is_err(), "full isn't kept");
        assert!(water(&[(CHUNK_VOLUME as u64, 1)]).is_err(), "off the end");
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
