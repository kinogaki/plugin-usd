// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Composed metadata, after UsdStage's `MetadataValueComposer` and its
//! special cases (`stage.cpp`: `_ComposeGeneralMetadataImpl`,
//! `_GetPrimSpecifierImpl`, `_GetPrimTypeNameImpl`): the strongest opinion
//! wins, except that dictionaries merge (stronger keys first) and list ops
//! compose over weaker ones, ending as the explicit list they give applied
//! to nothing. A prim's specifier is its strongest def (or class), else
//! over; its type the strongest non-empty one. Composition arcs, values,
//! children and read-only fields are not metadata (`_IsPrivateFieldKey`).

use std::sync::Arc;

use super::{Flattener, Opinion};
use crate::usd::compose::assets;
use crate::usd::compose::expressions::{Evaluated, evaluate, is_expression};
use crate::usd::compose::graph::PrimIndex;
use crate::usd::sdf::schema::{applies, find_field};
use crate::usd::sdf::{
    Array, Dictionary, LayerOffset, ListOp, Path, SpecType, Specifier, Token, Value, field,
};

const PRIVATE_NAMES: [&str; 25] = [
    "inheritPaths",
    "layerRelocates",
    "payload",
    "references",
    "specializes",
    "subLayers",
    "subLayerOffsets",
    "variantSelection",
    "variantSetNames",
    "clips",
    "clipSets",
    "default",
    "timeSamples",
    "spline",
    "connectionPaths",
    "targetPaths",
    "primChildren",
    "properties",
    "variantChildren",
    "variantSetChildren",
    "targetChildren",
    "connectionChildren",
    "expressionChildren",
    "mapperChildren",
    "mapperArgChildren",
];

/// The fields Sdf's schema defines for each spec type (`schema.cpp`'s
/// `_Define`); plugin metadata comes from the sdf schema table.
const LAYER_CORE: [&str; 20] = [
    "colorConfiguration",
    "colorManagementSystem",
    "comment",
    "customLayerData",
    "defaultPrim",
    "documentation",
    "endTimeCode",
    "expressionVariables",
    "framesPerSecond",
    "framePrecision",
    "hasOwnedSubLayers",
    "owner",
    "sessionOwner",
    "startTimeCode",
    "timeCodesPerSecond",
    "endFrame",
    "startFrame",
    "primChildren",
    "layerRelocates",
    "primOrder",
];
const PRIM_CORE: [&str; 35] = [
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
    "active",
    "assetInfo",
    "clips",
    "clipSets",
    "customData",
    "displayGroupOrder",
    "displayName",
    "documentation",
    "hidden",
    "instanceable",
    "kind",
    "payload",
    "permission",
    "prefix",
    "prefixSubstitutions",
    "suffix",
    "suffixSubstitutions",
    "symmetricPeer",
    "symmetryArguments",
    "symmetryFunction",
    "typeName",
    "apiSchemas",
];
const PROPERTY_CORE: [&str; 16] = [
    "custom",
    "variability",
    "comment",
    "default",
    "assetInfo",
    "customData",
    "displayGroup",
    "displayName",
    "documentation",
    "hidden",
    "permission",
    "prefix",
    "suffix",
    "symmetricPeer",
    "symmetryArguments",
    "symmetryFunction",
];
const ATTRIBUTE_CORE: [&str; 10] = [
    "typeName",
    "spline",
    "connectionChildren",
    "connectionPaths",
    "displayUnit",
    "timeSamples",
    "allowedTokens",
    "arraySizeConstraint",
    "colorSpace",
    "limits",
];
const RELATIONSHIP_CORE: [&str; 3] = ["targetChildren", "targetPaths", "noLoadHint"];

/// Whether a field is not metadata to flatten: private, or not valid on the
/// spec type (`SdfSpec::SetInfo` refuses it).
pub(super) fn private_field(name: &str, kind: SpecType) -> bool {
    PRIVATE_NAMES.contains(&name) || !valid_field(name, kind)
}

