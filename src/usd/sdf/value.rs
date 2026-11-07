// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Field values, after OpenUSD's `VtValue` as layers hold them.
//!
//! Attribute values are scalars, Gf vectors, quaternions and matrices, and
//! arrays of those ([`Array`]); metadata adds dictionaries, list ops, time
//! samples, path and token vectors and the enumerations. Arrays, list ops and
//! other large values are shared (`Arc`), so cloning a value is cheap and
//! composed stages share their layers' arrays.
//!
//! Quaternions are stored as Gf lays them out: imaginary `i, j, k`, then the
//! real part (text writes the real part first). Matrices are row-major.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::half::Half;
use super::list_op::ListOp;
use super::path::Path;
use super::token::Token;
use super::value_type::ValueKind;

/// The kinds every attribute value can be, as `(Value variant, Array
/// variant, element type, kind)`; the macro below expands it.
macro_rules! attribute_kinds {
    ($callback:ident) => {
        $callback! {
            (Bool, bool),
            (UChar, u8),
            (Int, i32),
            (UInt, u32),
            (Int64, i64),
            (UInt64, u64),
            (Half, Half),
            (Float, f32),
            (Double, f64),
            (TimeCode, f64),
            (String, Token),
            (Token, Token),
            (Asset, Token),
            (PathExpression, Token),
            (Vec2d, [f64; 2]),
            (Vec3d, [f64; 3]),
            (Vec4d, [f64; 4]),
            (Vec2f, [f32; 2]),
            (Vec3f, [f32; 3]),
            (Vec4f, [f32; 4]),
            (Vec2h, [Half; 2]),
            (Vec3h, [Half; 3]),
            (Vec4h, [Half; 4]),
            (Vec2i, [i32; 2]),
            (Vec3i, [i32; 3]),
            (Vec4i, [i32; 4]),
            (Quatd, [f64; 4]),
            (Quatf, [f32; 4]),
            (Quath, [Half; 4]),
            (Matrix2d, [f64; 4]),
            (Matrix3d, [f64; 9]),
            (Matrix4d, [f64; 16]),
        }
    };
}
pub(crate) use attribute_kinds;

macro_rules! define_array {
    ($(($name:ident, $element:ty)),* $(,)?) => {
        /// An array attribute value: one shared column of elements.
        #[allow(missing_docs)]
        #[derive(Clone, Debug, PartialEq)]
        pub enum Array {
            $($name(Arc<Vec<$element>>),)*
        }

        impl Array {
            /// The kind of the elements.
            pub fn kind(&self) -> ValueKind {
                match self {
                    $(Array::$name(_) => ValueKind::$name,)*
                }
            }

            /// Elements in the array.
            pub fn len(&self) -> usize {
                match self {
                    $(Array::$name(items) => items.len(),)*
                }
            }

            /// Whether the array has no elements.
            pub fn is_empty(&self) -> bool {
                self.len() == 0
            }

            /// An empty array of `kind`, when it is an attribute value kind.
            pub fn empty(kind: ValueKind) -> Option<Array> {
                match kind {
                    $(ValueKind::$name => Some(Array::$name(Arc::new(Vec::new()))),)*
                    _ => None,
                }
            }

            /// The address of the shared column: the array's identity while
            /// it is held, so writers write a shared column once.
            pub(crate) fn identity(&self) -> usize {
                match self {
                    $(Array::$name(items) => Arc::as_ptr(items) as *const () as usize,)*
                }
            }

            /// Element `index` as a scalar value.
            pub fn get(&self, index: usize) -> Option<Value> {
                match self {
                    $(Array::$name(items) => items.get(index).map(|item| Value::$name(item.clone())),)*
                }
            }
        }

        /// A value of an attribute value kind, scalar.
        impl Value {
            /// The attribute value kind of a scalar or array attribute value.
            pub fn attribute_kind(&self) -> Option<ValueKind> {
                match self {
                    $(Value::$name(_) => Some(ValueKind::$name),)*
                    Value::Array(array) => Some(array.kind()),
                    _ => None,
                }
            }
        }
    };
}
attribute_kinds!(define_array);

/// `SdfSpecifier`.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Specifier {
    Def = 0,
    Over = 1,
    Class = 2,
}

/// `SdfPermission`.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Permission {
    Public = 0,
    Private = 1,
}

/// `SdfVariability`.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Variability {
    Varying = 0,
    Uniform = 1,
}

/// A time offset and scale applied to a referenced layer (`SdfLayerOffset`):
/// stage time = offset + scale * layer time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerOffset {
    /// The offset in time codes.
    pub offset: f64,
    /// The scale.
    pub scale: f64,
}

impl Default for LayerOffset {
    fn default() -> LayerOffset {
        LayerOffset::IDENTITY
    }
}

