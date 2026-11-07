// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Arrays (`_ValueHandler::PackArray` and `_WritePossiblyCompressedArray`):
//! a u64 count, then integer arrays of 16 or more integer-compressed; half,
//! float and double arrays of 16 or more as `i` (all integral: compressed
//! int32s) or `t` (at most min(n/4, 1024) distinct values: a table and
//! compressed indexes), else uncompressed after 8-byte alignment. Token,
//! string, asset and path expression arrays hold crate indexes. An empty
//! array is its rep alone, and arrays sharing a column are written once.
//!
//! The compressible arrays are encoded and compressed up front, in
//! parallel; packing then copies their bytes in.

use std::collections::{HashMap, HashSet};

use super::CrateWriter;
use crate::usd::crate_file::compression::{CrateInt, compress_integers};
use crate::usd::crate_file::element::{CrateElement, encode};
use crate::usd::crate_file::rep::Rep;
use crate::usd::error::Result;
use crate::usd::parallel;
use crate::usd::sdf::{Array, Half, LayerData, Value, ValueKind};

/// Arrays shorter than this stay uncompressed.
const MIN_COMPRESSED: usize = 16;

/// One array's compressed form: its code (`i`, `t`, or none for integer
/// arrays), the lookup table bytes and entry count, and the compressed
/// integers.
pub(super) struct Prepared {
    code: Option<u8>,
    table: Vec<u8>,
    table_count: u32,
    packed: Vec<u8>,
}

fn compressible(array: &Array) -> bool {
    array.len() >= MIN_COMPRESSED
        && matches!(
            array,
            Array::Int(_)
                | Array::UInt(_)
                | Array::Int64(_)
                | Array::UInt64(_)
                | Array::Half(_)
                | Array::Float(_)
                | Array::Double(_)
        )
}

/// Find every compressible array (once per column) and compress them in
/// parallel; `None` entries did not pay and are written plain.
pub(super) fn prepare(layer: &LayerData) -> HashMap<usize, Prepared> {
    let mut seen = HashSet::new();
    let mut arrays: Vec<&Array> = Vec::new();
    for (_, spec) in layer.iter() {
        for (_, value) in &spec.fields {
            let samples = value
                .as_time_samples()
                .map(|samples| samples.iter().map(|(_, sample)| sample));
            let candidates = std::iter::once(value).chain(samples.into_iter().flatten());
            for candidate in candidates {
                if let Value::Array(array) = candidate
                    && compressible(array)
                    && seen.insert(array.identity())
                {
                    arrays.push(array);
                }
            }
        }
    }
    let prepared = parallel::map(arrays.len(), |index| compress_array(arrays[index]));
    arrays
        .into_iter()
        .zip(prepared)
        .filter_map(|(array, prepared)| prepared.map(|prepared| (array.identity(), prepared)))
        .collect()
}

fn integers<T: CrateInt>(values: &[T]) -> Option<Prepared> {
    let packed = compress_integers(values).ok()?;
    Some(Prepared {
        code: None,
        table: Vec::new(),
        table_count: 0,
        packed,
    })
}

/// A real number array element.
trait Real: CrateElement + PartialEq {
    fn to_f64(self) -> f64;
    /// Bits identifying the value exactly, for the lookup table.
    fn key(self) -> u64;
}

impl Real for f64 {
    fn to_f64(self) -> f64 {
        self
    }
    fn key(self) -> u64 {
        self.to_bits()
    }
}

impl Real for f32 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
    fn key(self) -> u64 {
        u64::from(self.to_bits())
    }
}

impl Real for Half {
    fn to_f64(self) -> f64 {
        self.to_f64()
    }
    fn key(self) -> u64 {
        u64::from(self.0)
    }
}

/// `i` when every value is an int32 exactly, else `t` when few distinct
/// values, else none (the array is written plain).
fn reals<T: Real>(values: &[T]) -> Option<Prepared> {
    let mut ints = Vec::with_capacity(values.len());
    let integral = values.iter().all(|value| {
        let number = value.to_f64();
        // -0 stays out of the int32 form, which would drop its sign.
        let negative_zero = number == 0.0 && number.is_sign_negative();
        let fits = !negative_zero
            && (-2_147_483_648.0..=2_147_483_647.0).contains(&number)
            && f64::from(number as i32) == number;
        if fits {
            ints.push(number as i32);
        }
        fits
    });
    if integral {
        let packed = compress_integers(&ints).ok()?;
        return Some(Prepared {
            code: Some(b'i'),
            table: Vec::new(),
            table_count: 0,
            packed,
        });
    }
    // A lookup table of at most min(count / 4, 1024) distinct values.
    let limit = (values.len() / 4).min(1024);
    let mut table: Vec<T> = Vec::new();
    let mut lookup: HashMap<u64, u32> = HashMap::new();
    let mut indexes = Vec::with_capacity(values.len());
    for value in values {
        let entry = match lookup.get(&value.key()) {
            Some(&entry) => entry,
            None => {
                if table.len() == limit {
                    return None;
                }
                let entry = table.len() as u32;
                table.push(*value);
                lookup.insert(value.key(), entry);
                entry
            }
        };
        indexes.push(entry as i32);
    }
    let mut table_bytes = Vec::new();
    encode(&table, &mut table_bytes);
    let packed = compress_integers(&indexes).ok()?;
    Some(Prepared {
        code: Some(b't'),
        table: table_bytes,
        table_count: table.len() as u32,
        packed,
    })
}