/// Whether Sdf's schema (with the plugin metadata) lets `name` be set on a
/// spec of `kind`.
fn valid_field(name: &str, kind: SpecType) -> bool {
    let (core, mask): (&[&str], u8) = match kind {
        SpecType::PseudoRoot => (&LAYER_CORE, applies::LAYER),
        SpecType::Prim => (&PRIM_CORE, applies::PRIM),
        SpecType::Attribute if ATTRIBUTE_CORE.contains(&name) => return true,
        SpecType::Attribute => (&PROPERTY_CORE, applies::ATTRIBUTE),
        SpecType::Relationship if RELATIONSHIP_CORE.contains(&name) => return true,
        SpecType::Relationship => (&PROPERTY_CORE, applies::RELATIONSHIP),
        _ => return false,
    };
    core.contains(&name) || find_field(name).is_some_and(|found| found.applies & mask != 0)
}

/// Whether a value composes over weaker ones (a dictionary or a list op).
fn composes_over(value: &Value) -> bool {
    matches!(
        value,
        Value::Dictionary(_)
            | Value::TokenListOp(_)
            | Value::StringListOp(_)
            | Value::PathListOp(_)
            | Value::ReferenceListOp(_)
            | Value::PayloadListOp(_)
            | Value::IntListOp(_)
            | Value::Int64ListOp(_)
            | Value::UIntListOp(_)
            | Value::UInt64ListOp(_)
            | Value::UnregisteredListOp(_)
    )
}

/// `strong` over `weak`, or `strong` when they cannot compose.
fn list_over<T: Clone + PartialEq>(
    strong: &Arc<ListOp<T>>,
    weak: &Arc<ListOp<T>>,
) -> Arc<ListOp<T>> {
    match strong.compose_over(weak) {
        Some(made) => Arc::new(made),
        None => strong.clone(),
    }
}

/// `strong` composed over `weak` (`VtValueTryComposeOver`): dictionaries
/// merge, list ops of one kind compose; otherwise the weaker value stays.
fn compose_over(strong: &Value, weak: &Value) -> Value {
    match (strong, weak) {
        (Value::Dictionary(a), Value::Dictionary(b)) => {
            Value::Dictionary(Arc::new(merged_dictionary(a, b)))
        }
        (Value::TokenListOp(a), Value::TokenListOp(b)) => Value::TokenListOp(list_over(a, b)),
        (Value::StringListOp(a), Value::StringListOp(b)) => Value::StringListOp(list_over(a, b)),
        (Value::PathListOp(a), Value::PathListOp(b)) => Value::PathListOp(list_over(a, b)),
        (Value::ReferenceListOp(a), Value::ReferenceListOp(b)) => {
            Value::ReferenceListOp(list_over(a, b))
        }
        (Value::PayloadListOp(a), Value::PayloadListOp(b)) => Value::PayloadListOp(list_over(a, b)),
        (Value::IntListOp(a), Value::IntListOp(b)) => Value::IntListOp(list_over(a, b)),
        (Value::Int64ListOp(a), Value::Int64ListOp(b)) => Value::Int64ListOp(list_over(a, b)),
        (Value::UIntListOp(a), Value::UIntListOp(b)) => Value::UIntListOp(list_over(a, b)),
        (Value::UInt64ListOp(a), Value::UInt64ListOp(b)) => Value::UInt64ListOp(list_over(a, b)),
        (Value::UnregisteredListOp(a), Value::UnregisteredListOp(b)) => {
            Value::UnregisteredListOp(list_over(a, b))
        }
        _ => weak.clone(),
    }
}

/// A dictionary with `strong`'s entries over `weak`'s, nested dictionaries
/// merged (`VtDictionaryOverRecursive`).
fn merged_dictionary(strong: &Dictionary, weak: &Dictionary) -> Dictionary {
    let mut made = weak.clone();
    for (key, value) in strong {
        if let (Value::Dictionary(a), Some(Value::Dictionary(b))) = (value, made.get(key)) {
            let merged = merged_dictionary(a, b);
            made.insert(key.clone(), Value::Dictionary(Arc::new(merged)));
            continue;
        }
        made.insert(key.clone(), value.clone());
    }
    made
}

fn explicit<T: Clone + PartialEq>(list: &Arc<ListOp<T>>) -> Arc<ListOp<T>> {
    Arc::new(ListOp::explicit(list.applied()))
}