impl LayerOffset {
    /// No offset, unit scale.
    pub const IDENTITY: LayerOffset = LayerOffset {
        offset: 0.0,
        scale: 1.0,
    };

    /// Whether the offset changes nothing.
    pub fn is_identity(&self) -> bool {
        self.offset == 0.0 && self.scale == 1.0
    }

    /// `time` mapped through the offset.
    pub fn apply(&self, time: f64) -> f64 {
        self.offset + self.scale * time
    }

    /// `self` after `inner`: mapping through `inner`, then through `self`.
    pub fn concatenate(&self, inner: &LayerOffset) -> LayerOffset {
        LayerOffset {
            offset: self.offset + self.scale * inner.offset,
            scale: self.scale * inner.scale,
        }
    }

    /// The inverse mapping.
    pub fn inverse(&self) -> LayerOffset {
        if self.is_identity() {
            return *self;
        }
        let scale = if self.scale != 0.0 {
            1.0 / self.scale
        } else {
            f64::INFINITY
        };
        LayerOffset {
            offset: -self.offset * scale,
            scale,
        }
    }
}

/// Equal offsets hash alike (`-0.0` as `0.0`), so list ops find them by
/// their hash.
impl Hash for LayerOffset {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for number in [self.offset, self.scale] {
            (number + 0.0).to_bits().hash(state);
        }
    }
}

/// A dictionary: keys in byte order (OpenUSD's `VtDictionary` is a std::map).
pub type Dictionary = BTreeMap<Token, Value>;

/// One reference (`SdfReference`).
#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    /// The layer referenced; empty for an internal reference.
    pub asset: Token,
    /// The prim referenced; empty for the layer's default prim.
    pub prim: Path,
    /// The time mapping into the referenced layer.
    pub offset: LayerOffset,
    /// The reference's custom data.
    pub custom_data: Dictionary,
}

/// A reference hashes what names its target: its custom data is compared,
/// not hashed.
impl Hash for Reference {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (&self.asset, &self.prim, &self.offset).hash(state);
    }
}

/// One payload (`SdfPayload`).
#[derive(Clone, Debug, PartialEq, Hash)]
pub struct Payload {
    /// The layer the payload loads; empty for an internal payload.
    pub asset: Token,
    /// The prim; empty for the layer's default prim.
    pub prim: Path,
    /// The time mapping into the payload layer.
    pub offset: LayerOffset,
}

/// Time samples: ascending times, each with its value.
pub type TimeSamples = Vec<(f64, Value)>;

/// A field value.
#[allow(missing_docs)]
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    UChar(u8),
    Int(i32),
    UInt(u32),
    Int64(i64),
    UInt64(u64),
    Half(Half),
    Float(f32),
    Double(f64),
    TimeCode(f64),
    String(Token),
    Token(Token),
    Asset(Token),
    PathExpression(Token),
    Vec2d([f64; 2]),
    Vec3d([f64; 3]),
    Vec4d([f64; 4]),
    Vec2f([f32; 2]),
    Vec3f([f32; 3]),
    Vec4f([f32; 4]),
    Vec2h([Half; 2]),
    Vec3h([Half; 3]),
    Vec4h([Half; 4]),
    Vec2i([i32; 2]),
    Vec3i([i32; 3]),
    Vec4i([i32; 4]),
    /// `i, j, k, real`.
    Quatd([f64; 4]),
    /// `i, j, k, real`.
    Quatf([f32; 4]),
    /// `i, j, k, real`.
    Quath([Half; 4]),
    Matrix2d([f64; 4]),
    Matrix3d([f64; 9]),
    Matrix4d([f64; 16]),
    /// An array attribute value.
    Array(Array),
    Dictionary(Arc<Dictionary>),
    TokenListOp(Arc<ListOp<Token>>),
    StringListOp(Arc<ListOp<Token>>),
    PathListOp(Arc<ListOp<Path>>),
    ReferenceListOp(Arc<ListOp<Reference>>),
    PayloadListOp(Arc<ListOp<Payload>>),
    IntListOp(Arc<ListOp<i32>>),
    Int64ListOp(Arc<ListOp<i64>>),
    UIntListOp(Arc<ListOp<u32>>),
    UInt64ListOp(Arc<ListOp<u64>>),
    /// A list op of an unregistered field: each item its text.
    UnregisteredListOp(Arc<ListOp<Token>>),
    PathVector(Arc<Vec<Path>>),
    TokenVector(Arc<Vec<Token>>),
    StringVector(Arc<Vec<Token>>),
    DoubleVector(Arc<Vec<f64>>),
    LayerOffsetVector(Arc<Vec<LayerOffset>>),
    Specifier(Specifier),
    Permission(Permission),
    Variability(Variability),
    /// Variant selections: variant set name to selection, by set name.
    VariantSelections(Arc<BTreeMap<Token, Token>>),
    TimeSamples(Arc<TimeSamples>),
    Payload(Payload),
    /// `None`: an authored block of weaker opinions.
    ValueBlock,
    /// `AnimationBlock`: blocks weaker time samples.
    AnimationBlock,
    /// An unregistered field's value, kept as the text it was written as.
    Unregistered(Token),
    /// An unregistered field holding a dictionary.
    UnregisteredDictionary(Arc<Dictionary>),
    /// Relocates: source and target paths (an empty target deletes).
    Relocates(Arc<Vec<(Path, Path)>>),
    /// A single path: a relationship's default target, or an attribute
    /// authored as a path reference.
    Path(Path),
}

