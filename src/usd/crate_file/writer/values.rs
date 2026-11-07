// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Packing values into ValueReps (the `_ValueHandler` Pack functions and
//! the `_Writer`'s Write overloads): what fits in 32 bits is inlined (small
//! scalars, indexes, doubles exact as floats, integers in 32 bits, vectors
//! of int8 components, int8-diagonal matrices, empty dictionaries); the rest
//! is written at the file's current end and the rep points there.

use super::CrateWriter;
use crate::usd::crate_file::element::CrateElement;
use crate::usd::crate_file::rep::Rep;
use crate::usd::error::{Error, Result};
use crate::usd::sdf::{
    Dictionary, Half, ListOp, ListPart, Path, Payload, Reference, Token, Value, ValueKind,
};

/// A scalar that may be exactly an int8.
trait Small: Copy {
    fn as_int8(self) -> Option<i8>;
}

impl Small for f64 {
    fn as_int8(self) -> Option<i8> {
        // -0.0 compares equal to 0, as in C++.
        ((-128.0..=127.0).contains(&self) && f64::from(self as i8) == self).then_some(self as i8)
    }
}

impl Small for f32 {
    fn as_int8(self) -> Option<i8> {
        f64::from(self).as_int8()
    }
}

impl Small for Half {
    fn as_int8(self) -> Option<i8> {
        self.to_f64().as_int8()
    }
}

impl Small for i32 {
    fn as_int8(self) -> Option<i8> {
        i8::try_from(self).ok()
    }
}

/// int8 components of a vector packed in 32 bits, when each is exactly one.
fn small_vector<T: Small>(items: &[T]) -> Option<u64> {
    let mut packed = 0u64;
    for (index, item) in items.iter().enumerate() {
        packed |= u64::from(item.as_int8()? as u8) << (index * 8);
    }
    Some(packed)
}

/// A matrix whose off-diagonal is zero and diagonal exact int8s, packed.
fn small_diagonal(items: &[f64], rows: usize) -> Option<u64> {
    let mut packed = 0u64;
    for row in 0..rows {
        for column in 0..rows {
            let value = items[row * rows + column];
            if row != column {
                if value != 0.0 {
                    return None;
                }
            } else {
                packed |= u64::from(value.as_int8()? as u8) << (row * 8);
            }
        }
    }
    Some(packed)
}

impl CrateWriter {
    /// A fixed-size element written at the end; its offset rep.
    fn element<T: CrateElement>(&mut self, kind: ValueKind, value: &T) -> Rep {
        let at = self.tell();
        value.write(&mut self.out);
        Rep::new(kind, at, false)
    }

    /// A vector: inlined as int8 components when it can be.
    fn vector<T: Small + CrateElement, const N: usize>(
        &mut self,
        kind: ValueKind,
        items: &[T; N],
    ) -> Rep
    where
        [T; N]: CrateElement,
    {
        match small_vector(items) {
            Some(packed) => Rep::new(kind, packed, true),
            None => self.element(kind, items),
        }
    }

    fn matrix<const N: usize>(&mut self, kind: ValueKind, items: &[f64; N], rows: usize) -> Rep
    where
        [f64; N]: CrateElement,
    {
        match small_diagonal(items, rows) {
            Some(packed) => Rep::new(kind, packed, true),
            None => self.element(kind, items),
        }
    }

