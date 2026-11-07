// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The metadata fields the text format reads by key, with their value types
//! and the specs they apply to: OpenUSD's `SdfSchema` registrations
//! (`pxr/usd/sdf/schema.cpp`) plus the fields the standard plugins declare
//! as `SdfMetadata` (usd, usdGeom, usdShade, usdSkel, usdUI, usdPhysics,
//! usdRender, usdImaging, exec). A key found here parses as its type; any
//! other key keeps its text as an unregistered value, as Sdf does.
//!
//! Fields with their own syntax (references, payload, inherits, kind,
//! permission, subLayers, ...) are read by the parser directly.

use super::value_type::ValueKind;

/// Which specs a field applies to (bit set).
pub mod applies {
    /// The layer (pseudo-root).
    pub const LAYER: u8 = 1;
    /// Prims and variants.
    pub const PRIM: u8 = 2;
    /// Attributes.
    pub const ATTRIBUTE: u8 = 4;
    /// Relationships.
    pub const RELATIONSHIP: u8 = 8;
    /// Attributes and relationships.
    pub const PROPERTY: u8 = ATTRIBUTE | RELATIONSHIP;
}

/// How a field's value is spelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldForm {
    /// One value of the kind (an array when `array`).
    Plain,
    /// A dictionary.
    Dictionary,
    /// A token list op.
    TokenListOp,
    /// A string list op.
    StringListOp,
    /// An int64 list op.
    Int64ListOp,
}

/// One registered field.
#[derive(Clone, Copy, Debug)]
pub struct MetadataField {
    /// The key.
    pub name: &'static str,
    /// How its value is spelled.
    pub form: FieldForm,
    /// The value kind of a plain field; none for `default`.
    pub kind: Option<ValueKind>,
    /// Whether a plain field holds an array.
    pub array: bool,
    /// The specs it is metadata for; 0 for non-metadata fields a key must
    /// not overwrite.
    pub applies: u8,
}

const fn plain(name: &'static str, kind: ValueKind, array: bool, applies: u8) -> MetadataField {
    MetadataField {
        name,
        form: FieldForm::Plain,
        kind: Some(kind),
        array,
        applies,
    }
}

const fn dictionary(name: &'static str, applies: u8) -> MetadataField {
    MetadataField {
        name,
        form: FieldForm::Dictionary,
        kind: Some(ValueKind::Dictionary),
        array: false,
        applies,
    }
}

const fn list(name: &'static str, form: FieldForm, kind: ValueKind, applies: u8) -> MetadataField {
    MetadataField {
        name,
        form,
        kind: Some(kind),
        array: false,
        applies,
    }
}

