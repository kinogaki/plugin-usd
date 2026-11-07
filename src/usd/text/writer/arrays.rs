// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Numeric values as text. Arrays, the bulk of geometry layers, format
//! straight into the output; long arrays format in parallel chunks joined in
//! order, and a layer's many mid-sized arrays (Kitchen Set's thousands of
//! small meshes) are formatted together up front, on every core, so the walk
//! copies each array's text in place.

use std::collections::HashMap;

use crate::usd::parallel;
use crate::usd::sdf::{Array, Half, LayerData, Value};
use crate::usd::text::number::{write_f32, write_f64, write_half, write_integer, write_unsigned};

/// Arrays longer than this format in parallel chunks.
const PARALLEL_ELEMENTS: usize = 32 * 1024;
/// Elements per parallel chunk.
const CHUNK_ELEMENTS: usize = 16 * 1024;
/// Arrays shorter than this format as the walk meets them.
const PREFORMAT_ELEMENTS: usize = 64;

/// One number or tuple element of a numeric array.
trait Format {
    fn format(&self, out: &mut Vec<u8>);
}

macro_rules! format_scalar {
    ($t:ty, $write:expr) => {
        impl Format for $t {
            fn format(&self, out: &mut Vec<u8>) {
                $write(out, *self)
            }
        }
    };
}
format_scalar!(f64, write_f64);
format_scalar!(f32, write_f32);
format_scalar!(Half, write_half);
format_scalar!(i32, |out, value: i32| write_integer(out, i64::from(value)));
format_scalar!(i64, write_integer);
format_scalar!(u32, |out, value: u32| write_unsigned(out, u64::from(value)));
format_scalar!(u64, write_unsigned);
format_scalar!(u8, |out, value: u8| write_unsigned(out, u64::from(value)));
format_scalar!(bool, |out: &mut Vec<u8>, value: bool| out.push(if value {
    b'1'
} else {
    b'0'
}));

/// A tuple `(a, b, c)`.
fn tuple<T: Format>(out: &mut Vec<u8>, items: &[T]) {
    out.push(b'(');
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.extend_from_slice(b", ");
        }
        item.format(out);
    }
    out.push(b')');
}

/// A quaternion: printed `(real, i, j, k)`, stored `i, j, k, real`.
fn quaternion<T: Format>(out: &mut Vec<u8>, items: &[T; 4]) {
    out.push(b'(');
    for index in 0..4 {
        if index > 0 {
            out.extend_from_slice(b", ");
        }
        items[(index + 3) % 4].format(out);
    }
    out.push(b')');
}

/// A matrix: `( (a, b), (c, d) )`.
fn matrix(out: &mut Vec<u8>, items: &[f64], rows: usize) {
    out.extend_from_slice(b"( ");
    for row in 0..rows {
        if row > 0 {
            out.extend_from_slice(b", ");
        }
        tuple(out, &items[row * rows..(row + 1) * rows]);
    }
    out.extend_from_slice(b" )");
}

/// One scalar attribute value (not a string or asset).
pub(super) fn format_element(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Bool(v) => v.format(out),
        Value::UChar(v) => v.format(out),
        Value::Int(v) => v.format(out),
        Value::UInt(v) => v.format(out),
        Value::Int64(v) => v.format(out),
        Value::UInt64(v) => v.format(out),
        Value::Half(v) => v.format(out),
        Value::Float(v) => v.format(out),
        Value::Double(v) | Value::TimeCode(v) => v.format(out),
        Value::Vec2d(v) => tuple(out, v),
        Value::Vec3d(v) => tuple(out, v),
        Value::Vec4d(v) => tuple(out, v),
        Value::Vec2f(v) => tuple(out, v),
        Value::Vec3f(v) => tuple(out, v),
        Value::Vec4f(v) => tuple(out, v),
        Value::Vec2h(v) => tuple(out, v),
        Value::Vec3h(v) => tuple(out, v),
        Value::Vec4h(v) => tuple(out, v),
        Value::Vec2i(v) => tuple(out, v),
        Value::Vec3i(v) => tuple(out, v),
        Value::Vec4i(v) => tuple(out, v),
        Value::Quatd(v) => quaternion(out, v),
        Value::Quatf(v) => quaternion(out, v),
        Value::Quath(v) => quaternion(out, v),
        Value::Matrix2d(v) => matrix(out, v, 2),
        Value::Matrix3d(v) => matrix(out, v, 3),
        Value::Matrix4d(v) => matrix(out, v, 4),
        _ => {}
    }
}

