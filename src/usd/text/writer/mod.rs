// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The usda writer: [`LayerData`] to text exactly as OpenUSD's usda file
//! format writes it (`pxr/usd/sdf/usdaFileFormat.cpp` `_WriteLayer` and the
//! `Sdf_Write*` functions of `fileIO_Common.cpp`), so a layer usdcat prints
//! and a layer this crate prints are the same bytes.
//!
//! - `mod.rs`: the writer state, the layer header and root prims
//! - `order`: which fields are metadata, and their order
//! - `prims`: prims, their metadata and bodies, variant sets, variants
//! - `properties`: attributes, relationships, connections and targets
//! - `values`: scalars, tuples, arrays, dictionaries, list ops, samples
//! - `arrays`: numeric arrays, formatted in parallel chunks

mod arrays;
mod order;
mod prims;
mod properties;
mod values;

use crate::usd::error::Result;
use crate::usd::sdf::{LayerData, Path, Spec, Token, Value, field};
use crate::usd::text::quote::{asset_into, quote_into};

/// The layer as usda text.
pub fn write_usda(layer: &LayerData) -> Result<Vec<u8>> {
    let preformatted = arrays::Preformatted::gather(layer);
    let mut writer = Writer {
        layer,
        out: Vec::with_capacity(4096),
        preformatted,
    };
    writer.write_layer()?;
    Ok(writer.out)
}

struct Writer<'a> {
    layer: &'a LayerData,
    out: Vec<u8>,
    preformatted: arrays::Preformatted,
}

impl Writer<'_> {
    fn put(&mut self, text: &str) {
        self.out.extend_from_slice(text.as_bytes());
    }

    fn byte(&mut self, byte: u8) {
        self.out.push(byte);
    }

    /// Four spaces per level.
    fn indent(&mut self, levels: usize) {
        for _ in 0..levels {
            self.out.extend_from_slice(b"    ");
        }
    }

    fn quoted(&mut self, text: &str) {
        quote_into(&mut self.out, text, true);
    }

    fn asset(&mut self, text: &str) {
        asset_into(&mut self.out, text);
    }

    /// `<path>`.
    fn path_ref(&mut self, path: &Path) {
        self.byte(b'<');
        let text = path.text();
        self.put(&text);
        self.byte(b'>');
    }

    /// The layer: `#usda 1.0`, the metadata section (comment first, then
    /// fields in byte order, subLayers with their offsets, layer
    /// relocates), the root prim order, and each root prim after a blank
    /// line.
    fn write_layer(&mut self) -> Result<()> {
        self.put("#usda 1.0\n");
        let root = self.layer.root();
        // The metadata section is written only when it holds something.
        let start = self.out.len();
        self.put("(\n");
        let header_start = self.out.len();
        if let Some(Value::String(comment)) = root.get(field::COMMENT)
            && !comment.is_empty()
        {
            self.indent(1);
            self.quoted(comment);
            self.put("\n");
        }
        for (name, value) in order::metadata_fields(root, true) {
            match name.as_str() {
                field::DOCUMENTATION => {
                    if let Some(text) = value.as_token().filter(|text| !text.is_empty()) {
                        self.indent(1);
                        self.put("doc = ");
                        self.quoted(text);
                        self.put("\n");
                    }
                }
                field::SUB_LAYERS => self.sublayers(root, value),
                field::HAS_OWNED_SUB_LAYERS => {
                    if value.as_bool() == Some(true) {
                        self.indent(1);
                        self.put("hasOwnedSubLayers = true\n");
                    }
                }
                _ => self.simple_field(1, name, value)?,
            }
        }
        if let Some(Value::Relocates(pairs)) = root.get(field::LAYER_RELOCATES)
            && !pairs.is_empty()
        {
            self.relocates(1, true, pairs);
        }
        if self.out.len() == header_start {
            self.out.truncate(start);
        } else {
            self.put(")\n");
        }
        if let Some(order) = root.get(field::PRIM_ORDER).and_then(Value::as_tokens)
            && order.len() > 1
        {
            self.put("\nreorder rootPrims = ");
            self.name_vector(order);
            self.put("\n");
        }
        let root_path = Path::absolute_root();
        for name in root.tokens(field::PRIM_CHILDREN) {
            self.put("\n");
            let path = root_path.child(name.clone());
            if self.layer.has_spec(&path) {
                self.write_prim(&path, 0)?;
            }
        }
        self.put("\n");
        Ok(())
    }

    /// `subLayers = [ @a@ (offset = 1; scale = 2), ... ]`.
    fn sublayers(&mut self, root: &Spec, value: &Value) {
        let Some(paths) = value.as_tokens() else {
            return;
        };
        let offsets = match root.get(field::SUB_LAYER_OFFSETS) {
            Some(Value::LayerOffsetVector(offsets)) => offsets.as_slice(),
            _ => &[],
        };
        self.indent(1);
        self.put("subLayers = [\n");
        for (index, path) in paths.iter().enumerate() {
            self.indent(2);
            self.asset(path);
            if let Some(offset) = offsets.get(index) {
                self.layer_offset(0, false, offset.offset, offset.scale);
            }
            self.put(if index + 1 < paths.len() { ",\n" } else { "\n" });
        }
        self.indent(1);
        self.put("]\n");
    }

    /// A name vector: one quoted name, or `["a", "b"]`.
    fn name_vector(&mut self, names: &[Token]) {
        if names.len() > 1 {
            self.byte(b'[');
        }
        for (index, name) in names.iter().enumerate() {
            if index > 0 {
                self.put(", ");
            }
            self.quoted(name);
        }
        if names.len() > 1 {
            self.byte(b']');
        }
    }
}
