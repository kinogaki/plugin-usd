// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The usda reader: text layers into [`LayerData`], following OpenUSD's text
//! file format grammar (`pxr/usd/sdf/textFileFormatParser.h`) and its actions
//! (`textParserHelpers.cpp`, `parserHelpers.cpp`): the same specs, fields and
//! values Sdf's parser makes, so both write back the same text and crate.
//!
//! - `scan`: padding, comments, separators, keywords, identifiers
//! - `atoms`: numbers, strings, asset paths, path references
//! - `values`: typed scalars, tuples, arrays, dictionaries, unknown values
//! - `arrays`: large numeric arrays, parsed in parallel chunks
//! - `metadata`: metadata blocks of layers, prims, attributes, relationships
//! - `arcs`: references, payloads, inherits, specializes, variants, sublayers
//! - `specs`: the layer, prims, variant sets and variants
//! - `properties`: attributes and relationships
//! - `validate`: the checks Sdf's schema makes while parsing

mod arcs;
mod arrays;
mod atoms;
mod metadata;
mod properties;
mod scan;
mod specs;
mod validate;
mod values;

use crate::usd::error::{Error, Result};
use crate::usd::sdf::LayerData;

/// The deepest nesting of dictionaries, tuples and recorded values.
const MOST_NESTING: usize = 256;

/// Read usda text into a layer. Fails with an error naming the line and
/// column on text that is not a layer.
pub fn read_usda(text: &[u8]) -> Result<LayerData> {
    let mut reader = Reader {
        data: text,
        at: 0,
        layer: LayerData::new(),
        nesting: 0,
    };
    reader.read_layer()?;
    Ok(reader.layer)
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
    layer: LayerData,
    nesting: usize,
}

impl Reader<'_> {
    /// An invalid-input error at the cursor.
    fn error(&self, message: impl Into<String>) -> Error {
        let end = self.at.min(self.data.len());
        let before = &self.data[..end];
        let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
        let line_start = before
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |at| at + 1);
        let column = String::from_utf8_lossy(&before[line_start..])
            .chars()
            .count()
            + 1;
        Error::invalid(message).at(line, column)
    }

    /// A not-ported-feature error at the cursor.
    fn unsupported(&self, message: impl Into<String>) -> Error {
        let mut error = self.error(message);
        error.kind = crate::usd::ErrorKind::Unsupported;
        error
    }

    /// Enter a nested construct; fails past the nesting limit.
    fn enter(&mut self) -> Result<()> {
        self.nesting += 1;
        if self.nesting > MOST_NESTING {
            let mut error = self.error("values nest too deeply");
            error.kind = crate::usd::ErrorKind::Limit;
            return Err(error);
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.nesting -= 1;
    }
}
