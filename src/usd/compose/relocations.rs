// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Relocations, after `Pcp_ComputeRelocationsForLayerStack` and
//! `_FilterRelocationsForPath` (`pxr/usd/pcp/layerStack.cpp`). A layer
//! stack's relocates come from its layers' `layerRelocates`, strongest layer
//! first for a source (prim-level relocates are ignored in USD mode).
//! Entries that are invalid or conflict (two sources with one target, a
//! target that is another's source or lies under one, a source under another
//! source) are dropped. Each entry keeps its incremental source (as
//! authored) and its origin: the source with any ancestor that is itself a
//! relocation's target mapped back to where that came from.

use super::Problems;
use super::arcs::ArcOptions;
use super::graph::{ArcType, PrimIndex};
use super::layer_stack::LayerStack;
use super::layers::Layers;
use super::map::{MapFunction, PathPairs, namespace_depth};
use super::tasks::Indexer;
use crate::usd::error::Result;
use crate::usd::sdf::{Path, Value, field};
use std::collections::{HashMap, HashSet};

/// One relocation: `source` moves to `target` (empty: to nowhere).
#[derive(Clone, Debug, PartialEq)]
pub struct Relocate {
    /// The incremental source, as authored.
    pub source: Path,
    /// The target (empty: deleted).
    pub target: Path,
    /// The source with relocated ancestors mapped back.
    pub origin: Path,
}

/// A layer stack's relocations, found by source and by target: each source
/// and each target (but the empty one) is one relocation's.
#[derive(Clone, Debug, Default)]
pub struct Relocates {
    all: Vec<Relocate>,
    by_source: HashMap<Path, usize>,
    by_target: HashMap<Path, usize>,
}

impl Relocates {
    fn new(all: Vec<Relocate>) -> Relocates {
        let at = |path: fn(&Relocate) -> &Path| {
            (all.iter().enumerate())
                .filter(|(_, r)| !path(r).is_empty())
                .map(|(k, r)| (path(r).clone(), k))
                .collect()
        };
        Relocates {
            by_source: at(|r| &r.source),
            by_target: at(|r| &r.target),
            all,
        }
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// Each, strongest layer's first for a source.
    pub fn iter(&self) -> std::slice::Iter<'_, Relocate> {
        self.all.iter()
    }

    /// The one moving `source`.
    pub fn of_source(&self, source: &Path) -> Option<&Relocate> {
        self.by_source.get(source).map(|&k| &self.all[k])
    }

    /// The one moving something to `target`.
    pub fn to_target(&self, target: &Path) -> Option<&Relocate> {
        self.by_target.get(target).map(|&k| &self.all[k])
    }
}

