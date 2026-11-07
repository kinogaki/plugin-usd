// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The usdc reader: crate files into [`LayerData`], after OpenUSD's
//! `pxr/usd/sdf/crateFile.cpp`. A crate is a bootstrap header (`PXR-USDC`,
//! the version, the table of contents' offset), the values, then six
//! structural sections: TOKENS, STRINGS, FIELDS (a token and a ValueRep
//! each), FIELDSETS (runs of field indexes), PATHS (a tree) and SPECS (path,
//! field set, spec type).
//!
//! Every read is checked against the file's bytes, so a damaged crate fails
//! with an error rather than reading out of bounds, and every allocation a
//! count sizes is charged to the read's budget first. A value several
//! fields or values share (one ValueRep) is decoded once and shared, so a
//! crate's sharing costs no copies. Array values decode in parallel after
//! the rest.
//!
//! - `sections`: bootstrap, table of contents, tokens, strings, fields,
//!   field sets and specs
//! - `paths`: the path tree
//! - `values`: ValueReps: inline values, dictionaries, list ops, samples
//! - `arrays`: arrays, compressed integers and floats

mod arrays;
mod paths;
mod sections;
mod values;

use std::collections::HashMap;
use std::sync::Mutex;

use super::compression::{CrateInt, decompress_integers};
use super::rep::Rep;
use crate::usd::error::{Error, Result};
use crate::usd::sdf::{LayerData, Path, SpecType, Token, Value};
use kinogaki_studio_api::format::ReadBudget;
use kinogaki_studio_api::geometry::parallel;

/// Crates smaller than this decode their arrays on the calling thread.
const PARALLEL_BYTES: usize = 1 << 20;

/// The newest crate version this reader reads (0.15).
pub const NEWEST_MINOR: u8 = 15;

/// Read a crate file into a layer, within `budget`.
pub fn read_usdc(data: &[u8], budget: &ReadBudget) -> Result<LayerData> {
    let mut reader = Crate {
        data,
        budget,
        minor: 0,
        patch: 0,
        tokens: Vec::new(),
        strings: Vec::new(),
        fields: Vec::new(),
        field_sets: Vec::new(),
        paths: Vec::new(),
        specs: Vec::new(),
        decoded: Mutex::default(),
    };
    reader.read_structure()?;
    reader.fill()
}

/// One spec record: crate path index, field set index, spec type.
#[derive(Clone, Copy)]
struct SpecRecord {
    path: u32,
    field_set: u32,
    kind: u32,
}

struct Crate<'a> {
    data: &'a [u8],
    budget: &'a ReadBudget,
    minor: u8,
    patch: u8,
    tokens: Vec<Token>,
    /// String index to its token.
    strings: Vec<Token>,
    /// Each field's name and ValueRep.
    fields: Vec<(Token, Rep)>,
    /// Field indexes, each set ended by `u32::MAX`.
    field_sets: Vec<u32>,
    paths: Vec<Path>,
    specs: Vec<SpecRecord>,
    /// Values decoded from the file's bytes, by their ValueRep: each once.
    decoded: Mutex<HashMap<u64, Value>>,
}

fn invalid(message: &str) -> Error {
    Error::invalid(message)
}