/// A composed value made final: list ops as their explicit lists.
fn finalized(value: Value) -> Value {
    match &value {
        Value::TokenListOp(list) => Value::TokenListOp(explicit(list)),
        Value::StringListOp(list) => Value::StringListOp(explicit(list)),
        Value::PathListOp(list) => Value::PathListOp(explicit(list)),
        Value::ReferenceListOp(list) => Value::ReferenceListOp(explicit(list)),
        Value::PayloadListOp(list) => Value::PayloadListOp(explicit(list)),
        Value::IntListOp(list) => Value::IntListOp(explicit(list)),
        Value::Int64ListOp(list) => Value::Int64ListOp(explicit(list)),
        Value::UIntListOp(list) => Value::UIntListOp(explicit(list)),
        Value::UInt64ListOp(list) => Value::UInt64ListOp(explicit(list)),
        Value::UnregisteredListOp(list) => Value::UnregisteredListOp(explicit(list)),
        _ => value,
    }
}

impl Flattener<'_> {
    /// The field `name` of the opinion's spec.
    pub(super) fn opinion_field(&self, opinion: &Opinion, name: &str) -> Option<&Value> {
        self.composer
            .layers
            .layer(opinion.layer)
            .get(&opinion.path, name)
    }

    /// The spec type of the opinion's spec.
    pub(super) fn opinion_kind(&self, opinion: &Opinion) -> SpecType {
        self.composer
            .layers
            .layer(opinion.layer)
            .spec(&opinion.path)
            .map_or(SpecType::Unknown, |spec| spec.kind)
    }

    /// The composed `active` (true when unauthored).
    pub(super) fn is_active(&self, opinions: &[Opinion]) -> bool {
        opinions
            .iter()
            .find_map(|opinion| self.opinion_field(opinion, field::ACTIVE))
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    /// Every metadata field of the prim's specs, composed, on the spec at `path`.
    pub(super) fn prim_metadata(&mut self, index: &PrimIndex, opinions: &[Opinion], path: &Path) {
        let mut fields: Vec<Token> = Vec::new();
        for opinion in opinions {
            let Some(spec) = self
                .composer
                .layers
                .layer(opinion.layer)
                .spec(&opinion.path)
            else {
                continue;
            };
            for (name, _) in &spec.fields {
                if !private_field(name, SpecType::Prim) && !fields.contains(name) {
                    fields.push(name.clone());
                }
            }
        }
        let specifier = Token::new(field::SPECIFIER);
        if !fields.contains(&specifier) {
            fields.push(specifier);
        }
        for key in fields {
            let value = match key.as_str() {
                field::SPECIFIER => Some(Value::Specifier(self.specifier(index, opinions))),
                field::TYPE_NAME => {
                    let name = self.type_name(opinions);
                    (!name.is_empty()).then_some(Value::Token(name))
                }
                _ => self.composed_value(index, opinions, &key),
            };
            if let Some(value) = value {
                self.out.set(path, &key, value);
            }
        }
    }

    /// The strongest def specifier; else the strongest class one, but a
    /// class due to a direct inherit only when nothing defines the prim;
    /// else over.
    fn specifier(&self, index: &PrimIndex, opinions: &[Opinion]) -> Specifier {
        let mut resolved = Specifier::Over;
        for opinion in opinions {
            let Some(found) = self
                .opinion_field(opinion, field::SPECIFIER)
                .and_then(Value::as_specifier)
            else {
                continue;
            };
            if found == Specifier::Over {
                continue;
            }
            if found != Specifier::Class || !index.due_to_direct_inherit(opinion.node) {
                return found;
            }
            resolved = Specifier::Class;
        }
        resolved
    }

    /// The strongest non-empty type name other than `__AnyType__`.
    pub(super) fn type_name(&self, opinions: &[Opinion]) -> Token {
        opinions
            .iter()
            .filter_map(|opinion| {
                self.opinion_field(opinion, field::TYPE_NAME)
                    .and_then(Value::as_token)
            })
            .find(|name| !name.is_empty() && name.as_str() != "__AnyType__")
            .cloned()
            .unwrap_or_default()
    }

    /// The weakest opinion's own value of `name`.
    pub(super) fn weakest_spec_field(&self, opinions: &[Opinion], name: &str) -> Option<&Value> {
        self.opinion_field(opinions.last()?, name)
    }

    /// The composed value of metadata field `name` (none when unauthored):
    /// each opinion moved into stage terms first.
    pub(super) fn composed_value(
        &self,
        index: &PrimIndex,
        opinions: &[Opinion],
        name: &str,
    ) -> Option<Value> {
        let mut value: Option<Value> = None;
        for opinion in opinions {
            let Some(found) = self.opinion_field(opinion, name) else {
                continue;
            };
            let next = self.to_stage(index, opinion, found);
            value = Some(match value {
                None => next,
                Some(strong) => compose_over(&strong, &next),
            });
            if !value.as_ref().is_some_and(composes_over) {
                break;
            }
        }
        value.map(finalized)
    }

    /// A field value moved into stage terms (`_FieldValueToStageXf`): asset
    /// paths anchored at their layer, time codes shifted by the layer's
    /// offset.
    pub(super) fn to_stage(&self, index: &PrimIndex, opinion: &Opinion, value: &Value) -> Value {
        match value {
            Value::Asset(asset) => {
                let evaluated = self.evaluated_asset(index, opinion, asset);
                Value::Asset(self.anchor_asset(&evaluated, opinion.layer))
            }
            Value::Array(Array::Asset(items)) => {
                // Anchored up to the first empty one (SdfAnchorAssetPaths stops there).
                let mut out = Vec::with_capacity(items.len());
                let mut anchoring = true;
                for item in items.iter() {
                    if !anchoring {
                        out.push(item.clone());
                        continue;
                    }
                    let evaluated = self.evaluated_asset(index, opinion, item);
                    if evaluated.is_empty() {
                        anchoring = false;
                        out.push(item.clone());
                        continue;
                    }
                    out.push(self.anchor_asset(&evaluated, opinion.layer));
                }
                Value::Array(Array::Asset(Arc::new(out)))
            }
            Value::TimeCode(time) => {
                Value::TimeCode(self.layer_offset(index, opinion).apply(*time))
            }
            Value::Array(Array::TimeCode(times)) => {
                let offset = self.layer_offset(index, opinion);
                Value::Array(Array::TimeCode(Arc::new(
                    times.iter().map(|&time| offset.apply(time)).collect(),
                )))
            }
            Value::Dictionary(held) => {
                // Dictionary values move too (VtValueTryTransform recurses).
                Value::Dictionary(Arc::new(
                    held.iter()
                        .map(|(key, entry)| (key.clone(), self.to_stage(index, opinion, entry)))
                        .collect(),
                ))
            }
            _ => value.clone(),
        }
    }

    /// The offset from the opinion's layer into stage time.
    pub(super) fn layer_offset(&self, index: &PrimIndex, opinion: &Opinion) -> LayerOffset {
        let stack = self.composer.stacks.stack(index.node(opinion.node).stack);
        index
            .offset_to_root(opinion.node)
            .concatenate(&stack.offsets[opinion.position])
    }

    /// An asset path expression evaluated with the expression variables of
    /// the layer stack that authored it; the path itself when it is no
    /// expression, fails, or gives None.
    fn evaluated_asset(&self, index: &PrimIndex, opinion: &Opinion, asset: &Token) -> Token {
        if !is_expression(asset) {
            return asset.clone();
        }
        let stack = self.composer.stacks.stack(index.node(opinion.node).stack);
        match evaluate(asset, &stack.variables) {
            Evaluated::String(text) => text,
            _ => asset.clone(),
        }
    }

    /// An asset path as flattening anchors it (`SdfAnchorAssetPaths`):
    /// anchored at the layer when that differs from the path on its own,
    /// except that a search path whose anchored file does not exist stays.
    fn anchor_asset(&self, asset: &Token, layer: usize) -> Token {
        if asset.is_empty() {
            return asset.clone();
        }
        let anchored = assets::anchored(asset, self.composer.layers.identifier(layer));
        let search = !asset.starts_with('.') && !assets::rooted(asset);
        if search && !self.composer.layers.exists(&anchored) {
            return asset.clone();
        }
        if assets::normalize(asset) == anchored {
            return asset.clone();
        }
        Token::from(anchored)
    }
}
