// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Composition, after OpenUSD's Pcp (`pxr/usd/pcp`) and UsdStage's
//! flattening: which layers' opinions make up each prim of a stage, and in
//! what strength order (LIVRPS: local, inherits, variants, relocates,
//! references, payloads, specializes), and the composed stage written as one
//! layer.
//!
//! - `assets`: asset paths anchored and normalized, package identifiers
//! - `layers`: the layers a composition opens, each once
//! - `layer_stack`, `relocations`: layer stacks with offsets and relocations
//! - `expressions`: variable expressions in asset paths and selections
//! - `map`: maps between namespaces
//! - `graph`, `strength`, `tasks`: the node graph of a prim index, its
//!   strength order and the work queue that builds it
//! - `composer`, `arcs`, `classes`, `variants`: building prim indexes
//! - `names`, `instancing`, `session`: what an index composes, instances,
//!   and stage options
//! - `flatten`: the composed stage as one layer (`UsdStage::Flatten`)
//! - `stage`: the public [`Stage`]

mod arcs;
pub mod assets;
mod classes;
mod composer;
pub mod expressions;
mod flatten;
mod graph;
mod instancing;
mod layer_stack;
mod layers;
mod map;
mod names;
mod relocations;
mod session;
mod stage;
mod strength;
mod tasks;
mod variants;

pub use flatten::{FlattenOptions, Flattened, Instances};
pub use graph::{ArcType, Node, PrimIndex};
pub use map::MapFunction;
pub use stage::{Stage, StageOptions};

/// Composition errors, one line each (`PcpErrorBase` descriptions).
#[derive(Clone, Debug, Default)]
pub struct Problems {
    lines: Vec<String>,
}

impl Problems {
    /// Record a problem.
    pub fn add(&mut self, message: impl Into<String>) {
        if self.lines.len() < 10_000 {
            self.lines.push(message.into());
        }
    }

    /// The problems, in the order met.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }
}
