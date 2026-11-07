// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Numeric arrays, the bulk of geometry layers, without the general value
//! machinery: the array's text is split at element boundaries into chunks
//! that parse straight into typed columns, in parallel when the array is
//! large. Anything unusual (comments, strings, `inf` and `nan`, a malformed
//! element) sends the array back to the general parser, which reads it the
//! same way and reports any error at its position.

use std::sync::Arc;

use super::Reader;
use crate::usd::error::Result;
use crate::usd::parallel;
use crate::usd::sdf::{Array, Half, Value, ValueKind};

/// Text per parallel chunk.
const CHUNK_BYTES: usize = 256 * 1024;

/// A scalar the fast path stores, converted as the general parser does.
trait FastScalar: Copy + Default + Send {
    /// From a literal with `.`, `e` or `E` (a double).
    fn from_real(value: f64) -> Option<Self>;
    /// From an integer literal's sign and magnitude.
    fn from_integer(negative: bool, magnitude: u64) -> Option<Self>;
}

macro_rules! float_scalar {
    ($t:ty, $convert:expr) => {
        impl FastScalar for $t {
            fn from_real(value: f64) -> Option<$t> {
                Some($convert(value))
            }
            fn from_integer(negative: bool, magnitude: u64) -> Option<$t> {
                Some($convert(if negative {
                    -(magnitude as f64)
                } else {
                    magnitude as f64
                }))
            }
        }
    };
}
float_scalar!(f64, |value: f64| value);
float_scalar!(f32, |value: f64| value as f32);
float_scalar!(Half, |value: f64| Half::from_f32(value as f32));

macro_rules! integer_scalar {
    ($($t:ty),*) => {$(
        impl FastScalar for $t {
            fn from_real(value: f64) -> Option<$t> {
                if value.is_nan() || !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&value) {
                    return None;
                }
                let whole = value as i64;
                <$t>::from_integer(whole < 0, whole.unsigned_abs())
            }
            fn from_integer(negative: bool, magnitude: u64) -> Option<$t> {
                if negative {
                    if magnitude > 1 << 63 {
                        return None;
                    }
                    <$t>::try_from((magnitude as i64).wrapping_neg()).ok()
                } else {
                    <$t>::try_from(magnitude).ok()
                }
            }
        }
    )*};
}
integer_scalar!(u8, i32, u32, i64, u64);

/// An element of a fast array: a scalar or a tuple of scalars.
trait FastElement: Copy + Send {
    /// Parse one element at `at`; the offset after it.
    fn parse(text: &[u8], at: usize, quaternion: bool) -> Option<(Self, usize)>;
}

impl<S: FastScalar> FastElement for S {
    fn parse(text: &[u8], at: usize, _: bool) -> Option<(S, usize)> {
        scalar_at(text, at)
    }
}

impl<S: FastScalar, const N: usize> FastElement for [S; N] {
    fn parse(text: &[u8], at: usize, quaternion: bool) -> Option<([S; N], usize)> {
        if text.get(at) != Some(&b'(') {
            return None;
        }
        let mut at = at + 1;
        let mut out = [S::default(); N];
        for index in 0..N {
            at = skip_blanks(text, at);
            let (value, next) = scalar_at::<S>(text, at)?;
            // Text is (real, i, j, k); memory is i, j, k, real.
            out[if quaternion { (index + 3) % 4 } else { index }] = value;
            // No line end before a comma (the grammar's ListOf).
            at = skip_inline_blanks(text, next);
            if index + 1 < N {
                if text.get(at) != Some(&b',') {
                    return None;
                }
                at += 1;
            } else {
                at = skip_blanks(text, at);
            }
        }
        if text.get(at) != Some(&b')') {
            return None;
        }
        Some((out, at + 1))
    }
}