impl<'a> IntoIterator for &'a Relocates {
    type Item = &'a Relocate;
    type IntoIter = std::slice::Iter<'a, Relocate>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Build `stack.relocates` from its layers.
pub fn compute_relocations(layers: &Layers, stack: &mut LayerStack, problems: &mut Problems) {
    let mut all = Vec::new();
    let mut sources = HashSet::new();
    for &layer in &stack.layers {
        let Some(Value::Relocates(pairs)) = layers.field(layer, field::LAYER_RELOCATES) else {
            continue;
        };
        for (source, target) in pairs.iter() {
            if !valid_relocate(source, target) {
                problems.add("Invalid authored relocation");
                continue;
            }
            if sources.insert(source) {
                all.push(Relocate {
                    source: source.clone(),
                    target: target.clone(),
                    origin: source.clone(),
                });
            }
        }
    }
    if all.is_empty() {
        return;
    }
    let mut relocates = Relocates::new(without_conflicts(all, problems));
    let origins: Vec<Path> = (relocates.iter())
        .map(|r| origin_of(&relocates, &r.source, 0))
        .collect();
    for (relocate, origin) in relocates.all.iter_mut().zip(origins) {
        relocate.origin = origin;
    }
    stack.relocates = relocates;
}

/// `_IsValidRelocatesEntryForComposing`: absolute prim paths without variant
/// selections, a source that is not a root prim, a target (unless empty)
/// that is neither the source nor above or below it.
fn valid_relocate(source: &Path, target: &Path) -> bool {
    if !plain_prim(source) || source.depth() == 1 {
        return false;
    }
    if target.is_empty() {
        return true;
    }
    plain_prim(target)
        && source != target
        && !target.has_prefix(source)
        && !source.has_prefix(target)
}

fn plain_prim(path: &Path) -> bool {
    path.is_absolute() && path.is_prim() && !path.has_variant_selection()
}

/// `relocates` (one a source) less those that conflict
/// (`_ValidateAndRemoveConflictingRelocates`): two sources with one target,
/// a target that is another's source (both go), a target or a source under
/// another's source.
fn without_conflicts(relocates: Vec<Relocate>, problems: &mut Problems) -> Vec<Relocate> {
    let sources: HashMap<&Path, usize> = (relocates.iter().enumerate())
        .map(|(k, r)| (&r.source, k))
        .collect();
    let mut targets: HashMap<&Path, usize> = HashMap::new();
    for relocate in relocates.iter().filter(|r| !r.target.is_empty()) {
        *targets.entry(&relocate.target).or_default() += 1;
    }
    // Whether an ancestor of `path` above `floor` levels is a source.
    let under_source = |path: &Path, floor: u32| {
        let mut up = path.parent();
        while !up.is_empty() && !up.is_root() && up.depth() > floor {
            if sources.contains_key(&up) {
                return true;
            }
            up = up.parent();
        }
        false
    };
    let mut dropped = vec![false; relocates.len()];
    for (at, entry) in relocates.iter().enumerate() {
        if !entry.target.is_empty() {
            if targets[&entry.target] > 1 {
                dropped[at] = true;
            }
            if let Some(&other) = sources.get(&entry.target) {
                dropped[at] = true;
                dropped[other] = true;
            }
            dropped[at] |= under_source(&entry.target, 0);
        }
        dropped[at] |= under_source(&entry.source, 1);
    }
    let kept: Vec<Relocate> = (relocates.iter().zip(&dropped))
        .filter(|(_, dropped)| !**dropped)
        .map(|(r, _)| r.clone())
        .collect();
    for _ in kept.len()..relocates.len() {
        problems.add("Conflicting relocation");
    }
    kept
}

/// The source's origin: the nearest ancestor (or itself) another relocation
/// targets, replaced by that relocation's own origin.
fn origin_of(relocates: &Relocates, source: &Path, depth: usize) -> Path {
    if depth > 64 {
        return source.clone();
    }
    let mut up = source.clone();
    while !up.is_empty() && !up.is_root() {
        if let Some(found) = relocates.to_target(&up) {
            let origin = origin_of(relocates, &found.source, depth + 1);
            return source.replace_prefix(&up, &origin);
        }
        up = up.parent();
    }
    source.clone()
}

/// The relocations affecting namespace at and below `path`
/// (`_FilterRelocationsForPath`): origin to target, then incremental source
/// to target for targets not yet seen, and the root identity.
pub fn relocation_pairs(stack: &LayerStack, path: &Path) -> PathPairs {
    let mut pairs = PathPairs::empty(true);
    let mut seen = HashSet::new();
    for relocate in &stack.relocates {
        if relocate.target.is_empty() || !relocate.origin.has_prefix(path) {
            continue;
        }
        seen.insert(&relocate.target);
        pairs.sources.push(relocate.origin.clone());
        pairs.targets.push(relocate.target.clone());
    }
    for relocate in &stack.relocates {
        if relocate.target.is_empty()
            || !relocate.source.has_prefix(path)
            || seen.contains(&relocate.target)
        {
            continue;
        }
        pairs.sources.push(relocate.source.clone());
        pairs.targets.push(relocate.target.clone());
    }
    pairs
}

impl super::composer::Composer {
    /// A node at a relocation's target brings the relocated source
    /// (`_EvalNodeRelocations`): its other arcs are elided, and a relocate
    /// arc adds the source's index, which only holds opinions below it.
    pub(super) fn eval_relocations(&mut self, indexer: &mut Indexer<'_>, node: u32) -> Result<()> {
        if !indexer.index.node(node).contributes() {
            return Ok(());
        }
        let stack = indexer.index.node(node).stack;
        let Some(source) = self
            .stacks
            .stack(stack)
            .relocated_to(&indexer.index.node(node).path)
            .cloned()
        else {
            return Ok(());
        };
        for child in indexer.index.node(node).children.clone() {
            if indexer.index.node(child).arc != ArcType::Variant {
                indexer.index.elide_subtree(child);
            }
        }
        let options = ArcOptions {
            contribute: false,
            ancestral: true,
            ..ArcOptions::plain()
        };
        let depth = namespace_depth(&indexer.index.node(node).path);
        let added = self.add_arc(
            indexer,
            ArcType::Relocate,
            node,
            Some(node),
            stack,
            source,
            MapFunction::identity(),
            0,
            depth,
            options,
        )?;
        if let Some(relocated) = added {
            self.elide_relocated(&mut indexer.index, relocated, 0);
        }
        Ok(())
    }