    /// The ValueRep of `value`, writing its bytes when not inlined.
    pub(super) fn pack(&mut self, value: &Value) -> Result<Rep> {
        use ValueKind as K;
        Ok(match value {
            Value::Array(array) => self.pack_array(array)?,
            Value::Bool(v) => Rep::new(K::Bool, u64::from(*v), true),
            Value::UChar(v) => Rep::new(K::UChar, u64::from(*v), true),
            Value::Int(v) => Rep::new(K::Int, u64::from(*v as u32), true),
            Value::UInt(v) => Rep::new(K::UInt, u64::from(*v), true),
            Value::Half(v) => Rep::new(K::Half, u64::from(v.0), true),
            Value::Float(v) => Rep::new(K::Float, u64::from(v.to_bits()), true),
            Value::Specifier(v) => Rep::new(K::Specifier, *v as u64, true),
            Value::Permission(v) => Rep::new(K::Permission, *v as u64, true),
            Value::Variability(v) => Rep::new(K::Variability, *v as u64, true),
            Value::Token(text) => {
                let index = self.token(text);
                Rep::new(K::Token, u64::from(index), true)
            }
            Value::String(text) => {
                let index = self.string(text);
                Rep::new(K::String, u64::from(index), true)
            }
            // Assets inline a token index (GetInlinedValue(SdfAssetPath)).
            Value::Asset(text) => {
                let index = self.token(text);
                Rep::new(K::Asset, u64::from(index), true)
            }
            Value::ValueBlock => Rep::new(K::ValueBlock, 0, true),
            Value::AnimationBlock => Rep::new(K::AnimationBlock, 0, true),
            Value::Double(v) => {
                let narrow = *v as f32;
                if f64::from(narrow).to_bits() == v.to_bits() {
                    Rep::new(K::Double, u64::from(narrow.to_bits()), true)
                } else {
                    self.element(K::Double, v)
                }
            }
            Value::TimeCode(v) => {
                self.require(9);
                self.element(K::TimeCode, v)
            }
            Value::Int64(v) => match i32::try_from(*v) {
                Ok(small) => Rep::new(K::Int64, u64::from(small as u32), true),
                Err(_) => self.element(K::Int64, v),
            },
            Value::UInt64(v) => match u32::try_from(*v) {
                Ok(small) => Rep::new(K::UInt64, u64::from(small), true),
                Err(_) => self.element(K::UInt64, v),
            },
            Value::PathExpression(text) => {
                self.require(10);
                let index = self.string(text);
                let at = self.tell();
                self.put_u32(index);
                Rep::new(K::PathExpression, at, false)
            }
            // Four bytes: always inlined as its own bytes.
            Value::Vec2h(v) => Rep::new(
                K::Vec2h,
                u64::from(v[0].0) | (u64::from(v[1].0) << 16),
                true,
            ),
            Value::Vec2d(v) => self.vector(K::Vec2d, v),
            Value::Vec3d(v) => self.vector(K::Vec3d, v),
            Value::Vec4d(v) => self.vector(K::Vec4d, v),
            Value::Vec2f(v) => self.vector(K::Vec2f, v),
            Value::Vec3f(v) => self.vector(K::Vec3f, v),
            Value::Vec4f(v) => self.vector(K::Vec4f, v),
            Value::Vec3h(v) => self.vector(K::Vec3h, v),
            Value::Vec4h(v) => self.vector(K::Vec4h, v),
            Value::Vec2i(v) => self.vector(K::Vec2i, v),
            Value::Vec3i(v) => self.vector(K::Vec3i, v),
            Value::Vec4i(v) => self.vector(K::Vec4i, v),
            Value::Matrix2d(v) => self.matrix(K::Matrix2d, v, 2),
            Value::Matrix3d(v) => self.matrix(K::Matrix3d, v, 3),
            Value::Matrix4d(v) => self.matrix(K::Matrix4d, v, 4),
            Value::Quatd(v) => self.element(K::Quatd, v),
            Value::Quatf(v) => self.element(K::Quatf, v),
            Value::Quath(v) => self.element(K::Quath, v),
            Value::Dictionary(dictionary) => {
                if dictionary.is_empty() {
                    Rep::new(K::Dictionary, 0, true)
                } else {
                    let at = self.tell();
                    self.write_dictionary(dictionary)?;
                    Rep::new(K::Dictionary, at, false)
                }
            }
            Value::TokenVector(items) => {
                let indexes: Vec<u32> = items.iter().map(|item| self.token(item)).collect();
                self.index_vector(K::TokenVector, &indexes)
            }
            Value::StringVector(items) => {
                let indexes: Vec<u32> = items.iter().map(|item| self.string(item)).collect();
                self.index_vector(K::StringVector, &indexes)
            }
            Value::PathVector(items) => {
                let indexes = items
                    .iter()
                    .map(|item| self.path_value(item))
                    .collect::<Result<Vec<u32>>>()?;
                self.index_vector(K::PathVector, &indexes)
            }
            Value::DoubleVector(items) => {
                let at = self.tell();
                self.put_u64(items.len() as u64);
                for item in items.iter() {
                    self.put_f64(*item);
                }
                Rep::new(K::DoubleVector, at, false)
            }
            Value::LayerOffsetVector(items) => {
                let at = self.tell();
                self.put_u64(items.len() as u64);
                for item in items.iter() {
                    self.put_f64(item.offset);
                    self.put_f64(item.scale);
                }
                Rep::new(K::LayerOffsetVector, at, false)
            }
            Value::VariantSelections(selections) => {
                let mut indexes = Vec::with_capacity(selections.len() * 2);
                for (set, selection) in selections.iter() {
                    indexes.push(self.string(set));
                    indexes.push(self.string(selection));
                }
                let at = self.tell();
                self.put_u64(selections.len() as u64);
                for index in indexes {
                    self.put_u32(index);
                }
                Rep::new(K::VariantSelectionMap, at, false)
            }
            Value::Relocates(pairs) => {
                self.require(11);
                let mut indexes = Vec::with_capacity(pairs.len() * 2);
                for (source, target) in pairs.iter() {
                    indexes.push(self.path_value(source)?);
                    indexes.push(self.path_value(target)?);
                }
                let at = self.tell();
                self.put_u64(pairs.len() as u64);
                for index in indexes {
                    self.put_u32(index);
                }
                Rep::new(K::Relocates, at, false)
            }
            Value::TokenListOp(list) => self.list_op(K::TokenListOp, list, |w, item: &Token| {
                let index = w.token(item);
                w.put_u32(index);
                Ok(())
            })?,
            Value::StringListOp(list) => {
                self.list_op(K::StringListOp, list, |w, item: &Token| {
                    let index = w.string(item);
                    w.put_u32(index);
                    Ok(())
                })?
            }
            Value::PathListOp(list) => self.list_op(K::PathListOp, list, |w, item: &Path| {
                let index = w.path_value(item)?;
                w.put_u32(index);
                Ok(())
            })?,
            Value::IntListOp(list) => self.list_op(K::IntListOp, list, |w, item: &i32| {
                w.put_u32(*item as u32);
                Ok(())
            })?,
            Value::UIntListOp(list) => self.list_op(K::UIntListOp, list, |w, item: &u32| {
                w.put_u32(*item);
                Ok(())
            })?,
            Value::Int64ListOp(list) => self.list_op(K::Int64ListOp, list, |w, item: &i64| {
                w.put_u64(*item as u64);
                Ok(())
            })?,
            Value::UInt64ListOp(list) => self.list_op(K::UInt64ListOp, list, |w, item: &u64| {
                w.put_u64(*item);
                Ok(())
            })?,
            Value::ReferenceListOp(list) => {
                self.list_op(K::ReferenceListOp, list, |w, item: &Reference| {
                    w.reference(item)
                })?
            }
            Value::PayloadListOp(list) => {
                self.list_op(K::PayloadListOp, list, |w, item: &Payload| {
                    w.payload(item);
                    Ok(())
                })?
            }
            Value::UnregisteredListOp(list) => {
                // Held in an SdfUnregisteredValue: a nested list op value.
                let at = self.tell();
                self.put_u64(0);
                let inner = self.list_op(K::UnregisteredValueListOp, list, |w, item: &Token| {
                    w.write_nested(&Value::String(item.clone()))
                })?;
                let rep_at = self.tell();
                self.patch_u64(at, rep_at - at);
                self.put_u64(inner.0);
                Rep::new(K::UnregisteredValue, at, false)
            }
            Value::TimeSamples(samples) => self.time_samples(samples)?,
            Value::Unregistered(text) => {
                let at = self.tell();
                self.write_nested(&Value::String(text.clone()))?;
                Rep::new(K::UnregisteredValue, at, false)
            }
            Value::UnregisteredDictionary(dictionary) => {
                let at = self.tell();
                self.write_nested(&Value::Dictionary(dictionary.clone()))?;
                Rep::new(K::UnregisteredValue, at, false)
            }
            Value::Payload(payload) => {
                let at = self.tell();
                self.payload(payload);
                Rep::new(K::Payload, at, false)
            }
            Value::Path(_) => {
                return Err(Error::unsupported(
                    "a crate cannot hold a single path value (a relationship default)",
                ));
            }
        })
    }