impl Value {
    /// The value's kind (crate type number); `Path` reports `PathVector`.
    pub fn kind(&self) -> ValueKind {
        use ValueKind as K;
        if let Some(kind) = self.attribute_kind() {
            return kind;
        }
        match self {
            Value::Dictionary(_) => K::Dictionary,
            Value::TokenListOp(_) => K::TokenListOp,
            Value::StringListOp(_) => K::StringListOp,
            Value::PathListOp(_) => K::PathListOp,
            Value::ReferenceListOp(_) => K::ReferenceListOp,
            Value::PayloadListOp(_) => K::PayloadListOp,
            Value::IntListOp(_) => K::IntListOp,
            Value::Int64ListOp(_) => K::Int64ListOp,
            Value::UIntListOp(_) => K::UIntListOp,
            Value::UInt64ListOp(_) => K::UInt64ListOp,
            Value::UnregisteredListOp(_) => K::UnregisteredValueListOp,
            Value::PathVector(_) | Value::Path(_) => K::PathVector,
            Value::TokenVector(_) => K::TokenVector,
            Value::StringVector(_) => K::StringVector,
            Value::DoubleVector(_) => K::DoubleVector,
            Value::LayerOffsetVector(_) => K::LayerOffsetVector,
            Value::Specifier(_) => K::Specifier,
            Value::Permission(_) => K::Permission,
            Value::Variability(_) => K::Variability,
            Value::VariantSelections(_) => K::VariantSelectionMap,
            Value::TimeSamples(_) => K::TimeSamples,
            Value::Payload(_) => K::Payload,
            Value::ValueBlock => K::ValueBlock,
            Value::AnimationBlock => K::AnimationBlock,
            Value::Unregistered(_) | Value::UnregisteredDictionary(_) => K::UnregisteredValue,
            Value::Relocates(_) => K::Relocates,
            _ => K::Value,
        }
    }

    /// The token or string text of a token, string or asset value.
    pub fn as_token(&self) -> Option<&Token> {
        match self {
            Value::Token(text)
            | Value::String(text)
            | Value::Asset(text)
            | Value::PathExpression(text) => Some(text),
            _ => None,
        }
    }

    /// A bool value.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// A numeric scalar as f64 (integers, halves and floats convert).
    pub fn as_f64(&self) -> Option<f64> {
        Some(match self {
            Value::Double(value) | Value::TimeCode(value) => *value,
            Value::Float(value) => f64::from(*value),
            Value::Half(value) => value.to_f64(),
            Value::Int(value) => f64::from(*value),
            Value::UInt(value) => f64::from(*value),
            Value::UChar(value) => f64::from(*value),
            Value::Int64(value) => *value as f64,
            Value::UInt64(value) => *value as f64,
            _ => return None,
        })
    }

    /// A dictionary value's entries.
    pub fn as_dictionary(&self) -> Option<&Dictionary> {
        match self {
            Value::Dictionary(dictionary) | Value::UnregisteredDictionary(dictionary) => {
                Some(dictionary)
            }
            _ => None,
        }
    }

    /// The tokens of a token or string vector.
    pub fn as_tokens(&self) -> Option<&[Token]> {
        match self {
            Value::TokenVector(items) | Value::StringVector(items) => Some(items),
            _ => None,
        }
    }

    /// An array value.
    pub fn as_array(&self) -> Option<&Array> {
        match self {
            Value::Array(array) => Some(array),
            _ => None,
        }
    }

    /// A time samples value.
    pub fn as_time_samples(&self) -> Option<&TimeSamples> {
        match self {
            Value::TimeSamples(samples) => Some(samples),
            _ => None,
        }
    }

    /// A specifier value.
    pub fn as_specifier(&self) -> Option<Specifier> {
        match self {
            Value::Specifier(specifier) => Some(*specifier),
            _ => None,
        }
    }

    /// Whether this is a value block (`None`).
    pub fn is_block(&self) -> bool {
        matches!(self, Value::ValueBlock)
    }

    /// A token vector value.
    pub fn token_vector(items: Vec<Token>) -> Value {
        Value::TokenVector(Arc::new(items))
    }
}

impl From<Array> for Value {
    fn from(array: Array) -> Value {
        Value::Array(array)
    }
}