#[rustfmt::skip]
const FIELDS: &[MetadataField] = {
    use applies::*;
    use ValueKind as K;
    &[
        // Layers.
        plain("colorConfiguration", K::Asset, false, LAYER),
        plain("colorManagementSystem", K::Token, false, LAYER),
        dictionary("customLayerData", LAYER),
        plain("defaultPrim", K::Token, false, LAYER),
        plain("endTimeCode", K::Double, false, LAYER),
        dictionary("expressionVariables", LAYER),
        plain("framesPerSecond", K::Double, false, LAYER),
        plain("framePrecision", K::Int, false, LAYER),
        plain("hasOwnedSubLayers", K::Bool, false, LAYER),
        plain("owner", K::String, false, LAYER),
        plain("sessionOwner", K::String, false, LAYER),
        plain("startTimeCode", K::Double, false, LAYER),
        plain("timeCodesPerSecond", K::Double, false, LAYER),
        plain("endFrame", K::Double, false, LAYER),
        plain("startFrame", K::Double, false, LAYER),
        dictionary("fallbackPrimTypes", LAYER),
        plain("metersPerUnit", K::Double, false, LAYER),
        plain("upAxis", K::Token, false, LAYER),
        plain("kilogramsPerUnit", K::Double, false, LAYER),
        plain("renderSettingsPrimPath", K::String, false, LAYER),
        plain("documentation", K::String, false, LAYER | PRIM | PROPERTY),
        // Prims.
        plain("active", K::Bool, false, PRIM),
        dictionary("clips", PRIM),
        list("clipSets", FieldForm::StringListOp, K::StringListOp, PRIM),
        plain("displayGroupOrder", K::String, true, PRIM),
        plain("instanceable", K::Bool, false, PRIM),
        plain("typeName", K::Token, false, PRIM),
        plain("payloadAssetDependencies", K::Asset, true, PRIM),
        list("apiSchemas", FieldForm::TokenListOp, K::TokenListOp, PRIM),
        list("inactiveIds", FieldForm::Int64ListOp, K::Int64ListOp, PRIM),
        // Prims and properties.
        dictionary("assetInfo", PRIM | PROPERTY),
        dictionary("customData", PRIM | PROPERTY),
        plain("displayName", K::String, false, PRIM | PROPERTY),
        plain("hidden", K::Bool, false, PRIM | PROPERTY),
        plain("prefix", K::String, false, PRIM | PROPERTY),
        plain("suffix", K::String, false, PRIM | PROPERTY),
        plain("symmetricPeer", K::String, false, PRIM | PROPERTY),
        dictionary("symmetryArguments", PRIM | PROPERTY),
        dictionary("sdrMetadata", PRIM | ATTRIBUTE),
        dictionary("uiHints", PRIM | PROPERTY),
        // Properties.
        plain("displayGroup", K::String, false, PROPERTY),
        plain("renderType", K::Token, false, PROPERTY),
        // Attributes.
        plain("allowedTokens", K::Token, true, ATTRIBUTE),
        plain("arraySizeConstraint", K::Int64, false, ATTRIBUTE),
        plain("colorSpace", K::Token, false, ATTRIBUTE),
        dictionary("limits", ATTRIBUTE),
        plain("irIsInvertible", K::Bool, false, ATTRIBUTE),
        plain("irRole", K::Token, false, ATTRIBUTE),
        plain("constraintTargetIdentifier", K::Token, false, ATTRIBUTE),
        plain("elementSize", K::Int, false, ATTRIBUTE),
        plain("interpolation", K::Token, false, ATTRIBUTE),
        plain("unauthoredValuesIndex", K::Int, false, ATTRIBUTE),
        plain("connectability", K::Token, false, ATTRIBUTE),
        plain("weight", K::Float, false, ATTRIBUTE),
        plain("faceIndexPrimvar", K::Token, false, ATTRIBUTE),
        plain("faceOffsetPrimvar", K::Token, false, ATTRIBUTE),
        plain("uvPrimvar", K::Token, false, ATTRIBUTE),
        // Relationships.
        plain("noLoadHint", K::Bool, false, RELATIONSHIP),
        plain("bindMaterialAs", K::Token, false, RELATIONSHIP),
        plain("outputName", K::Token, false, RELATIONSHIP),
        // Fields that are not metadata, which a key must not overwrite.
        plain("comment", K::String, false, 0),
        plain("specifier", K::Specifier, false, 0),
        plain("custom", K::Bool, false, 0),
        plain("variability", K::Variability, false, 0),
        MetadataField { name: "default", form: FieldForm::Plain, kind: None, array: false, applies: 0 },
        plain("timeSamples", K::TimeSamples, false, 0),
        plain("targetPaths", K::PathListOp, false, 0),
        plain("connectionPaths", K::PathListOp, false, 0),
        plain("primChildren", K::TokenVector, false, 0),
        plain("properties", K::TokenVector, false, 0),
        plain("subLayerOffsets", K::LayerOffsetVector, false, 0),
        plain("primOrder", K::TokenVector, false, 0),
        plain("propertyOrder", K::TokenVector, false, 0),
        plain("spline", K::Spline, false, 0),
    ]
};

/// The field `name`, when the schema knows it.
pub fn find_field(name: &str) -> Option<&'static MetadataField> {
    FIELDS.iter().find(|field| field.name == name)
}