impl Crate<'_> {
    /// Whether the file's version is at least 0.minor.patch.
    fn since(&self, minor: u8, patch: u8) -> bool {
        self.minor > minor || (self.minor == minor && self.patch >= patch)
    }

    /// `count` bytes at `at`, checked.
    fn bytes(&self, at: u64, count: u64) -> Result<&[u8]> {
        let length = self.data.len() as u64;
        if at > length || count > length - at {
            return Err(invalid("a crate file read past its end"));
        }
        Ok(&self.data[at as usize..(at + count) as usize])
    }

    fn u64_at(&self, at: u64) -> Result<u64> {
        let bytes = self.bytes(at, 8)?;
        Ok(u64::from_le_bytes(bytes.try_into().expect("8 bytes")))
    }

    fn i64_at(&self, at: u64) -> Result<i64> {
        Ok(self.u64_at(at)? as i64)
    }

    fn u32_at(&self, at: u64) -> Result<u32> {
        let bytes = self.bytes(at, 4)?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("4 bytes")))
    }

    fn f64_at(&self, at: u64) -> Result<f64> {
        Ok(f64::from_bits(self.u64_at(at)?))
    }

    /// A count read from the file, bounded by what the file could hold.
    fn count_at(&self, at: u64, per_item: u64) -> Result<u64> {
        let count = self.u64_at(at)?;
        if count.saturating_mul(per_item.max(1)) > self.data.len() as u64 {
            return Err(invalid("a crate count larger than the file can hold"));
        }
        Ok(count)
    }

    fn token(&self, index: u64) -> Result<Token> {
        self.tokens
            .get(index as usize)
            .cloned()
            .ok_or_else(|| invalid("a token index outside the crate's tokens"))
    }

    fn string(&self, index: u64) -> Result<Token> {
        self.strings
            .get(index as usize)
            .cloned()
            .ok_or_else(|| invalid("a string index outside the crate's strings"))
    }

    /// The path at crate index `index`; entries the tree never fills stand
    /// for the empty path (a reference without a prim path).
    fn path(&self, index: u64) -> Result<Path> {
        self.paths
            .get(index as usize)
            .cloned()
            .ok_or_else(|| invalid("a path index outside the crate's paths"))
    }

    /// `count` integer-compressed integers at `at` (a u64 size, then the
    /// compressed bytes); the integers and the offset after them.
    fn compressed_ints<T: CrateInt>(&self, at: u64, count: usize) -> Result<(Vec<T>, u64)> {
        let size = self.u64_at(at)?;
        self.charge::<T>(count as u64)?;
        let values = if count > 0 {
            decompress_integers::<T>(self.bytes(at + 8, size)?, count)?
        } else {
            Vec::new()
        };
        Ok((values, at + 8 + size))
    }

    /// Specs and their fields into a layer; arrays decode last, in
    /// parallel, each distinct one once.
    fn fill(&self) -> Result<LayerData> {
        let mut layer = LayerData::new();
        layer.reserve(self.specs.len());
        let mut pending: Vec<(Path, usize, usize)> = Vec::new();
        let mut arrays: Vec<Rep> = Vec::new();
        let mut array_at: HashMap<u64, usize> = HashMap::new();
        for record in &self.specs {
            self.budget.check()?;
            let path = self.path(u64::from(record.path))?;
            if path.is_empty() {
                return Err(invalid("a crate spec with an empty path"));
            }
            let kind = SpecType::from_number(record.kind)
                .filter(|kind| *kind != SpecType::Unknown)
                .ok_or_else(|| invalid("a crate spec of an unknown type"))?;
            if !path.is_absolute_root() && layer.has_spec(&path) {
                return Err(invalid("a crate spec repeated"));
            }
            // Specs may share one field set: each spec's fields are its own.
            let set = (self.field_sets.get(record.field_set as usize..))
                .and_then(|rest| rest.iter().position(|&f| f == u32::MAX))
                .ok_or_else(|| invalid("a field set runs past its section"))?;
            self.charge::<(Token, Value)>(set as u64)?;
            let spec = layer.create_spec(path.clone(), kind);
            let mut at = record.field_set as usize;
            loop {
                let field = *self
                    .field_sets
                    .get(at)
                    .ok_or_else(|| invalid("a field set runs past its section"))?;
                if field == u32::MAX {
                    break;
                }
                let (name, rep) = self
                    .fields
                    .get(field as usize)
                    .ok_or_else(|| invalid("a field index outside the crate's fields"))?;
                if rep.is_array() && rep.payload() != 0 && !rep.is_array_edit() {
                    // Decoded below; the field keeps its place in the spec.
                    spec.set(name.clone(), Value::ValueBlock);
                    let slot = *array_at.entry(rep.0).or_insert_with(|| {
                        arrays.push(*rep);
                        arrays.len() - 1
                    });
                    pending.push((path.clone(), field as usize, slot));
                } else {
                    spec.set(name.clone(), self.value(*rep, 0)?);
                }
                at += 1;
            }
        }
        // Small crates decode on this thread: starting workers costs more.
        // A cancelled read decodes no further array.
        let decode = |rep: Rep| -> Result<Value> {
            self.budget.check()?;
            self.value(rep, 0)
        };
        let decoded: Vec<Result<Value>> = if self.data.len() < PARALLEL_BYTES {
            arrays.iter().map(|rep| decode(*rep)).collect()
        } else {
            parallel::map(arrays.len(), |index| decode(arrays[index]))
        };
        let decoded = decoded.into_iter().collect::<Result<Vec<Value>>>()?;
        for (path, field, slot) in pending {
            let name = &self.fields[field].0;
            layer.set(&path, name, decoded[slot].clone());
        }
        Ok(layer)
    }

    /// Charges `count` items of `T` the file counted to the read's budget.
    fn charge<T>(&self, count: u64) -> Result<()> {
        let bytes = count.saturating_mul(size_of::<T>() as u64);
        Ok(self.budget.allocate(bytes)?)
    }
}
