//! Region files: a cube of [`REGION_SIZE`]³ chunks' saved blobs in one
//! file, so a world is a few hundred files rather than a file per chunk.
//! See `docs/world-format.md`.
//!
//! A region file is a small header, a table with one entry per chunk that's
//! been saved, and the chunks' blobs (from `chunk`) one after another.
//! Chunks are stored as they are, already compressed, so the table can be
//! read and a chunk found without touching the others.

use bevy::{platform::collections::HashMap, prelude::*};

use super::chunk::Reader;

/// Chunks along each side of a region.
pub const REGION_SIZE: i32 = 16;
/// The first bytes of every region file.
const MAGIC: &[u8; 4] = b"DFRG";
/// The region file format this build writes.
pub const REGION_FORMAT: u16 = 1;

/// The region holding a chunk, and the chunk's index within it.
pub fn split(chunk: IVec3) -> (IVec3, u16) {
    let region = chunk.div_euclid(IVec3::splat(REGION_SIZE));
    let local = chunk.rem_euclid(IVec3::splat(REGION_SIZE));
    let index = (local.y * REGION_SIZE + local.z) * REGION_SIZE + local.x;
    (region, index as u16)
}

/// The chunk at `index` in `region`: the inverse of [`split`].
#[cfg(any(test, feature = "test-support"))]
pub fn join(region: IVec3, index: u16) -> IVec3 {
    let (index, size) = (index as i32, REGION_SIZE);
    region * size + IVec3::new(index % size, index / (size * size), index / size % size)
}

/// The region's file name in the world's `regions` folder.
pub fn file_name(region: IVec3) -> String {
    format!("r.{}.{}.{}.dfr", region.x, region.y, region.z)
}

/// Writes a region file holding `chunks` (their blobs), by index.
pub fn encode(chunks: &HashMap<u16, impl AsRef<[u8]>>) -> Vec<u8> {
    let mut indices: Vec<u16> = chunks.keys().copied().collect();
    indices.sort_unstable();
    let header = 4 + 2 + 2 + 4;
    let table = indices.len() * 10;
    let total = header + table + chunks.values().map(|b| b.as_ref().len()).sum::<usize>();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&REGION_FORMAT.to_le_bytes());
    out.extend_from_slice(&(REGION_SIZE as u16).to_le_bytes());
    out.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    let mut offset = (header + table) as u32;
    for &index in &indices {
        let len = chunks[&index].as_ref().len() as u32;
        out.extend_from_slice(&index.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        offset += len;
    }
    for &index in &indices {
        out.extend_from_slice(chunks[&index].as_ref());
    }
    out
}

/// Whether `data` is a region file in a newer format than this build
/// reads: not damaged, but not this build's to read (or overwrite).
pub fn is_newer(data: &[u8]) -> bool {
    data.len() >= 6 && &data[..4] == MAGIC && u16::from_le_bytes([data[4], data[5]]) > REGION_FORMAT
}

/// Reads a region file written by [`encode`]: each saved chunk's blob, by
/// index.
pub fn decode(data: &[u8]) -> Result<HashMap<u16, Vec<u8>>, String> {
    let mut reader = Reader::new(data);
    if reader.bytes(4)? != MAGIC {
        return Err("not a region file".into());
    }
    let format = u16_at(&mut reader)?;
    if format != REGION_FORMAT {
        return Err(format!(
            "region format {format} isn't one this build reads (it reads {REGION_FORMAT})"
        ));
    }
    let size = u16_at(&mut reader)?;
    if size as i32 != REGION_SIZE {
        return Err(format!("region is {size} chunks wide, not {REGION_SIZE}"));
    }
    let count = u32_at(&mut reader)? as usize;
    let capacity = REGION_SIZE.pow(3) as usize;
    if count > capacity {
        return Err(format!("region lists {count} chunks"));
    }
    let mut chunks = HashMap::with_capacity(count);
    for _ in 0..count {
        let index = u16_at(&mut reader)?;
        let offset = u32_at(&mut reader)? as usize;
        let len = u32_at(&mut reader)? as usize;
        if index as usize >= capacity {
            return Err(format!("chunk index {index} is outside the region"));
        }
        let blob = offset
            .checked_add(len)
            .and_then(|end| data.get(offset..end))
            .ok_or("a chunk runs past the end of the file")?;
        chunks.insert(index, blob.to_vec());
    }
    Ok(chunks)
}

fn u16_at(reader: &mut Reader) -> Result<u16, String> {
    Ok(u16::from_le_bytes(reader.bytes(2)?.try_into().unwrap()))
}

fn u32_at(reader: &mut Reader) -> Result<u32, String> {
    Ok(u32::from_le_bytes(reader.bytes(4)?.try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_split_into_regions() {
        assert_eq!(split(IVec3::ZERO), (IVec3::ZERO, 0));
        assert_eq!(split(IVec3::new(15, 0, 1)), (IVec3::ZERO, 31));
        assert_eq!(split(IVec3::new(0, 1, 0)), (IVec3::ZERO, 256));
        assert_eq!(
            split(IVec3::new(16, -1, -17)),
            (IVec3::new(1, -1, -2), 15 * 256 + 15 * 16)
        );
        for chunk in [IVec3::new(-40, 7, 70), IVec3::new(5, -500, -5), IVec3::ZERO] {
            let (region, index) = split(chunk);
            assert!((index as i32) < REGION_SIZE.pow(3));
            assert_eq!(join(region, index), chunk);
        }
        assert_eq!(file_name(IVec3::new(-1, 3, 0)), "r.-1.3.0.dfr");
    }

    #[test]
    fn regions_round_trip() {
        let chunks: HashMap<u16, Vec<u8>> =
            [(0, vec![1, 2, 3]), (4095, vec![9; 300]), (77, vec![])]
                .into_iter()
                .collect();
        let file = encode(&chunks);
        assert_eq!(decode(&file).unwrap(), chunks);
        let empty: HashMap<u16, Vec<u8>> = HashMap::new();
        assert_eq!(decode(&encode(&empty)).unwrap(), empty);
    }

    #[test]
    fn bad_regions_are_errors_not_panics() {
        let chunks: HashMap<u16, Vec<u8>> = [(5, vec![1, 2, 3])].into_iter().collect();
        let file = encode(&chunks);
        assert!(decode(b"").is_err());
        assert!(decode(b"NOPE\x01\x00\x10\x00\x00\x00\x00\x00").is_err());
        assert!(decode(&file[..file.len() - 1]).is_err(), "truncated blob");
        assert!(decode(&file[..14]).is_err(), "truncated table");
        assert!(!is_newer(&file));
        let mut newer = file.clone();
        newer[4] = 2;
        assert!(decode(&newer).unwrap_err().contains("format 2"));
        assert!(is_newer(&newer));
        assert!(!is_newer(b"garbage"));
    }
}
