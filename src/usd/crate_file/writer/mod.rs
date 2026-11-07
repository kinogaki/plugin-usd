// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The usdc writer: [`LayerData`] into a crate file, after OpenUSD's
//! `crateFile.cpp` packing (`CrateFile::_Write`, `_AddSpec`, `_PackValue`):
//! the bootstrap, the values each field holds, then the TOKENS, STRINGS,
//! FIELDS, FIELDSETS, PATHS and SPECS sections (integer-compressed, as from
//! 0.4.0) and the table of contents. Files are version 0.8.0, raised to
//! 0.9.0 for time codes, 0.10.0 for path expressions and 0.11.0 for layer
//! relocates, as OpenUSD raises them.
//!
//! Tokens, strings, paths, fields and field sets are deduplicated; arrays
//! sharing one column are written once. Large integer and float arrays are
//! compressed in parallel before packing.
//!
//! - `values`: packing a value into a ValueRep and its bytes
//! - `arrays`: arrays, their compression and the parallel pre-pass
//! - `sections`: the structural sections, the path tree and the bootstrap

mod arrays;
mod sections;
mod values;

use std::collections::HashMap;

use super::rep::Rep;
use crate::usd::error::{Error, Result};
use crate::usd::sdf::{LayerData, Path, PathKind, Spec, Token, field};

/// The crate version new files take.
const BASE_MINOR: u8 = 8;
/// Bytes reserved for the bootstrap.
const BOOTSTRAP: usize = 88;

/// The layer as a crate file.
pub fn write_usdc(layer: &LayerData) -> Result<Vec<u8>> {
    let mut writer = CrateWriter {
        out: vec![0; BOOTSTRAP],
        minor: BASE_MINOR,
        tokens: Vec::new(),
        token_index: HashMap::new(),
        strings: Vec::new(),
        string_index: HashMap::new(),
        paths: Vec::new(),
        path_index: HashMap::new(),
        empty_path: None,
        fields: Vec::new(),
        field_index: HashMap::new(),
        field_sets: Vec::new(),
        set_index: HashMap::new(),
        specs: Vec::new(),
        arrays: HashMap::new(),
        prepared: arrays::prepare(layer),
    };
    for (path, spec) in layer.iter() {
        writer.add_spec(path, spec)?;
    }
    writer.write_sections()?;
    Ok(writer.out)
}

struct CrateWriter {
    out: Vec<u8>,
    minor: u8,
    tokens: Vec<Token>,
    token_index: HashMap<Token, u32>,
    /// Each string's token index.
    strings: Vec<u32>,
    string_index: HashMap<Token, u32>,
    paths: Vec<Path>,
    path_index: HashMap<Path, u32>,
    /// The index standing for the empty path, once one is needed.
    empty_path: Option<u32>,
    fields: Vec<(u32, Rep)>,
    field_index: HashMap<(u32, Rep), u32>,
    field_sets: Vec<u32>,
    set_index: HashMap<Vec<u32>, u32>,
    /// Path index, field set index, spec type.
    specs: Vec<(u32, u32, u32)>,
    /// Arrays already written, by column identity.
    arrays: HashMap<usize, Rep>,
    /// Compressed array payloads prepared in parallel, by column identity.
    prepared: HashMap<usize, arrays::Prepared>,
}

impl CrateWriter {
    /// Raise the file's version to at least 0.minor.
    fn require(&mut self, minor: u8) {
        self.minor = self.minor.max(minor);
    }

    /// A spec: its fields packed and deduplicated, its field set added.
    fn add_spec(&mut self, path: &Path, spec: &Spec) -> Result<()> {
        let mut set = Vec::with_capacity(spec.fields.len());
        for (name, value) in &spec.fields {
            // OpenUSD's crate cannot hold a prim's relocates map or a
            // display unit (a TfEnum); refuse them as it does.
            if name.as_str() == field::RELOCATES || name.as_str() == field::DISPLAY_UNIT {
                return Err(Error::unsupported(
                    "a crate cannot hold prim relocates or display units",
                ));
            }
            let token = self.token(name);
            let rep = self.pack(value)?;
            set.push(self.field(token, rep));
        }
        let field_set = self.field_set(set);
        let path_index = self.path(path)?;
        self.specs.push((path_index, field_set, spec.kind as u32));
        Ok(())
    }