/// Elements `range` of a numeric array, ", " between them (and before the
/// first when the range does not start at 0).
fn format_range(out: &mut Vec<u8>, array: &Array, first: usize, last: usize) {
    macro_rules! each {
        ($items:expr, $format:expr) => {
            for index in first..last {
                if index > 0 {
                    out.extend_from_slice(b", ");
                }
                $format(&mut *out, &$items[index]);
            }
        };
    }
    match array {
        Array::Bool(items) => each!(items, |out: &mut Vec<u8>, v: &bool| v.format(out)),
        Array::UChar(items) => each!(items, |out: &mut Vec<u8>, v: &u8| v.format(out)),
        Array::Int(items) => each!(items, |out: &mut Vec<u8>, v: &i32| v.format(out)),
        Array::UInt(items) => each!(items, |out: &mut Vec<u8>, v: &u32| v.format(out)),
        Array::Int64(items) => each!(items, |out: &mut Vec<u8>, v: &i64| v.format(out)),
        Array::UInt64(items) => each!(items, |out: &mut Vec<u8>, v: &u64| v.format(out)),
        Array::Half(items) => each!(items, |out: &mut Vec<u8>, v: &Half| v.format(out)),
        Array::Float(items) => each!(items, |out: &mut Vec<u8>, v: &f32| v.format(out)),
        Array::Double(items) | Array::TimeCode(items) => {
            each!(items, |out: &mut Vec<u8>, v: &f64| v.format(out))
        }
        Array::Vec2d(items) => each!(items, |out: &mut Vec<u8>, v: &[f64; 2]| tuple(out, v)),
        Array::Vec3d(items) => each!(items, |out: &mut Vec<u8>, v: &[f64; 3]| tuple(out, v)),
        Array::Vec4d(items) => each!(items, |out: &mut Vec<u8>, v: &[f64; 4]| tuple(out, v)),
        Array::Vec2f(items) => each!(items, |out: &mut Vec<u8>, v: &[f32; 2]| tuple(out, v)),
        Array::Vec3f(items) => each!(items, |out: &mut Vec<u8>, v: &[f32; 3]| tuple(out, v)),
        Array::Vec4f(items) => each!(items, |out: &mut Vec<u8>, v: &[f32; 4]| tuple(out, v)),
        Array::Vec2h(items) => each!(items, |out: &mut Vec<u8>, v: &[Half; 2]| tuple(out, v)),
        Array::Vec3h(items) => each!(items, |out: &mut Vec<u8>, v: &[Half; 3]| tuple(out, v)),
        Array::Vec4h(items) => each!(items, |out: &mut Vec<u8>, v: &[Half; 4]| tuple(out, v)),
        Array::Vec2i(items) => each!(items, |out: &mut Vec<u8>, v: &[i32; 2]| tuple(out, v)),
        Array::Vec3i(items) => each!(items, |out: &mut Vec<u8>, v: &[i32; 3]| tuple(out, v)),
        Array::Vec4i(items) => each!(items, |out: &mut Vec<u8>, v: &[i32; 4]| tuple(out, v)),
        Array::Quatd(items) => each!(items, |out: &mut Vec<u8>, v: &[f64; 4]| quaternion(out, v)),
        Array::Quatf(items) => each!(items, |out: &mut Vec<u8>, v: &[f32; 4]| quaternion(out, v)),
        Array::Quath(items) => each!(items, |out: &mut Vec<u8>, v: &[Half; 4]| quaternion(out, v)),
        Array::Matrix2d(items) => each!(items, |out: &mut Vec<u8>, v: &[f64; 4]| matrix(out, v, 2)),
        Array::Matrix3d(items) => each!(items, |out: &mut Vec<u8>, v: &[f64; 9]| matrix(out, v, 3)),
        Array::Matrix4d(items) => {
            each!(items, |out: &mut Vec<u8>, v: &[f64; 16]| matrix(out, v, 4))
        }
        Array::String(_) | Array::Token(_) | Array::Asset(_) | Array::PathExpression(_) => {}
    }
}

