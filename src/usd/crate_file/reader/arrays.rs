// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Array values (`UnpackArray` and `_ReadPossiblyCompressedArray`): a count
//! (u32 before 0.7.0, u64 after; a rank before it before 0.5.0), then the
//! elements. Integer arrays of 16 or more may be integer-compressed (0.5.0);
//! half, float and double arrays may hold `i` (all integral: compressed
//! int32s) or `t` (a lookup table and compressed indexes) (0.6.0). Token,
//! string, asset and path expression arrays hold crate indexes.

use std::sync::Arc;

use super::{Crate, invalid};
use crate::usd::crate_file::compression::{CrateInt, decompress_integers};
use crate::usd::crate_file::element::{CrateElement, decode};
use crate::usd::crate_file::rep::Rep;
use crate::usd::error::Result;
use crate::usd::sdf::{Array, Half, Value, ValueKind};

/// Arrays shorter than this are never compressed.
pub const MIN_COMPRESSED: u64 = 16;

/// A real element rebuilt from an `i`-compressed int32.
trait FromInt: CrateElement {
    fn from_int(value: i32) -> Self;
}

impl FromInt for f64 {
    fn from_int(value: i32) -> f64 {
        f64::from(value)
    }
}

impl FromInt for f32 {
    fn from_int(value: i32) -> f32 {
        value as f32
    }
}

impl FromInt for Half {
    fn from_int(value: i32) -> Half {
        Half::from_f32(value as f32)
    }
}

