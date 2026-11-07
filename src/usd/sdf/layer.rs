// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! A layer's contents, after OpenUSD's `SdfData`: specs keyed by path, each a
//! spec type and a list of named fields. This is the whole of a layer; usda
//! and usdc are two serializations of it, and composition reads it.
//!
//! Children are ordinary fields, as in Sdf: a prim's `primChildren` and
//! `properties` token vectors, `variantSetChildren`, a variant set's
//! `variantChildren`, a relationship's `targetChildren`. The pseudo-root
//! spec (`/`) holds the layer's metadata.

use super::path::{Path, PathMap};
use super::token::Token;
use super::value::Value;

/// `SdfSpecType`; the numbers are the crate's.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SpecType {
    Unknown = 0,
    Attribute = 1,
    Connection = 2,
    Expression = 3,
    Mapper = 4,
    MapperArg = 5,
    Prim = 6,
    PseudoRoot = 7,
    Relationship = 8,
    RelationshipTarget = 9,
    Variant = 10,
    VariantSet = 11,
}

impl SpecType {
    /// The spec type with crate number `number`.
    pub fn from_number(number: u32) -> Option<SpecType> {
        use SpecType::*;
        [
            Unknown,
            Attribute,
            Connection,
            Expression,
            Mapper,
            MapperArg,
            Prim,
            PseudoRoot,
            Relationship,
            RelationshipTarget,
            Variant,
            VariantSet,
        ]
        .get(number as usize)
        .copied()
    }
}

/// The field names Sdf defines (`SDF_FIELD_KEYS`, `SDF_CHILDREN_KEYS`).
#[allow(missing_docs)]
pub mod field {
    pub const ACTIVE: &str = "active";
    pub const API_SCHEMAS: &str = "apiSchemas";
    pub const ASSET_INFO: &str = "assetInfo";
    pub const COLOR_SPACE: &str = "colorSpace";
    pub const COMMENT: &str = "comment";
    pub const CONNECTION_CHILDREN: &str = "connectionChildren";
    pub const CONNECTION_PATHS: &str = "connectionPaths";
    pub const CUSTOM: &str = "custom";
    pub const CUSTOM_DATA: &str = "customData";
    pub const CUSTOM_LAYER_DATA: &str = "customLayerData";
    pub const DEFAULT: &str = "default";
    pub const DEFAULT_PRIM: &str = "defaultPrim";
    pub const DISPLAY_UNIT: &str = "displayUnit";
    pub const DOCUMENTATION: &str = "documentation";
    pub const END_TIME_CODE: &str = "endTimeCode";
    pub const EXPRESSION_VARIABLES: &str = "expressionVariables";
    pub const FRAMES_PER_SECOND: &str = "framesPerSecond";
    pub const HAS_OWNED_SUB_LAYERS: &str = "hasOwnedSubLayers";
    pub const INHERIT_PATHS: &str = "inheritPaths";
    pub const INSTANCEABLE: &str = "instanceable";
    pub const KIND: &str = "kind";
    pub const LAYER_RELOCATES: &str = "layerRelocates";
    pub const PAYLOAD: &str = "payload";
    pub const PERMISSION: &str = "permission";
    pub const PREFIX_SUBSTITUTIONS: &str = "prefixSubstitutions";
    pub const PRIM_CHILDREN: &str = "primChildren";
    pub const PRIM_ORDER: &str = "primOrder";
    pub const PROPERTIES: &str = "properties";
    pub const PROPERTY_ORDER: &str = "propertyOrder";
    pub const REFERENCES: &str = "references";
    pub const RELOCATES: &str = "relocates";
    pub const SPECIALIZES: &str = "specializes";
    pub const SPECIFIER: &str = "specifier";
    pub const SPLINE: &str = "spline";
    pub const START_TIME_CODE: &str = "startTimeCode";
    pub const SUB_LAYERS: &str = "subLayers";
    pub const SUB_LAYER_OFFSETS: &str = "subLayerOffsets";
    pub const SUFFIX_SUBSTITUTIONS: &str = "suffixSubstitutions";
    pub const SYMMETRY_FUNCTION: &str = "symmetryFunction";
    pub const TARGET_CHILDREN: &str = "targetChildren";
    pub const TARGET_PATHS: &str = "targetPaths";
    pub const TIME_CODES_PER_SECOND: &str = "timeCodesPerSecond";
    pub const TIME_SAMPLES: &str = "timeSamples";
    pub const TYPE_NAME: &str = "typeName";
    pub const VARIABILITY: &str = "variability";
    pub const VARIANT_CHILDREN: &str = "variantChildren";
    pub const VARIANT_SELECTION: &str = "variantSelection";
    pub const VARIANT_SET_CHILDREN: &str = "variantSetChildren";
    pub const VARIANT_SET_NAMES: &str = "variantSetNames";
}

