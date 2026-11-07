// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The scene description data model, after OpenUSD's Sdf: tokens, paths,
//! values, list ops and layers of specs.

mod half;
pub mod identifier;
mod identifier_tables;
mod layer;
mod list_op;
mod path;
pub mod schema;
mod token;
mod value;
mod value_type;

pub use half::Half;
pub use layer::{LayerData, Spec, SpecType, field};
pub use list_op::{ListOp, ListPart};
pub use path::{Path, PathHasher, PathKind, PathMap};
pub use token::Token;
pub(crate) use value::attribute_kinds;
pub use value::{
    Array, Dictionary, LayerOffset, Payload, Permission, Reference, Specifier, TimeSamples, Value,
    Variability,
};
pub use value_type::{Role, TypeName, ValueKind, find_type, serialization_name};
