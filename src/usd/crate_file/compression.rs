// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The crate's compression layers: `TfFastCompression`'s LZ4 wrapper
//! (`pxr/base/tf/fastCompression.cpp`: a chunk-count byte, then one LZ4
//! block, or int32-sized chunks for inputs past LZ4's 2 GiB block limit) and
//! `Sdf_IntegerCompression` (`pxr/usd/sdf/integerCoding.cpp`): integers as
//! deltas from the previous one, the most common delta written once, a 2-bit
//! code per integer (common, small, medium, large), then the
//! variable-width deltas, all LZ4-compressed.

use super::lz4;
use crate::usd::error::{Error, Result};

/// Decompress a `TfFastCompression` buffer, appending at most `most` bytes.
pub fn decompress(input: &[u8], out: &mut Vec<u8>, most: usize) -> Result<()> {
    let Some((&chunks, mut rest)) = input.split_first() else {
        return Err(Error::invalid("an empty compressed block in a crate file"));
    };
    if chunks == 0 {
        return lz4::decompress(rest, out, most);
    }
    let start = out.len();
    for _ in 0..chunks {
        if rest.len() < 4 {
            return Err(Error::invalid(
                "a truncated compressed chunk in a crate file",
            ));
        }
        let size = i32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
        let size = usize::try_from(size)
            .map_err(|_| Error::invalid("a negative compressed chunk size"))?;
        rest = &rest[4..];
        if size > rest.len() {
            return Err(Error::invalid("a compressed chunk runs past its buffer"));
        }
        let used = out.len() - start;
        lz4::decompress(&rest[..size], out, most - used)?;
        rest = &rest[size..];
    }
    Ok(())
}

/// Compress `input` as one `TfFastCompression` chunk, appended to `out`.
pub fn compress(input: &[u8], out: &mut Vec<u8>) -> Result<()> {
    out.push(0);
    lz4::compress(input, out)
}

/// Bytes the integer encoding of `count` integers of `width` bytes may take.
pub fn encoded_bound(count: usize, width: usize) -> usize {
    if count == 0 {
        return 0;
    }
    width + (count * 2).div_ceil(8) + count * width
}

/// An integer a crate encodes: i32 or i64.
pub trait CrateInt: Copy + Default + Eq + std::hash::Hash + Ord + Send + Sync {
    /// Bytes per integer.
    const WIDTH: usize;
    /// `self - previous`, wrapping in the integer's width.
    fn delta(self, previous: Self) -> i64;
    /// `previous + delta`, wrapping.
    fn advance(previous: Self, delta: i64) -> Self;
    /// Read little-endian bytes.
    fn read(bytes: &[u8]) -> Self;
    /// Append little-endian bytes.
    fn write(self, out: &mut Vec<u8>);
}

impl CrateInt for i32 {
    const WIDTH: usize = 4;
    fn delta(self, previous: i32) -> i64 {
        i64::from(self.wrapping_sub(previous))
    }
    fn advance(previous: i32, delta: i64) -> i32 {
        previous.wrapping_add(delta as i32)
    }
    fn read(bytes: &[u8]) -> i32 {
        i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }
    fn write(self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_le_bytes());
    }
}

impl CrateInt for i64 {
    const WIDTH: usize = 8;
    fn delta(self, previous: i64) -> i64 {
        self.wrapping_sub(previous)
    }
    fn advance(previous: i64, delta: i64) -> i64 {
        previous.wrapping_add(delta)
    }
    fn read(bytes: &[u8]) -> i64 {
        let mut raw = [0u8; 8];
        raw.copy_from_slice(&bytes[..8]);
        i64::from_le_bytes(raw)
    }
    fn write(self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_le_bytes());
    }
}

fn signed_bytes(bytes: &[u8]) -> i64 {
    match bytes.len() {
        1 => i64::from(bytes[0] as i8),
        2 => i64::from(i16::from_le_bytes([bytes[0], bytes[1]])),
        4 => i64::from(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        _ => i64::read(bytes),
    }
}

/// Decode `count` integers from the integer encoding `data`.
pub fn decode_integers<T: CrateInt>(data: &[u8], count: usize) -> Result<Vec<T>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let truncated = || Error::invalid("truncated integer codes in a crate file");
    let width = T::WIDTH;
    let codes_bytes = (count * 2).div_ceil(8);
    if data.len() < width + codes_bytes {
        return Err(truncated());
    }
    let common = signed_bytes(&data[..width]);
    let codes = &data[width..width + codes_bytes];
    let mut at = width + codes_bytes;
    // Code widths: 4-byte integers use 1/2/4-byte deltas, 8-byte ones 2/4/8.
    let sizes = if width == 4 {
        [0, 1, 2, 4]
    } else {
        [0, 2, 4, 8]
    };
    let mut out = Vec::with_capacity(count);
    let mut previous = T::default();
    for index in 0..count {
        let code = (codes[index / 4] >> ((index % 4) * 2)) & 3;
        let delta = if code == 0 {
            common
        } else {
            let size = sizes[usize::from(code)];
            let bytes = data.get(at..at + size).ok_or_else(truncated)?;
            at += size;
            signed_bytes(bytes)
        };
        previous = T::advance(previous, delta);
        out.push(previous);
    }
    Ok(out)
}