/// One spec: its type and its fields in the order they were set.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    /// The spec type.
    pub kind: SpecType,
    /// The fields, by name, in authoring order.
    pub fields: Vec<(Token, Value)>,
}

impl Spec {
    /// The value of field `name`.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields
            .iter()
            .find(|(key, _)| key.as_str() == name)
            .map(|(_, value)| value)
    }

    /// Set field `name`, replacing an earlier value in place.
    pub fn set(&mut self, name: impl Into<Token> + AsRef<str>, value: Value) {
        if let Some(slot) = self
            .fields
            .iter_mut()
            .find(|(key, _)| key.as_str() == name.as_ref())
        {
            slot.1 = value;
            return;
        }
        self.fields.push((name.into(), value));
    }

    /// Remove field `name`; the value it had.
    pub fn erase(&mut self, name: &str) -> Option<Value> {
        let at = self
            .fields
            .iter()
            .position(|(key, _)| key.as_str() == name)?;
        Some(self.fields.remove(at).1)
    }

    /// The tokens of a token-vector field (children lists), or none.
    pub fn tokens(&self, name: &str) -> &[Token] {
        self.get(name).and_then(Value::as_tokens).unwrap_or(&[])
    }
}

/// A layer's specs, in creation order.
#[derive(Clone, Debug, Default)]
pub struct LayerData {
    paths: Vec<Path>,
    specs: Vec<Spec>,
    index: PathMap<usize>,
}

impl PartialEq for LayerData {
    /// Layers are equal when they hold the same specs with the same fields
    /// (in any creation order).
    fn eq(&self, other: &LayerData) -> bool {
        self.specs.len() == other.specs.len()
            && self.paths.iter().zip(&self.specs).all(|(path, spec)| {
                other.spec(path).is_some_and(|theirs| {
                    theirs.kind == spec.kind
                        && theirs.fields.len() == spec.fields.len()
                        && spec
                            .fields
                            .iter()
                            .all(|(name, value)| theirs.get(name) == Some(value))
                })
            })
    }
}

impl LayerData {
    /// An empty layer: the pseudo-root alone.
    pub fn new() -> LayerData {
        let mut layer = LayerData::default();
        layer.create_spec(Path::absolute_root(), SpecType::PseudoRoot);
        layer
    }

    /// Specs held, the pseudo-root included.
    pub fn len(&self) -> usize {
        self.specs.len()
    }

    /// Whether the layer has no specs (not even a pseudo-root).
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Every spec with its path, in creation order.
    pub fn iter(&self) -> impl Iterator<Item = (&Path, &Spec)> {
        self.paths.iter().zip(&self.specs)
    }

    /// The spec at `path`.
    pub fn spec(&self, path: &Path) -> Option<&Spec> {
        self.index.get(path).map(|&at| &self.specs[at])
    }

    /// The spec at `path`, to change.
    pub fn spec_mut(&mut self, path: &Path) -> Option<&mut Spec> {
        self.index.get(path).map(|&at| &mut self.specs[at])
    }

    /// The pseudo-root spec, holding layer metadata.
    pub fn root(&self) -> &Spec {
        self.spec(&Path::absolute_root())
            .expect("a layer has a pseudo-root")
    }

    /// The pseudo-root spec, to change.
    pub fn root_mut(&mut self) -> &mut Spec {
        self.spec_mut(&Path::absolute_root())
            .expect("a layer has a pseudo-root")
    }

    /// Whether a spec exists at `path`.
    pub fn has_spec(&self, path: &Path) -> bool {
        self.index.contains_key(path)
    }

    /// The spec at `path`, created with `kind` when absent.
    pub fn create_spec(&mut self, path: Path, kind: SpecType) -> &mut Spec {
        if let Some(&at) = self.index.get(&path) {
            return &mut self.specs[at];
        }
        let at = self.specs.len();
        self.index.insert(path.clone(), at);
        self.paths.push(path);
        self.specs.push(Spec {
            kind,
            fields: Vec::new(),
        });
        &mut self.specs[at]
    }

    /// The field `name` of the spec at `path`.
    pub fn get(&self, path: &Path, name: &str) -> Option<&Value> {
        self.spec(path)?.get(name)
    }

    /// Set the field `name` of an existing spec; false when there is none.
    pub fn set(&mut self, path: &Path, name: &str, value: Value) -> bool {
        match self.spec_mut(path) {
            Some(spec) => {
                spec.set(Token::new(name), value);
                true
            }
            None => false,
        }
    }

    /// Reserve room for `additional` more specs.
    pub fn reserve(&mut self, additional: usize) {
        self.paths.reserve(additional);
        self.specs.reserve(additional);
        self.index.reserve(additional);
    }
}
