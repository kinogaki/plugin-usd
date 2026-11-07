// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The checks Sdf's schema makes while parsing (`pxr/usd/sdf/schema.cpp`
//! validators): variant names, arc and target paths, relocates, display
//! units, and duplicate list op items.

use super::Reader;
use crate::usd::error::Result;
use crate::usd::sdf::identifier::continue_len;
use crate::usd::sdf::{Path, has_duplicates};
use std::hash::Hash;

/// Display unit names (`SdfGetUnitFromName` over `_SDF_UNITS`).
const UNIT_NAMES: [&str; 13] = [
    "mm", "cm", "dm", "m", "km", "in", "ft", "yd", "mi", "deg", "rad", "%", "default",
];

impl Reader<'_> {
    /// A variant name: an optional `.`, then identifier continue
    /// characters, `|` and `-` (`SdfSchema::IsValidVariantIdentifier`).
    pub(super) fn check_variant_name(&self, text: &str) -> Result<()> {
        let bytes = text.as_bytes();
        let mut index = usize::from(bytes.first() == Some(&b'.'));
        while index < bytes.len() {
            if matches!(bytes[index], b'|' | b'-') {
                index += 1;
                continue;
            }
            let width = continue_len(bytes, index);
            if width == 0 {
                return Err(self.error(format!("'{text}' is not a valid variant name")));
            }
            index += width;
        }
        Ok(())
    }

    /// A reference or payload prim path: empty, or an absolute prim path
    /// without variant selections.
    pub(super) fn check_arc_path(&self, path: &Path) -> Result<()> {
        if path.is_empty() {
            return Ok(());
        }
        if path.has_variant_selection() {
            return Err(self.error("reference and payload paths cannot contain variant selections"));
        }
        if !(path.is_absolute() && path.is_prim()) {
            return Err(
                self.error("a reference or payload path must be empty or an absolute prim path")
            );
        }
        Ok(())
    }

    /// An inherits or specializes path: an absolute prim path without
    /// variant selections.
    pub(super) fn check_class_path(&self, path: &Path) -> Result<()> {
        if path.has_variant_selection() {
            return Err(
                self.error("inherit and specialize paths cannot contain variant selections")
            );
        }
        if !(path.is_absolute() && path.is_prim()) {
            return Err(self.error(format!("{path} is not a valid prim path")));
        }
        Ok(())
    }

    /// A relationship target or connection: a prim or property path
    /// without variant selections.
    pub(super) fn check_target_path(&self, path: &Path) -> Result<()> {
        if path.has_variant_selection() {
            return Err(self.error("target paths cannot contain variant selections"));
        }
        if !(path.is_prim() || path.is_property()) {
            return Err(self.error(format!("{path} is not a valid prim or property scene path")));
        }
        Ok(())
    }

    /// A relocates source: a prim path (not a root, not empty).
    pub(super) fn check_relocates_source(&self, path: &Path) -> Result<()> {
        if !path.is_prim() {
            return Err(self.error(format!("{path} is not a valid relocates source path")));
        }
        Ok(())
    }

    /// A relocates target: empty or a prim path.
    pub(super) fn check_relocates_target(&self, path: &Path) -> Result<()> {
        if !path.is_empty() && !path.is_prim() {
            return Err(self.error(format!("{path} is not a valid relocates target path")));
        }
        Ok(())
    }

    pub(super) fn check_display_unit(&self, name: &str) -> Result<()> {
        if UNIT_NAMES.contains(&name) {
            return Ok(());
        }
        Err(self.error(format!("'{name}' is not a valid display unit")))
    }

    /// No item may appear twice in a list op part.
    pub(super) fn check_unique<T: PartialEq + Hash>(&self, items: &[T]) -> Result<()> {
        match has_duplicates(items) {
            true => Err(self.error("duplicate items in a list op")),
            false => Ok(()),
        }
    }
}
