// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! ValueReps into values (`CrateFile::_UnpackValue` and `_Reader::Read`):
//! always-inlined values in the payload (4-byte scalars, token, string and
//! asset indexes, enums), values inlined when they fit (doubles exact as
//! floats, integers in 32 bits, vectors of int8 components, matrices with an
//! int8 diagonal, empty dictionaries), and everything else at the payload's
//! file offset: dictionaries, list ops, time samples, vectors, references,
//! payloads, relocates and nested values.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use super::{Crate, invalid};
use crate::usd::crate_file::element::CrateElement;
use crate::usd::crate_file::rep::Rep;
use crate::usd::error::{Error, Result};
use crate::usd::sdf::{
    Dictionary, Half, LayerOffset, ListOp, Path, Payload, Permission, Reference, Specifier, Token,
    Value, ValueKind, Variability,
};

/// Values nest (dictionaries, nested values, time samples) at most this deep.
const MOST_DEPTH: usize = 64;

/// A value kind's int8-packed inline form.
fn small<T: Copy + Default, const N: usize>(payload: u64, convert: impl Fn(i8) -> T) -> [T; N] {
    let mut out = [T::default(); N];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = convert((payload >> (index * 8)) as u8 as i8);
    }
    out
}

fn diagonal<const N: usize>(payload: u64, rows: usize) -> [f64; N] {
    let mut out = [0.0; N];
    for row in 0..rows {
        out[row * rows + row] = f64::from((payload >> (row * 8)) as u8 as i8);
    }
    out
}