fn compress_array(array: &Array) -> Option<Prepared> {
    match array {
        Array::Int(values) => integers(values),
        Array::UInt(values) => integers(&values.iter().map(|&v| v as i32).collect::<Vec<_>>()),
        Array::Int64(values) => integers(values),
        Array::UInt64(values) => integers(&values.iter().map(|&v| v as i64).collect::<Vec<_>>()),
        Array::Half(values) => reals(values),
        Array::Float(values) => reals(values),
        Array::Double(values) => reals(values),
        _ => None,
    }
}

impl CrateWriter {
    /// An array's rep, its bytes written (or shared) now.
    pub(super) fn pack_array(&mut self, array: &Array) -> Result<Rep> {
        let kind = array.kind();
        if array.is_empty() {
            return Ok(Rep::array(kind, 0, false));
        }
        let identity = array.identity();
        if let Some(&rep) = self.arrays.get(&identity) {
            return Ok(rep);
        }
        match kind {
            ValueKind::TimeCode => self.require(9),
            ValueKind::PathExpression => self.require(10),
            _ => {}
        }
        let rep = match self.prepared.remove(&identity) {
            Some(prepared) => {
                let at = self.tell();
                self.put_u64(array.len() as u64);
                if let Some(code) = prepared.code {
                    self.out.push(code);
                }
                if prepared.code == Some(b't') {
                    self.put_u32(prepared.table_count);
                    self.out.extend_from_slice(&prepared.table);
                }
                self.put_u64(prepared.packed.len() as u64);
                self.out.extend_from_slice(&prepared.packed);
                Rep::array(kind, at, true)
            }
            None => self.write_plain(array),
        };
        self.arrays.insert(identity, rep);
        Ok(rep)
    }

    /// An uncompressed array: aligned to 8 bytes (all but integer arrays),
    /// a u64 count, the elements.
    fn write_plain(&mut self, array: &Array) -> Rep {
        let integers = matches!(
            array,
            Array::Int(_) | Array::UInt(_) | Array::Int64(_) | Array::UInt64(_)
        );
        if !integers {
            self.align(8);
        }
        let at = self.tell();
        self.put_u64(array.len() as u64);
        macro_rules! plain {
            ($items:expr) => {
                encode($items, &mut self.out)
            };
        }
        match array {
            Array::Token(items) => {
                for item in items.iter() {
                    let index = self.token(item);
                    self.put_u32(index);
                }
            }
            // Asset arrays hold string indexes (a scalar asset, a token's).
            Array::String(items) | Array::Asset(items) | Array::PathExpression(items) => {
                for item in items.iter() {
                    let index = self.string(item);
                    self.put_u32(index);
                }
            }
            Array::Bool(items) => plain!(items),
            Array::UChar(items) => plain!(items),
            Array::Int(items) => plain!(items),
            Array::UInt(items) => plain!(items),
            Array::Int64(items) => plain!(items),
            Array::UInt64(items) => plain!(items),
            Array::Half(items) => plain!(items),
            Array::Float(items) => plain!(items),
            Array::Double(items) | Array::TimeCode(items) => plain!(items),
            Array::Vec2d(items) => plain!(items),
            Array::Vec3d(items) => plain!(items),
            Array::Vec4d(items) => plain!(items),
            Array::Vec2f(items) => plain!(items),
            Array::Vec3f(items) => plain!(items),
            Array::Vec4f(items) => plain!(items),
            Array::Vec2h(items) => plain!(items),
            Array::Vec3h(items) => plain!(items),
            Array::Vec4h(items) => plain!(items),
            Array::Vec2i(items) => plain!(items),
            Array::Vec3i(items) => plain!(items),
            Array::Vec4i(items) => plain!(items),
            Array::Quatd(items) => plain!(items),
            Array::Quatf(items) => plain!(items),
            Array::Quath(items) => plain!(items),
            Array::Matrix2d(items) => plain!(items),
            Array::Matrix3d(items) => plain!(items),
            Array::Matrix4d(items) => plain!(items),
        }
        Rep::array(array.kind(), at, false)
    }
}