impl Crate<'_> {
    /// The array value `rep` holds.
    pub(super) fn array(&self, rep: Rep) -> Result<Value> {
        let kind = ValueKind::from_number(rep.type_byte())
            .filter(|kind| kind.is_attribute_value())
            .ok_or_else(|| invalid("a crate array of a type that cannot form arrays"))?;
        let payload = rep.payload();
        if payload == 0 {
            return Ok(Value::Array(
                Array::empty(kind).expect("an attribute value kind"),
            ));
        }
        let mut at = payload;
        if !self.since(5, 0) {
            at += 4;
        }
        let count = if self.since(7, 0) {
            let count = self.u64_at(at)?;
            at += 8;
            count
        } else {
            let count = u64::from(self.u32_at(at)?);
            at += 4;
            count
        };
        if count > self.data.len() as u64 * 8 {
            return Err(invalid("a crate array longer than the file can hold"));
        }
        let compressed = rep.is_compressed() && count >= MIN_COMPRESSED;
        let array = match kind {
            ValueKind::Int => Array::Int(Arc::new(self.ints::<i32>(at, count, compressed)?)),
            ValueKind::UInt => Array::UInt(Arc::new(
                self.ints::<i32>(at, count, compressed)?
                    .into_iter()
                    .map(|v| v as u32)
                    .collect(),
            )),
            ValueKind::Int64 => Array::Int64(Arc::new(self.ints::<i64>(at, count, compressed)?)),
            ValueKind::UInt64 => Array::UInt64(Arc::new(
                self.ints::<i64>(at, count, compressed)?
                    .into_iter()
                    .map(|v| v as u64)
                    .collect(),
            )),
            ValueKind::Half => Array::Half(Arc::new(self.reals(at, count, compressed)?)),
            ValueKind::Float => Array::Float(Arc::new(self.reals(at, count, compressed)?)),
            ValueKind::Double => Array::Double(Arc::new(self.reals(at, count, compressed)?)),
            ValueKind::Token => {
                Array::Token(Arc::new(self.index_column(at, count, |i| self.token(i))?))
            }
            ValueKind::String => {
                Array::String(Arc::new(self.index_column(at, count, |i| self.string(i))?))
            }
            ValueKind::Asset => {
                Array::Asset(Arc::new(self.index_column(at, count, |i| self.string(i))?))
            }
            ValueKind::PathExpression => {
                Array::PathExpression(Arc::new(self.index_column(at, count, |i| self.string(i))?))
            }
            ValueKind::Bool => Array::Bool(Arc::new(self.plain(at, count)?)),
            ValueKind::UChar => Array::UChar(Arc::new(self.plain(at, count)?)),
            ValueKind::TimeCode => Array::TimeCode(Arc::new(self.plain(at, count)?)),
            ValueKind::Vec2d => Array::Vec2d(Arc::new(self.plain(at, count)?)),
            ValueKind::Vec3d => Array::Vec3d(Arc::new(self.plain(at, count)?)),
            ValueKind::Vec4d => Array::Vec4d(Arc::new(self.plain(at, count)?)),
            ValueKind::Vec2f => Array::Vec2f(Arc::new(self.plain(at, count)?)),
            ValueKind::Vec3f => Array::Vec3f(Arc::new(self.plain(at, count)?)),
            ValueKind::Vec4f => Array::Vec4f(Arc::new(self.plain(at, count)?)),
            ValueKind::Vec2h => Array::Vec2h(Arc::new(self.plain(at, count)?)),
            ValueKind::Vec3h => Array::Vec3h(Arc::new(self.plain(at, count)?)),
            ValueKind::Vec4h => Array::Vec4h(Arc::new(self.plain(at, count)?)),
            ValueKind::Vec2i => Array::Vec2i(Arc::new(self.plain(at, count)?)),
            ValueKind::Vec3i => Array::Vec3i(Arc::new(self.plain(at, count)?)),
            ValueKind::Vec4i => Array::Vec4i(Arc::new(self.plain(at, count)?)),
            ValueKind::Quatd => Array::Quatd(Arc::new(self.plain(at, count)?)),
            ValueKind::Quatf => Array::Quatf(Arc::new(self.plain(at, count)?)),
            ValueKind::Quath => Array::Quath(Arc::new(self.plain(at, count)?)),
            ValueKind::Matrix2d => Array::Matrix2d(Arc::new(self.plain(at, count)?)),
            ValueKind::Matrix3d => Array::Matrix3d(Arc::new(self.plain(at, count)?)),
            ValueKind::Matrix4d => Array::Matrix4d(Arc::new(self.plain(at, count)?)),
            _ => return Err(invalid("a crate array of a type that cannot form arrays")),
        };
        Ok(Value::Array(array))
    }

    fn plain<T: CrateElement>(&self, at: u64, count: u64) -> Result<Vec<T>> {
        let size = count
            .checked_mul(T::SIZE as u64)
            .ok_or_else(|| invalid("a crate array too large"))?;
        Ok(decode(self.bytes(at, size)?))
    }

    fn ints<T: CrateInt + CrateElement>(
        &self,
        at: u64,
        count: u64,
        compressed: bool,
    ) -> Result<Vec<T>> {
        if compressed && self.since(5, 0) {
            let size = self.u64_at(at)?;
            return decompress_integers::<T>(self.bytes(at + 8, size)?, count as usize);
        }
        self.plain(at, count)
    }

    /// A half, float or double array: plain, or `i` then compressed int32s,
    /// or `t` then a lookup table and compressed u32 indexes.
    fn reals<T: FromInt>(&self, at: u64, count: u64, compressed: bool) -> Result<Vec<T>> {
        if !(compressed && self.since(6, 0)) {
            return self.plain(at, count);
        }
        let code = self.bytes(at, 1)?[0];
        match code {
            b'i' => {
                let size = self.u64_at(at + 1)?;
                let ints = decompress_integers::<i32>(self.bytes(at + 9, size)?, count as usize)?;
                Ok(ints.into_iter().map(T::from_int).collect())
            }
            b't' => {
                let entries = u64::from(self.u32_at(at + 1)?);
                let table: Vec<T> = decode(self.bytes(at + 5, entries * T::SIZE as u64)?);
                let indexes_at = at + 5 + entries * T::SIZE as u64;
                let size = self.u64_at(indexes_at)?;
                let indexes =
                    decompress_integers::<i32>(self.bytes(indexes_at + 8, size)?, count as usize)?;
                indexes
                    .into_iter()
                    .map(|index| {
                        table
                            .get(index as u32 as usize)
                            .copied()
                            .ok_or_else(|| invalid("a crate lookup index outside its table"))
                    })
                    .collect()
            }
            _ => Err(invalid("a compressed crate array with an unknown code")),
        }
    }

    fn index_column<T>(
        &self,
        at: u64,
        count: u64,
        resolve: impl Fn(u64) -> Result<T>,
    ) -> Result<Vec<T>> {
        self.bytes(at, count * 4)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|raw| resolve(u64::from(u32::read(raw))))
            .collect()
    }
}