    /// The crate index of `token`, added when new.
    fn token(&mut self, token: &str) -> u32 {
        if let Some(&index) = self.token_index.get(token) {
            return index;
        }
        let index = self.tokens.len() as u32;
        let token = Token::new(token);
        self.tokens.push(token.clone());
        self.token_index.insert(token, index);
        index
    }

    /// The crate string index of `text`, added when new.
    fn string(&mut self, text: &str) -> u32 {
        if let Some(&index) = self.string_index.get(text) {
            return index;
        }
        let token = self.token(text);
        let index = self.strings.len() as u32;
        self.strings.push(token);
        self.string_index.insert(Token::new(text), index);
        index
    }

    /// The crate index of `path`, with its ancestors, target paths and
    /// element tokens added (`CrateFile::_AddPath`).
    fn path(&mut self, path: &Path) -> Result<u32> {
        if let Some(&index) = self.path_index.get(path) {
            return Ok(index);
        }
        if path.is_empty() {
            return Err(Error::invalid("an empty path in a crate spec"));
        }
        if let Some(target) = path.target_path()
            && !target.is_empty()
        {
            self.path(&target.clone())?;
        }
        if !path.is_absolute_root() {
            if !path.is_absolute() {
                return Err(Error::invalid(format!(
                    "the relative path {path} cannot be written to a crate"
                )));
            }
            self.path(&path.parent())?;
        }
        let element = element_token(path);
        self.token(&element);
        let index = self.paths.len() as u32;
        self.paths.push(path.clone());
        self.path_index.insert(path.clone(), index);
        Ok(index)
    }

    /// The crate index for a path value: the empty path is one index the
    /// tree never fills, reserved once.
    fn path_value(&mut self, path: &Path) -> Result<u32> {
        if !path.is_empty() {
            return self.path(path);
        }
        if let Some(index) = self.empty_path {
            return Ok(index);
        }
        let index = self.paths.len() as u32;
        self.paths.push(Path::empty());
        self.empty_path = Some(index);
        Ok(index)
    }

    /// The index of a field, deduplicated.
    fn field(&mut self, token: u32, rep: Rep) -> u32 {
        let next = self.fields.len() as u32;
        let index = *self.field_index.entry((token, rep)).or_insert(next);
        if index == next {
            self.fields.push((token, rep));
        }
        index
    }

    /// The start of a field set equal to `set`, added (with its terminator)
    /// when new.
    fn field_set(&mut self, set: Vec<u32>) -> u32 {
        if let Some(&start) = self.set_index.get(&set) {
            return start;
        }
        let start = self.field_sets.len() as u32;
        self.field_sets.extend_from_slice(&set);
        self.field_sets.push(u32::MAX);
        self.set_index.insert(set, start);
        start
    }

    /// The offset of the next byte.
    fn tell(&self) -> u64 {
        self.out.len() as u64
    }

    fn put_u64(&mut self, value: u64) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    fn put_u32(&mut self, value: u32) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    fn put_f64(&mut self, value: f64) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    /// Pad with zeros to a multiple of `alignment`.
    fn align(&mut self, alignment: usize) {
        while !self.out.len().is_multiple_of(alignment) {
            self.out.push(0);
        }
    }

    /// Write `value` at `offset`.
    fn patch_u64(&mut self, offset: u64, value: u64) {
        let at = offset as usize;
        self.out[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
}

/// The token a path's last element is stored under: a prim's or prim
/// property's name, otherwise the element's text (`{set=sel}`,
/// `[/target]`, `.name`).
fn element_token(path: &Path) -> String {
    match path.kind() {
        PathKind::Root | PathKind::Empty => String::new(),
        PathKind::Prim | PathKind::Property => path.name().to_owned(),
        PathKind::Variant => {
            let (set, variant) = path.variant_selection().expect("a variant selection");
            format!("{{{set}={variant}}}")
        }
        PathKind::Target => format!(
            "[{}]",
            path.target_path().map(Path::text).unwrap_or_default()
        ),
        PathKind::Mapper => format!(
            ".mapper[{}]",
            path.target_path().map(Path::text).unwrap_or_default()
        ),
        PathKind::Expression => ".expression".to_owned(),
        PathKind::Relational | PathKind::MapperArg => format!(".{}", path.name()),
    }
}