    /// Elide the subtrees under `node` rooted at a relocation's source:
    /// their opinions moved with it.
    fn elide_relocated(&self, index: &mut PrimIndex, node: u32, depth: usize) {
        if depth > 1024 {
            return;
        }
        for child in index.node(node).children.clone() {
            if index.node(child).arc == ArcType::Relocate {
                continue;
            }
            let entry = index.node(child);
            if entry.contributes() && self.stacks.stack(entry.stack).relocates_source(&entry.path) {
                index.elide_subtree(child);
                continue;
            }
            self.elide_relocated(index, child, depth + 1);
        }
    }

    /// A relocation node implies its source to its grandparent, mapped
    /// across its parent's arc (`_EvalImpliedRelocations`).
    pub(super) fn eval_implied_relocations(
        &mut self,
        indexer: &mut Indexer<'_>,
        node: u32,
    ) -> Result<()> {
        let entry = indexer.index.node(node);
        if entry.arc != ArcType::Relocate || entry.due_to_ancestor {
            return Ok(());
        }
        let Some(parent) = entry.parent else {
            return Ok(());
        };
        let Some(grandparent) = indexer.index.node(parent).parent else {
            return Ok(());
        };
        let source = indexer
            .index
            .node(parent)
            .map_to_parent
            .map(&entry.path, false);
        if source.is_empty() {
            return Ok(());
        }
        if indexer
            .index
            .node(grandparent)
            .children
            .iter()
            .any(|&child| {
                indexer.index.node(child).path == source
                    && indexer.index.node(child).arc == ArcType::Relocate
            })
        {
            return Ok(());
        }
        let options = ArcOptions {
            contribute: false,
            ..ArcOptions::plain()
        };
        let stack = indexer.index.node(grandparent).stack;
        let depth = namespace_depth(&indexer.index.node(grandparent).path);
        self.add_arc(
            indexer,
            ArcType::Relocate,
            grandparent,
            Some(node),
            stack,
            source,
            MapFunction::identity(),
            0,
            depth,
            options,
        )?;
        Ok(())
    }

    /// Whether the index is of a prim a relocation moved away: a
    /// contributing node's site is a relocation's source.
    pub(super) fn prohibited(&self, index: &PrimIndex, node: u32) -> bool {
        let entry = index.node(node);
        if entry.culled {
            return false;
        }
        if !entry.inert && self.stacks.stack(entry.stack).relocates_source(&entry.path) {
            return true;
        }
        entry
            .children
            .iter()
            .any(|&child| self.prohibited(index, child))
    }
}