    fn index_vector(&mut self, kind: ValueKind, indexes: &[u32]) -> Rep {
        let at = self.tell();
        self.put_u64(indexes.len() as u64);
        for index in indexes {
            self.put_u32(*index);
        }
        Rep::new(kind, at, false)
    }

    /// A nested value (`_Writer::Write(VtValue)`): an int64 forward offset,
    /// the value's own bytes, then its rep at the offset's target.
    fn write_nested(&mut self, value: &Value) -> Result<()> {
        let offset_at = self.tell();
        self.put_u64(0);
        let rep = self.pack(value)?;
        let end = self.tell();
        self.patch_u64(offset_at, end - offset_at);
        self.put_u64(rep.0);
        Ok(())
    }

    /// A dictionary (`WriteMap`): a u64 count, then (string index, nested
    /// value) pairs in key order.
    fn write_dictionary(&mut self, dictionary: &Dictionary) -> Result<()> {
        self.put_u64(dictionary.len() as u64);
        for (key, value) in dictionary {
            let index = self.string(key);
            self.put_u32(index);
            self.write_nested(value)?;
        }
        Ok(())
    }

    /// A list op: its header byte, then each non-empty part (explicit,
    /// added, prepended, appended, deleted, ordered) as a count and items.
    fn list_op<T: Clone + PartialEq>(
        &mut self,
        kind: ValueKind,
        list: &ListOp<T>,
        mut item: impl FnMut(&mut CrateWriter, &T) -> Result<()>,
    ) -> Result<Rep> {
        let mut bits = u8::from(list.explicit);
        for (flag, part) in [
            (2u8, ListPart::Explicit),
            (4, ListPart::Added),
            (8, ListPart::Deleted),
            (16, ListPart::Ordered),
            (32, ListPart::Prepended),
            (64, ListPart::Appended),
        ] {
            if !list.items(part).is_empty() {
                bits |= flag;
            }
        }
        let at = self.tell();
        self.out.push(bits);
        for part in [
            ListPart::Explicit,
            ListPart::Added,
            ListPart::Prepended,
            ListPart::Appended,
            ListPart::Deleted,
            ListPart::Ordered,
        ] {
            let items = list.items(part);
            if items.is_empty() {
                continue;
            }
            self.put_u64(items.len() as u64);
            for entry in items {
                item(self, entry)?;
            }
        }
        Ok(Rep::new(kind, at, false))
    }

