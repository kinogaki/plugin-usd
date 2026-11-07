// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! LZ4 block format (the format `LZ4_compress_default` writes and
//! `LZ4_decompress_safe` reads), which crate files compress their sections
//! and arrays with.
//!
//! A block is a run of sequences: a token (literal length high nibble,
//! match length - 4 low nibble, 15 meaning "more bytes follow"), the
//! literals, a little-endian 16-bit offset back into the output and the
//! extra match length. The last sequence holds literals only; the last five
//! bytes are always literals and no match starts in the last twelve.

use crate::usd::error::{Error, Result};

const MIN_MATCH: usize = 4;
/// No match may start in the last `MF_LIMIT` bytes.
const MF_LIMIT: usize = 12;
/// The last `LAST_LITERALS` bytes are always literals.
const LAST_LITERALS: usize = 5;
const MAX_OFFSET: usize = 65_535;
const HASH_BITS: u32 = 16;

/// The largest input one block holds (`LZ4_MAX_INPUT_SIZE`).
pub const MOST_INPUT: usize = 0x7E00_0000;

/// The most bytes compressing `size` bytes can take (`LZ4_compressBound`).
pub fn bound(size: usize) -> usize {
    size + size / 255 + 16
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn hash(sequence: u32) -> usize {
    (sequence.wrapping_mul(2_654_435_761) >> (32 - HASH_BITS)) as usize
}

fn put_length(out: &mut Vec<u8>, mut rest: usize) {
    while rest >= 255 {
        out.push(255);
        rest -= 255;
    }
    out.push(rest as u8);
}

fn sequence(out: &mut Vec<u8>, literals: &[u8], offset: usize, match_length: usize) {
    let literal_nibble = literals.len().min(15);
    let match_nibble = if match_length == 0 {
        0
    } else {
        (match_length - MIN_MATCH).min(15)
    };
    out.push(((literal_nibble as u8) << 4) | match_nibble as u8);
    if literals.len() >= 15 {
        put_length(out, literals.len() - 15);
    }
    out.extend_from_slice(literals);
    if match_length == 0 {
        return;
    }
    out.extend_from_slice(&(offset as u16).to_le_bytes());
    if match_length - MIN_MATCH >= 15 {
        put_length(out, match_length - MIN_MATCH - 15);
    }
}

/// Compress `input` as one LZ4 block, appended to `out`.
pub fn compress(input: &[u8], out: &mut Vec<u8>) -> Result<()> {
    if input.len() > MOST_INPUT {
        return Err(Error::limit("a block larger than LZ4 holds"));
    }
    out.reserve(bound(input.len()));
    let length = input.len();
    let mut anchor = 0;
    if length > MF_LIMIT {
        // Positions + 1, so 0 means empty.
        let mut table = vec![0u32; 1 << HASH_BITS];
        let limit = length - MF_LIMIT;
        let match_limit = length - LAST_LITERALS;
        let mut at = 0;
        let mut misses = 0usize;
        while at < limit {
            let sequence_here = read_u32(input, at);
            let slot = hash(sequence_here);
            let candidate = table[slot] as usize;
            table[slot] = (at + 1) as u32;
            if candidate > 0 {
                let from = candidate - 1;
                if at - from <= MAX_OFFSET && read_u32(input, from) == sequence_here {
                    // Extend backwards over literals, then forwards.
                    let (mut start, mut back) = (at, from);
                    while start > anchor && back > 0 && input[start - 1] == input[back - 1] {
                        start -= 1;
                        back -= 1;
                    }
                    let mut end = at + MIN_MATCH;
                    let mut source = from + MIN_MATCH;
                    while end < match_limit && input[end] == input[source] {
                        end += 1;
                        source += 1;
                    }
                    sequence(out, &input[anchor..start], start - back, end - start);
                    anchor = end;
                    at = end;
                    misses = 0;
                    if at >= 2 && at - 2 < limit {
                        table[hash(read_u32(input, at - 2))] = (at - 1) as u32;
                    }
                    continue;
                }
            }
            // Skip faster through incompressible data.
            misses += 1;
            at += 1 + (misses >> 6);
        }
    }
    sequence(out, &input[anchor..], 0, 0);
    Ok(())
}

/// The most bytes `packed` bytes of LZ4 decompress to: a sequence of no
/// literals, its two offset bytes and `k` length bytes copies at most
/// `19 + 255·k` bytes, so no input gives more than 255 bytes a byte.
pub fn most_decompressed(packed: u64) -> u64 {
    packed.saturating_mul(255)
}

/// Decompress one LZ4 block from `input`, appending at most `most` bytes to
/// `out`. Fails on a damaged block or one that would exceed `most`.
pub fn decompress(input: &[u8], out: &mut Vec<u8>, most: usize) -> Result<()> {
    let damaged = || Error::invalid("a damaged LZ4 block");
    let start = out.len();
    let limit = start.checked_add(most).ok_or_else(damaged)?;
    let mut at = 0;
    loop {
        let token = *input.get(at).ok_or_else(damaged)?;
        at += 1;
        let mut literals = usize::from(token >> 4);
        if literals == 15 {
            loop {
                let byte = *input.get(at).ok_or_else(damaged)?;
                at += 1;
                literals = literals
                    .checked_add(usize::from(byte))
                    .ok_or_else(damaged)?;
                if byte != 255 {
                    break;
                }
            }
        }
        let end = at.checked_add(literals).ok_or_else(damaged)?;
        if end > input.len() || out.len() + literals > limit {
            return Err(damaged());
        }
        out.extend_from_slice(&input[at..end]);
        at = end;
        if at == input.len() {
            return Ok(());
        }
        if at + 2 > input.len() {
            return Err(damaged());
        }
        let offset = usize::from(u16::from_le_bytes([input[at], input[at + 1]]));
        at += 2;
        let mut length = usize::from(token & 15);
        if length == 15 {
            loop {
                let byte = *input.get(at).ok_or_else(damaged)?;
                at += 1;
                length = length.checked_add(usize::from(byte)).ok_or_else(damaged)?;
                if byte != 255 {
                    break;
                }
            }
        }
        length += MIN_MATCH;
        if offset == 0 || offset > out.len() - start || out.len() + length > limit {
            return Err(damaged());
        }
        let from = out.len() - offset;
        if offset >= length {
            out.extend_from_within(from..from + length);
        } else {
            // Overlapping copy: repeat the period.
            for index in 0..length {
                let byte = out[from + index];
                out.push(byte);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(input: &[u8]) {
        let mut packed = Vec::new();
        compress(input, &mut packed).unwrap();
        assert!(packed.len() <= bound(input.len()));
        let mut out = Vec::new();
        decompress(&packed, &mut out, input.len()).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn round_trips() {
        round_trip(b"");
        round_trip(b"a");
        round_trip(b"abcdefghijklmnop");
        round_trip(&[7u8; 100_000]);
        let mut noise = Vec::new();
        let mut state = 0x1234_5678_u32;
        for _ in 0..200_000 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            noise.push((state % 7) as u8);
        }
        round_trip(&noise);
        let pattern: Vec<u8> = (0..300_000u32)
            .flat_map(|value| (value / 3).to_le_bytes())
            .collect();
        round_trip(&pattern);
    }

    #[test]
    fn rejects_damage() {
        let mut out = Vec::new();
        assert!(decompress(&[0xF0], &mut out, 100).is_err());
        assert!(decompress(&[0x11, b'a', 9, 0], &mut out, 100).is_err());
        let mut packed = Vec::new();
        compress(&[1u8; 1000], &mut packed).unwrap();
        assert!(decompress(&packed, &mut Vec::new(), 10).is_err());
    }
}