impl Crate<'_> {
    /// The value `rep` holds (arrays included); one held at a file offset
    /// is decoded once, and shared by every rep naming it.
    pub(super) fn value(&self, rep: Rep, depth: usize) -> Result<Value> {
        if rep.is_inlined() || (rep.is_array() && rep.payload() == 0) {
            return self.unpacked(rep, depth);
        }
        let held = |decoded: &Mutex<HashMap<u64, Value>>| {
            let decoded = decoded.lock().unwrap_or_else(|e| e.into_inner());
            decoded.get(&rep.0).cloned()
        };
        if let Some(value) = held(&self.decoded) {
            return Ok(value);
        }
        let value = self.unpacked(rep, depth)?;
        let mut decoded = self.decoded.lock().unwrap_or_else(|e| e.into_inner());
        decoded.insert(rep.0, value.clone());
        Ok(value)
    }

    /// The value `rep` holds, decoded from the file.
    fn unpacked(&self, rep: Rep, depth: usize) -> Result<Value> {
        if rep.is_array_edit() {
            return Err(Error::unsupported("array edit values are not supported"));
        }
        if rep.is_array() {
            return self.array(rep);
        }
        let kind = ValueKind::from_number(rep.type_byte()).ok_or_else(|| {
            Error::unsupported(format!("a crate value of unknown type {}", rep.type_byte()))
        })?;
        let payload = rep.payload();
        let bits = rep.inline_bits();
        let inlined = rep.is_inlined();
        Ok(match kind {
            ValueKind::Bool => Value::Bool(bits & 0xFF != 0),
            ValueKind::UChar => Value::UChar(bits as u8),
            ValueKind::Int => Value::Int(bits as i32),
            ValueKind::UInt => Value::UInt(bits),
            ValueKind::Half => Value::Half(Half(bits as u16)),
            ValueKind::Float => Value::Float(f32::from_bits(bits)),
            ValueKind::Int64 if inlined => Value::Int64(i64::from(bits as i32)),
            ValueKind::Int64 => Value::Int64(self.i64_at(payload)?),
            ValueKind::UInt64 if inlined => Value::UInt64(u64::from(bits)),
            ValueKind::UInt64 => Value::UInt64(self.u64_at(payload)?),
            ValueKind::Double if inlined => Value::Double(f64::from(f32::from_bits(bits))),
            ValueKind::Double => Value::Double(self.f64_at(payload)?),
            ValueKind::TimeCode if inlined => Value::TimeCode(f64::from(f32::from_bits(bits))),
            ValueKind::TimeCode => Value::TimeCode(self.f64_at(payload)?),
            ValueKind::String => Value::String(self.string(u64::from(bits))?),
            ValueKind::Token => Value::Token(self.token(u64::from(bits))?),
            ValueKind::Asset => Value::Asset(self.token(u64::from(bits))?),
            ValueKind::PathExpression if inlined => {
                Value::PathExpression(self.string(u64::from(bits))?)
            }
            ValueKind::PathExpression => {
                Value::PathExpression(self.string(u64::from(self.u32_at(payload)?))?)
            }
            ValueKind::Specifier => Value::Specifier(match bits {
                0 => Specifier::Def,
                1 => Specifier::Over,
                2 => Specifier::Class,
                _ => return Err(invalid("a crate specifier out of range")),
            }),
            ValueKind::Permission => Value::Permission(if bits == 1 {
                Permission::Private
            } else {
                Permission::Public
            }),
            // The legacy "config" variability (2) reads as uniform.
            ValueKind::Variability => Value::Variability(if bits == 0 {
                Variability::Varying
            } else {
                Variability::Uniform
            }),
            ValueKind::ValueBlock => Value::ValueBlock,
            ValueKind::AnimationBlock => Value::AnimationBlock,
            ValueKind::Vec2h if inlined => {
                Value::Vec2h([Half(bits as u16), Half((bits >> 16) as u16)])
            }
            ValueKind::Vec2d if inlined => Value::Vec2d(small(payload, f64::from)),
            ValueKind::Vec3d if inlined => Value::Vec3d(small(payload, f64::from)),
            ValueKind::Vec4d if inlined => Value::Vec4d(small(payload, f64::from)),
            ValueKind::Vec2f if inlined => Value::Vec2f(small(payload, f32::from)),
            ValueKind::Vec3f if inlined => Value::Vec3f(small(payload, f32::from)),
            ValueKind::Vec4f if inlined => Value::Vec4f(small(payload, f32::from)),
            ValueKind::Vec3h if inlined => {
                Value::Vec3h(small(payload, |v| Half::from_f32(f32::from(v))))
            }
            ValueKind::Vec4h if inlined => {
                Value::Vec4h(small(payload, |v| Half::from_f32(f32::from(v))))
            }
            ValueKind::Vec2i if inlined => Value::Vec2i(small(payload, i32::from)),
            ValueKind::Vec3i if inlined => Value::Vec3i(small(payload, i32::from)),
            ValueKind::Vec4i if inlined => Value::Vec4i(small(payload, i32::from)),
            ValueKind::Matrix2d if inlined => Value::Matrix2d(diagonal(payload, 2)),
            ValueKind::Matrix3d if inlined => Value::Matrix3d(diagonal(payload, 3)),
            ValueKind::Matrix4d if inlined => Value::Matrix4d(diagonal(payload, 4)),
            ValueKind::Vec2d => Value::Vec2d(self.element(payload)?),
            ValueKind::Vec3d => Value::Vec3d(self.element(payload)?),
            ValueKind::Vec4d => Value::Vec4d(self.element(payload)?),
            ValueKind::Vec2f => Value::Vec2f(self.element(payload)?),
            ValueKind::Vec3f => Value::Vec3f(self.element(payload)?),
            ValueKind::Vec4f => Value::Vec4f(self.element(payload)?),
            ValueKind::Vec2h => Value::Vec2h(self.element(payload)?),
            ValueKind::Vec3h => Value::Vec3h(self.element(payload)?),
            ValueKind::Vec4h => Value::Vec4h(self.element(payload)?),
            ValueKind::Vec2i => Value::Vec2i(self.element(payload)?),
            ValueKind::Vec3i => Value::Vec3i(self.element(payload)?),
            ValueKind::Vec4i => Value::Vec4i(self.element(payload)?),
            ValueKind::Quatd => Value::Quatd(self.element(payload)?),
            ValueKind::Quatf => Value::Quatf(self.element(payload)?),
            ValueKind::Quath => Value::Quath(self.element(payload)?),
            ValueKind::Matrix2d => Value::Matrix2d(self.element(payload)?),
            ValueKind::Matrix3d => Value::Matrix3d(self.element(payload)?),
            ValueKind::Matrix4d => Value::Matrix4d(self.element(payload)?),
            ValueKind::Dictionary if inlined => Value::Dictionary(Arc::new(Dictionary::new())),
            ValueKind::Dictionary => {
                Value::Dictionary(Arc::new(self.dictionary_at(&mut { payload }, depth)?))
            }
            ValueKind::TokenVector => {
                Value::TokenVector(Arc::new(self.indexes(payload, |i| self.token(i))?))
            }
            ValueKind::StringVector => {
                Value::StringVector(Arc::new(self.indexes(payload, |i| self.string(i))?))
            }
            ValueKind::PathVector => {
                Value::PathVector(Arc::new(self.indexes(payload, |i| self.path(i))?))
            }
            ValueKind::DoubleVector => {
                let count = self.count_at(payload, 8)?;
                let bytes = self.bytes(payload + 8, count * 8)?;
                Value::DoubleVector(Arc::new(
                    bytes
                        .as_chunks::<8>()
                        .0
                        .iter()
                        .map(|raw| f64::read(raw))
                        .collect(),
                ))
            }
            ValueKind::LayerOffsetVector => {
                let count = self.count_at(payload, 16)?;
                let bytes = self.bytes(payload + 8, count * 16)?;
                Value::LayerOffsetVector(Arc::new(
                    bytes
                        .as_chunks::<16>()
                        .0
                        .iter()
                        .map(|pair| LayerOffset {
                            offset: f64::read(&pair[..8]),
                            scale: f64::read(&pair[8..]),
                        })
                        .collect(),
                ))
            }
            ValueKind::VariantSelectionMap => {
                let pairs = self.indexes_pairs(payload, |i| self.string(i))?;
                Value::VariantSelections(Arc::new(pairs.into_iter().collect::<BTreeMap<_, _>>()))
            }
            ValueKind::Relocates => {
                Value::Relocates(Arc::new(self.indexes_pairs(payload, |i| self.path(i))?))
            }
            ValueKind::TokenListOp => {
                Value::TokenListOp(Arc::new(self.list_op(payload, |at| self.token_item(at))?))
            }
            ValueKind::StringListOp => {
                Value::StringListOp(Arc::new(self.list_op(payload, |at| self.string_item(at))?))
            }
            ValueKind::PathListOp => {
                Value::PathListOp(Arc::new(self.list_op(payload, |at| self.path_item(at))?))
            }
            ValueKind::IntListOp => Value::IntListOp(Arc::new(self.list_op(payload, |at| {
                let value = self.u32_at(*at)? as i32;
                *at += 4;
                Ok(value)
            })?)),
            ValueKind::UIntListOp => Value::UIntListOp(Arc::new(self.list_op(payload, |at| {
                let value = self.u32_at(*at)?;
                *at += 4;
                Ok(value)
            })?)),
            ValueKind::Int64ListOp => {
                Value::Int64ListOp(Arc::new(self.list_op(payload, |at| {
                    let value = self.i64_at(*at)?;
                    *at += 8;
                    Ok(value)
                })?))
            }
            ValueKind::UInt64ListOp => {
                Value::UInt64ListOp(Arc::new(self.list_op(payload, |at| {
                    let value = self.u64_at(*at)?;
                    *at += 8;
                    Ok(value)
                })?))
            }
            ValueKind::ReferenceListOp => Value::ReferenceListOp(Arc::new(
                self.list_op(payload, |at| self.reference_item(at, depth))?,
            )),
            ValueKind::PayloadListOp => {
                Value::PayloadListOp(Arc::new(self.list_op(payload, |at| self.payload_item(at))?))
            }
            ValueKind::UnregisteredValueListOp => {
                Value::UnregisteredListOp(Arc::new(self.list_op(payload, |at| {
                    match self.nested(at, depth)? {
                        Value::String(text) => Ok(text),
                        _ => Err(invalid(
                            "an unregistered crate list item that is not a string",
                        )),
                    }
                })?))
            }
            ValueKind::Payload => {
                // A single payload (files before list-op payloads): an
                // explicit list op of one; an empty payload is none.
                let item = self.payload_item(&mut { payload })?;
                let items = if item.asset.is_empty() && item.prim.is_empty() {
                    Vec::new()
                } else {
                    vec![item]
                };
                Value::PayloadListOp(Arc::new(ListOp::explicit(items)))
            }
            ValueKind::TimeSamples => {
                Value::TimeSamples(Arc::new(self.time_samples_at(payload, depth)?))
            }
            ValueKind::Value => self.nested(&mut { payload }, depth)?,
            ValueKind::UnregisteredValue => match self.nested(&mut { payload }, depth)? {
                Value::String(text) => Value::Unregistered(text),
                Value::Dictionary(dictionary) => Value::UnregisteredDictionary(dictionary),
                list @ Value::UnregisteredListOp(_) => list,
                _ => return Err(invalid("an unregistered crate value of an unexpected type")),
            },
            ValueKind::Spline => {
                return Err(Error::unsupported("attribute splines are not supported"));
            }
        })
    }

    fn element<T: CrateElement>(&self, at: u64) -> Result<T> {
        Ok(T::read(self.bytes(at, T::SIZE as u64)?))
    }

    /// A u64 count, then u32 indexes resolved with `resolve`.
    fn indexes<T>(&self, at: u64, resolve: impl Fn(u64) -> Result<T>) -> Result<Vec<T>> {
        let count = self.count_at(at, 4)?;
        self.bytes(at + 8, count * 4)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|raw| resolve(u64::from(u32::read(raw))))
            .collect()
    }

    /// A u64 count, then pairs of u32 indexes.
    fn indexes_pairs<T>(&self, at: u64, resolve: impl Fn(u64) -> Result<T>) -> Result<Vec<(T, T)>> {
        let count = self.count_at(at, 8)?;
        self.bytes(at + 8, count * 8)?
            .as_chunks::<8>()
            .0
            .iter()
            .map(|raw| {
                Ok((
                    resolve(u64::from(u32::read(&raw[..4])))?,
                    resolve(u64::from(u32::read(&raw[4..])))?,
                ))
            })
            .collect()
    }

    /// A dictionary at `*at`: a u64 count, then (string index, nested
    /// value) pairs; `*at` moves past it.
    fn dictionary_at(&self, at: &mut u64, depth: usize) -> Result<Dictionary> {
        if depth >= MOST_DEPTH {
            return Err(invalid("crate values nested too deeply"));
        }
        let count = self.count_at(*at, 12)?;
        *at += 8;
        let mut dictionary = Dictionary::new();
        for _ in 0..count {
            let key = self.string(u64::from(self.u32_at(*at)?))?;
            *at += 4;
            let value = self.nested(at, depth + 1)?;
            dictionary.insert(key, value);
        }
        Ok(dictionary)
    }

    /// A nested value at `*at`: an int64 offset (relative to `*at`) to its
    /// ValueRep; `*at` moves past the rep, as the reader's stream does.
    fn nested(&self, at: &mut u64, depth: usize) -> Result<Value> {
        if depth >= MOST_DEPTH {
            return Err(invalid("crate values nested too deeply"));
        }
        let offset = self.i64_at(*at)?;
        let rep_at = (*at as i64)
            .checked_add(offset)
            .filter(|at| *at >= 0)
            .ok_or_else(|| invalid("a crate value offset outside the file"))?
            as u64;
        let rep = Rep(self.u64_at(rep_at)?);
        *at = rep_at + 8;
        self.value(rep, depth + 1)
    }

    /// A list op: a header byte (explicit, then which parts follow), then
    /// each part as a u64 count and its items.
    fn list_op<T: Clone + PartialEq>(
        &self,
        at: u64,
        mut item: impl FnMut(&mut u64) -> Result<T>,
    ) -> Result<ListOp<T>> {
        let bits = self.bytes(at, 1)?[0];
        let mut list = ListOp {
            explicit: bits & 1 != 0,
            ..ListOp::default()
        };
        let mut cursor = at + 1;
        // Header bits per part, in the file's part order: explicit, added,
        // prepended, appended, deleted, ordered.
        for (flag, part) in [
            (2u8, crate::usd::sdf::ListPart::Explicit),
            (4, crate::usd::sdf::ListPart::Added),
            (32, crate::usd::sdf::ListPart::Prepended),
            (64, crate::usd::sdf::ListPart::Appended),
            (8, crate::usd::sdf::ListPart::Deleted),
            (16, crate::usd::sdf::ListPart::Ordered),
        ] {
            if bits & flag == 0 {
                continue;
            }
            let count = self.count_at(cursor, 4)?;
            cursor += 8;
            let mut items = Vec::with_capacity(count as usize);
            for _ in 0..count {
                items.push(item(&mut cursor)?);
            }
            *list.items_mut(part) = items;
        }
        Ok(list)
    }

    fn token_item(&self, at: &mut u64) -> Result<Token> {
        let value = self.token(u64::from(self.u32_at(*at)?))?;
        *at += 4;
        Ok(value)
    }

    fn string_item(&self, at: &mut u64) -> Result<Token> {
        let value = self.string(u64::from(self.u32_at(*at)?))?;
        *at += 4;
        Ok(value)
    }

    fn path_item(&self, at: &mut u64) -> Result<Path> {
        let value = self.path(u64::from(self.u32_at(*at)?))?;
        *at += 4;
        Ok(value)
    }

    fn reference_item(&self, at: &mut u64, depth: usize) -> Result<Reference> {
        let asset = self.string(u64::from(self.u32_at(*at)?))?;
        let prim = self.path(u64::from(self.u32_at(*at + 4)?))?;
        let offset = LayerOffset {
            offset: self.f64_at(*at + 8)?,
            scale: self.f64_at(*at + 16)?,
        };
        *at += 24;
        let custom_data = self.dictionary_at(at, depth)?;
        Ok(Reference {
            asset,
            prim,
            offset,
            custom_data,
        })
    }

    /// A payload: asset string, prim path, and (from 0.8.0) a layer offset.
    fn payload_item(&self, at: &mut u64) -> Result<Payload> {
        let asset = self.string(u64::from(self.u32_at(*at)?))?;
        let prim = self.path(u64::from(self.u32_at(*at + 4)?))?;
        *at += 8;
        let mut offset = LayerOffset::IDENTITY;
        if self.since(8, 0) {
            offset = LayerOffset {
                offset: self.f64_at(*at)?,
                scale: self.f64_at(*at + 8)?,
            };
            *at += 16;
        }
        Ok(Payload {
            asset,
            prim,
            offset,
        })
    }

    /// Time samples: an offset to the times' ValueRep, an offset to the
    /// values, then a u64 count and that many ValueReps.
    fn time_samples_at(&self, at: u64, depth: usize) -> Result<Vec<(f64, Value)>> {
        let outside = || invalid("a crate time sample offset outside the file");
        let times_at = (at as i64)
            .checked_add(self.i64_at(at)?)
            .filter(|at| *at >= 0)
            .ok_or_else(outside)? as u64;
        let times = match self.value(Rep(self.u64_at(times_at)?), depth + 1)? {
            Value::DoubleVector(times) => times.to_vec(),
            Value::Array(crate::usd::sdf::Array::Double(times)) => times.to_vec(),
            _ => return Err(invalid("crate sample times that are not doubles")),
        };
        let values_offset_at = times_at + 8;
        let values_at = (values_offset_at as i64)
            .checked_add(self.i64_at(values_offset_at)?)
            .filter(|at| *at >= 0)
            .ok_or_else(outside)? as u64;
        let count = self.count_at(values_at, 8)?;
        if count != times.len() as u64 {
            return Err(invalid("crate time samples with more values than times"));
        }
        let mut samples = Vec::with_capacity(times.len());
        for (index, time) in times.into_iter().enumerate() {
            let rep = Rep(self.u64_at(values_at + 8 + index as u64 * 8)?);
            samples.push((time, self.value(rep, depth + 1)?));
        }
        samples.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(samples)
    }
}