fn skip_inline_blanks(text: &[u8], mut at: usize) -> usize {
    while matches!(text.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    at
}

fn skip_blanks(text: &[u8], mut at: usize) -> usize {
    while matches!(text.get(at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        at += 1;
    }
    at
}

/// The number at `at` as a scalar, and the offset after it; none when the
/// text is not a number the fast path takes.
fn scalar_at<S: FastScalar>(text: &[u8], origin: usize) -> Option<(S, usize)> {
    let mut at = origin;
    let negative = text.get(at) == Some(&b'-');
    if negative {
        at += 1;
    }
    let digits_start = at;
    let mut magnitude: u64 = 0;
    let mut overflow = false;
    while let Some(&byte) = text.get(at) {
        if !byte.is_ascii_digit() {
            break;
        }
        match magnitude
            .checked_mul(10)
            .and_then(|m| m.checked_add(u64::from(byte - b'0')))
        {
            Some(next) => magnitude = next,
            None => overflow = true,
        }
        at += 1;
    }
    let mut real = overflow;
    if matches!(text.get(at), Some(b'.' | b'e' | b'E')) {
        if at == digits_start && text[at] != b'.' {
            return None;
        }
        real = true;
        if text[at] == b'.' {
            at += 1;
            while text.get(at).is_some_and(u8::is_ascii_digit) {
                at += 1;
            }
        }
        if matches!(text.get(at), Some(b'e' | b'E')) {
            at += 1;
            if matches!(text.get(at), Some(b'+' | b'-')) {
                at += 1;
            }
            if !text.get(at).is_some_and(u8::is_ascii_digit) {
                return None;
            }
            while text.get(at).is_some_and(u8::is_ascii_digit) {
                at += 1;
            }
        }
    }
    if at == digits_start || !ended(text, at) {
        return None;
    }
    if real {
        // Validated digits and punctuation: valid UTF-8.
        let literal = std::str::from_utf8(&text[origin..at]).ok()?;
        let value: f64 = literal.parse().ok()?;
        return Some((S::from_real(value)?, at));
    }
    if negative && magnitude == 0 {
        // "-0" reads as the double -0.
        return Some((S::from_real(-0.0)?, at));
    }
    Some((S::from_integer(negative, magnitude)?, at))
}

/// The first `needle` in `haystack`, eight bytes at a time (SWAR): the
/// array scan that finds a numeric array's end runs over most of a geometry
/// layer's bytes.
fn find_byte(haystack: &[u8], needle: u8) -> Option<usize> {
    const LOW: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let pattern = LOW * u64::from(needle);
    let (words, rest) = haystack.as_chunks::<8>();
    for (index, word) in words.iter().enumerate() {
        let x = u64::from_le_bytes(*word) ^ pattern;
        let found = x.wrapping_sub(LOW) & !x & HIGH;
        if found != 0 {
            return Some(index * 8 + (found.trailing_zeros() / 8) as usize);
        }
    }
    rest.iter()
        .position(|&b| b == needle)
        .map(|at| words.len() * 8 + at)
}

/// Whether a number ending at `at` ends there: at the end or a delimiter.
fn ended(text: &[u8], at: usize) -> bool {
    matches!(
        text.get(at),
        None | Some(b',' | b')' | b' ' | b'\t' | b'\n' | b'\r' | b']')
    )
}

/// Parse a whole chunk of elements separated by commas.
fn parse_chunk<T: FastElement>(text: &[u8], quaternion: bool) -> Option<Vec<T>> {
    let mut items = Vec::with_capacity(text.len() / 8);
    let mut at = skip_blanks(text, 0);
    while at < text.len() {
        let (item, next) = T::parse(text, at, quaternion)?;
        items.push(item);
        at = skip_inline_blanks(text, next);
        match text.get(at) {
            Some(b',') => at = skip_blanks(text, at + 1),
            None => break,
            Some(_) => {
                at = skip_blanks(text, at);
                if at < text.len() {
                    return None;
                }
            }
        }
    }
    Some(items)
}

/// Split points of `body`: every `CHUNK_BYTES`, moved to the next element
/// start (after a comma, or at a tuple's `(`).
fn splits(body: &[u8], tuples: bool) -> Vec<usize> {
    let mut points = vec![0];
    let mut at = CHUNK_BYTES;
    while at < body.len() {
        if tuples {
            while at < body.len() && body[at] != b'(' {
                at += 1;
            }
        } else {
            while at < body.len() && body[at] != b',' {
                at += 1;
            }
            at += 1;
        }
        if at >= body.len() {
            break;
        }
        points.push(at);
        at += CHUNK_BYTES;
    }
    points.push(body.len());
    points
}

/// Parse `body` (the text between the brackets) in parallel chunks.
fn parse_body<T: FastElement>(body: &[u8], tuples: bool, quaternion: bool) -> Option<Vec<T>> {
    // Commas must not end the last chunk alone: "1, 2," is not an array.
    if body.iter().rev().find(|b| !b.is_ascii_whitespace()) == Some(&b',') {
        return None;
    }
    let points = splits(body, tuples);
    if points.len() == 2 {
        return parse_chunk(body, quaternion);
    }
    let parts = parallel::map(points.len() - 1, |index| {
        let chunk = &body[points[index]..points[index + 1]];
        // A chunk ends after its comma; drop it so the chunk ends cleanly.
        let trimmed = trim_trailing_comma(chunk);
        parse_chunk::<T>(trimmed, quaternion)
    });
    let total = parts
        .iter()
        .map(|part| part.as_ref().map_or(0, Vec::len))
        .sum();
    let mut items = Vec::with_capacity(total);
    for part in parts {
        items.extend(part?);
    }
    Some(items)
}

fn trim_trailing_comma(chunk: &[u8]) -> &[u8] {
    let mut end = chunk.len();
    while end > 0 && chunk[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end > 0 && chunk[end - 1] == b',' {
        &chunk[..end - 1]
    } else {
        chunk
    }
}

fn column<T: FastElement>(body: &[u8], tuples: bool, quaternion: bool) -> Option<Arc<Vec<T>>> {
    parse_body::<T>(body, tuples, quaternion).map(Arc::new)
}

impl Reader<'_> {
    /// The numeric array at the cursor (at its `[`), or none to use the
    /// general parser (the cursor unchanged).
    pub(super) fn fast_array(&mut self, kind: ValueKind) -> Result<Option<Value>> {
        if self.peek() != b'[' {
            return Ok(None);
        }
        let start = self.at + 1;
        // A numeric array holds no other ']': the first one ends it.
        let Some(length) = find_byte(&self.data[start..], b']') else {
            return Ok(None);
        };
        let body = &self.data[start..start + length];
        let tuples = kind.components() > 1;
        let quaternion = kind.is_quaternion();
        let array = match kind {
            ValueKind::UChar => column::<u8>(body, tuples, quaternion).map(Array::UChar),
            ValueKind::Int => column::<i32>(body, tuples, quaternion).map(Array::Int),
            ValueKind::UInt => column::<u32>(body, tuples, quaternion).map(Array::UInt),
            ValueKind::Int64 => column::<i64>(body, tuples, quaternion).map(Array::Int64),
            ValueKind::UInt64 => column::<u64>(body, tuples, quaternion).map(Array::UInt64),
            ValueKind::Half => column::<Half>(body, tuples, quaternion).map(Array::Half),
            ValueKind::Float => column::<f32>(body, tuples, quaternion).map(Array::Float),
            ValueKind::Double => column::<f64>(body, tuples, quaternion).map(Array::Double),
            ValueKind::TimeCode => column::<f64>(body, tuples, quaternion).map(Array::TimeCode),
            ValueKind::Vec2d => column(body, tuples, quaternion).map(Array::Vec2d),
            ValueKind::Vec3d => column(body, tuples, quaternion).map(Array::Vec3d),
            ValueKind::Vec4d => column(body, tuples, quaternion).map(Array::Vec4d),
            ValueKind::Vec2f => column(body, tuples, quaternion).map(Array::Vec2f),
            ValueKind::Vec3f => column(body, tuples, quaternion).map(Array::Vec3f),
            ValueKind::Vec4f => column(body, tuples, quaternion).map(Array::Vec4f),
            ValueKind::Vec2h => column(body, tuples, quaternion).map(Array::Vec2h),
            ValueKind::Vec3h => column(body, tuples, quaternion).map(Array::Vec3h),
            ValueKind::Vec4h => column(body, tuples, quaternion).map(Array::Vec4h),
            ValueKind::Vec2i => column(body, tuples, quaternion).map(Array::Vec2i),
            ValueKind::Vec3i => column(body, tuples, quaternion).map(Array::Vec3i),
            ValueKind::Vec4i => column(body, tuples, quaternion).map(Array::Vec4i),
            ValueKind::Quatd => column(body, tuples, quaternion).map(Array::Quatd),
            ValueKind::Quatf => column(body, tuples, quaternion).map(Array::Quatf),
            ValueKind::Quath => column(body, tuples, quaternion).map(Array::Quath),
            _ => None,
        };
        let Some(array) = array else { return Ok(None) };
        self.at = start + length + 1;
        Ok(Some(Value::Array(array)))
    }
}

#[cfg(test)]
mod tests {
    use super::find_byte;

    #[test]
    fn finds_bytes_in_words_and_tails() {
        let text = b"0123456789abcdef]gh]";
        assert_eq!(find_byte(text, b']'), Some(16));
        assert_eq!(find_byte(&text[17..], b']'), Some(2));
        assert_eq!(find_byte(b"\x80\xff\x5d", b']'), Some(2));
        assert_eq!(find_byte(b"no bracket here at all", b']'), None);
        for at in 0..40 {
            let mut bytes = vec![b'1'; 40];
            bytes[at] = b']';
            assert_eq!(find_byte(&bytes, b']'), Some(at));
        }
    }
}