/// A numeric array's elements without brackets, long ones in parallel.
fn format_items(out: &mut Vec<u8>, array: &Array) {
    let count = array.len();
    if count <= PARALLEL_ELEMENTS {
        format_range(out, array, 0, count);
        return;
    }
    let chunks = count.div_ceil(CHUNK_ELEMENTS);
    let texts = parallel::map(chunks, |index| {
        let first = index * CHUNK_ELEMENTS;
        let last = (first + CHUNK_ELEMENTS).min(count);
        let mut text = Vec::with_capacity((last - first) * 16);
        format_range(&mut text, array, first, last);
        text
    });
    out.reserve(texts.iter().map(Vec::len).sum());
    for text in texts {
        out.extend_from_slice(&text);
    }
}

/// `[a, b, ...]` for a numeric array.
pub(super) fn format_array(out: &mut Vec<u8>, array: &Array) {
    out.push(b'[');
    format_items(out, array);
    out.push(b']');
}

fn numeric(array: &Array) -> bool {
    !matches!(
        array,
        Array::String(_) | Array::Token(_) | Array::Asset(_) | Array::PathExpression(_)
    )
}

/// The layer's mid-sized numeric arrays, formatted ahead, by identity.
pub(super) struct Preformatted {
    texts: HashMap<usize, Vec<u8>>,
}

impl Preformatted {
    /// Format every field's numeric array of `PREFORMAT_ELEMENTS` to
    /// `PARALLEL_ELEMENTS` elements (defaults and time samples).
    pub(super) fn gather(layer: &LayerData) -> Preformatted {
        let mut candidates: Vec<&Array> = Vec::new();
        for (_, spec) in layer.iter() {
            for (_, value) in &spec.fields {
                match value {
                    Value::Array(array) => candidates.push(array),
                    Value::TimeSamples(samples) => {
                        candidates
                            .extend(samples.iter().filter_map(|(_, sample)| sample.as_array()));
                    }
                    _ => {}
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        let arrays: Vec<&Array> = candidates
            .into_iter()
            .filter(|array| {
                numeric(array)
                    && (PREFORMAT_ELEMENTS..=PARALLEL_ELEMENTS).contains(&array.len())
                    && seen.insert(array.identity())
            })
            .collect();
        if arrays.len() < 2 {
            return Preformatted {
                texts: HashMap::new(),
            };
        }
        // Batches of about `PARALLEL_ELEMENTS` elements, one per job: the
        // batches depend on the layer alone, never on the threads a run gets.
        let mut batches: Vec<Vec<&Array>> = vec![Vec::new()];
        let mut filled = 0;
        for array in arrays {
            if filled >= PARALLEL_ELEMENTS {
                batches.push(Vec::new());
                filled = 0;
            }
            filled += array.len();
            batches.last_mut().expect("a batch").push(array);
        }
        let formatted = parallel::map(batches.len(), |index| {
            batches[index]
                .iter()
                .map(|array| {
                    let mut text = Vec::with_capacity(array.len() * 12);
                    format_range(&mut text, array, 0, array.len());
                    (array.identity(), text)
                })
                .collect::<Vec<_>>()
        });
        Preformatted {
            texts: formatted.into_iter().flatten().collect(),
        }
    }

    /// The text of `array`, when formatted ahead.
    pub(super) fn find(&self, array: &Array) -> Option<&[u8]> {
        if self.texts.is_empty() {
            return None;
        }
        self.texts.get(&array.identity()).map(Vec::as_slice)
    }
}