/// Decompress then decode `count` integers.
pub fn decompress_integers<T: CrateInt>(input: &[u8], count: usize) -> Result<Vec<T>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    // Two bits a code are the least `count` integers take, compressed or not.
    let least = T::WIDTH as u64 + (count as u64 * 2).div_ceil(8);
    let most = lz4::most_decompressed(input.len() as u64);
    if least > most {
        return Err(Error::invalid(
            "crate integers counting more than their compressed bytes hold",
        ));
    }
    let bound = encoded_bound(count, T::WIDTH).min(most as usize);
    let mut encoded = Vec::with_capacity(bound);
    decompress(input, &mut encoded, encoded_bound(count, T::WIDTH))?;
    decode_integers(&encoded, count)
}

/// The integer encoding of `values`. The common value is the most frequent
/// delta, the larger on ties, as Sdf chooses it.
pub fn encode_integers<T: CrateInt>(values: &[T]) -> Vec<u8> {
    let count = values.len();
    if count == 0 {
        return Vec::new();
    }
    let width = T::WIDTH;
    let common = most_common_delta(values);
    let codes_bytes = (count * 2).div_ceil(8);
    let mut out = Vec::with_capacity(encoded_bound(count, width));
    if width == 4 {
        (common as i32).write(&mut out);
    } else {
        common.write(&mut out);
    }
    out.resize(width + codes_bytes, 0);
    let mut previous = T::default();
    for (index, &value) in values.iter().enumerate() {
        let delta = value.delta(previous);
        previous = value;
        let code: u8 = if delta == common {
            0
        } else if width == 4 && (-128..=127).contains(&delta) {
            out.push(delta as i8 as u8);
            1
        } else if (-32768..=32767).contains(&delta) {
            out.extend_from_slice(&(delta as i16).to_le_bytes());
            if width == 4 { 2 } else { 1 }
        } else if (-2_147_483_648..=2_147_483_647).contains(&delta) {
            out.extend_from_slice(&(delta as i32).to_le_bytes());
            if width == 4 { 3 } else { 2 }
        } else {
            out.extend_from_slice(&delta.to_le_bytes());
            3
        };
        out[width + index / 4] |= code << ((index % 4) * 2);
    }
    out
}

/// Slots in the delta count table; once half are used, new deltas are no
/// longer counted (the choice stays valid, if less compact, on noisy data).
const DELTA_SLOTS: usize = 1 << 12;

fn most_common_delta<T: CrateInt>(values: &[T]) -> i64 {
    let mut keys = [0i64; DELTA_SLOTS];
    let mut counts = [0u32; DELTA_SLOTS];
    let mut used = 0;
    let (mut common, mut common_count) = (0i64, 0u32);
    let mut previous = T::default();
    for &value in values {
        let delta = value.delta(previous);
        previous = value;
        let mut slot = ((delta as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 52) as usize;
        while counts[slot] != 0 && keys[slot] != delta {
            slot = (slot + 1) & (DELTA_SLOTS - 1);
        }
        if counts[slot] == 0 {
            if used * 2 >= DELTA_SLOTS {
                continue;
            }
            keys[slot] = delta;
            used += 1;
        }
        counts[slot] += 1;
        let seen = counts[slot];
        if seen > common_count || (seen == common_count && delta > common) {
            common = delta;
            common_count = seen;
        }
    }
    common
}

/// `values` integer-encoded and compressed (`_WriteCompressedInts` without
/// the size prefix).
pub fn compress_integers<T: CrateInt>(values: &[T]) -> Result<Vec<u8>> {
    let encoded = encode_integers(values);
    let mut out = Vec::with_capacity(lz4::bound(encoded.len()) + 1);
    compress(&encoded, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_round_trip() {
        let values: Vec<i32> = vec![
            0,
            1,
            2,
            3,
            3,
            3,
            100,
            -5,
            70_000,
            i32::MIN,
            i32::MAX,
            4,
            5,
            6,
            7,
            8,
            9,
        ];
        let packed = compress_integers(&values).unwrap();
        assert_eq!(
            decompress_integers::<i32>(&packed, values.len()).unwrap(),
            values
        );
        let wide: Vec<i64> = vec![0, 1 << 40, -(1 << 50), 7, 7, 7, 9, i64::MAX];
        let packed = compress_integers(&wide).unwrap();
        assert_eq!(
            decompress_integers::<i64>(&packed, wide.len()).unwrap(),
            wide
        );
        assert!(decompress_integers::<i32>(&packed[..packed.len() - 1], wide.len()).is_err());
    }
}