    fn reference(&mut self, item: &Reference) -> Result<()> {
        let asset = self.string(&item.asset);
        let prim = self.path_value(&item.prim)?;
        self.put_u32(asset);
        self.put_u32(prim);
        self.put_f64(item.offset.offset);
        self.put_f64(item.offset.scale);
        if item.custom_data.is_empty() {
            self.put_u64(0);
            return Ok(());
        }
        self.write_dictionary(&item.custom_data)
    }

    fn payload(&mut self, item: &Payload) {
        let asset = self.string(&item.asset);
        // Payload prim paths are prim paths or empty; never fails.
        let prim = self.path_value(&item.prim).unwrap_or(0);
        self.put_u32(asset);
        self.put_u32(prim);
        self.put_f64(item.offset.offset);
        self.put_f64(item.offset.scale);
    }

    /// Time samples: an offset to the times' rep (the times written before
    /// it), an offset to the values (their bytes before them), then the
    /// count and the values' reps.
    fn time_samples(&mut self, samples: &[(f64, Value)]) -> Result<Rep> {
        let at = self.tell();
        self.put_u64(0);
        let times_at = self.tell();
        self.put_u64(samples.len() as u64);
        for (time, _) in samples {
            self.put_f64(*time);
        }
        let times_rep = Rep::new(ValueKind::DoubleVector, times_at, false);
        let times_rep_at = self.tell();
        self.patch_u64(at, times_rep_at - at);
        self.put_u64(times_rep.0);
        let values_offset_at = self.tell();
        self.put_u64(0);
        let reps = samples
            .iter()
            .map(|(_, value)| self.pack(value))
            .collect::<Result<Vec<Rep>>>()?;
        let values_at = self.tell();
        self.patch_u64(values_offset_at, values_at - values_offset_at);
        self.put_u64(reps.len() as u64);
        for rep in reps {
            self.put_u64(rep.0);
        }
        Ok(Rep::new(ValueKind::TimeSamples, at, false))
    }
}
