// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Which of a spec's fields the writer puts in its metadata section
//! (`Sdf_IsMetadataField`: every field that is metadata for the spec's type,
//! and every field the type does not know, which may be plugin metadata),
//! and their order: `TfDictionaryLessThan` for prims and properties, byte
//! order for the layer.

use crate::usd::sdf::identifier::dictionary_order;
use crate::usd::sdf::{Spec, SpecType, Token, Value, field};

/// The fields each spec type knows that are not metadata.
const PRIM_FIELDS: [&str; 13] = [
    "specifier",
    "comment",
    "inheritPaths",
    "specializes",
    "primChildren",
    "primOrder",
    "properties",
    "propertyOrder",
    "references",
    "relocates",
    "variantSelection",
    "variantSetChildren",
    "variantSetNames",
];
const ATTRIBUTE_FIELDS: [&str; 10] = [
    "custom",
    "variability",
    "comment",
    "default",
    "typeName",
    "spline",
    "connectionChildren",
    "connectionPaths",
    "displayUnit",
    "timeSamples",
];
const RELATIONSHIP_FIELDS: [&str; 6] = [
    "custom",
    "variability",
    "comment",
    "default",
    "targetChildren",
    "targetPaths",
];
const LAYER_FIELDS: [&str; 6] = [
    "comment",
    "primChildren",
    "layerRelocates",
    "primOrder",
    "subLayers",
    "subLayerOffsets",
];

/// Whether field `name` belongs in the metadata section of a `kind` spec.
fn is_metadata(kind: SpecType, name: &str) -> bool {
    match kind {
        SpecType::Prim | SpecType::Variant => !PRIM_FIELDS.contains(&name),
        SpecType::Attribute => !ATTRIBUTE_FIELDS.contains(&name),
        SpecType::Relationship => !RELATIONSHIP_FIELDS.contains(&name),
        SpecType::PseudoRoot => !LAYER_FIELDS.contains(&name),
        _ => true,
    }
}

/// Fields a spec type's metadata section also takes in its own syntax.
fn extra(kind: SpecType, name: &str) -> bool {
    match kind {
        SpecType::PseudoRoot => name == field::SUB_LAYERS,
        SpecType::Prim | SpecType::Variant => matches!(
            name,
            field::PAYLOAD
                | field::REFERENCES
                | field::RELOCATES
                | field::INHERIT_PATHS
                | field::SPECIALIZES
                | field::VARIANT_SET_NAMES
                | field::VARIANT_SELECTION
        ),
        SpecType::Attribute => name == field::DISPLAY_UNIT,
        _ => false,
    }
}

/// The fields of `spec` for its metadata section, sorted (dictionary
/// order, or byte order for the layer), without `comment` (written first)
/// and a prim's `typeName` (written in its preamble).
pub(super) fn metadata_fields(spec: &Spec, layer: bool) -> Vec<(&Token, &Value)> {
    let mut fields: Vec<(&Token, &Value)> = spec
        .fields
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            if name == field::COMMENT {
                return false;
            }
            if name == field::TYPE_NAME && matches!(spec.kind, SpecType::Prim | SpecType::Variant) {
                return false;
            }
            is_metadata(spec.kind, name) || extra(spec.kind, name)
        })
        .map(|(name, value)| (name, value))
        .collect();
    if layer {
        fields.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    } else {
        fields.sort_by(|a, b| dictionary_order(a.0, b.0));
    }
    fields
}
