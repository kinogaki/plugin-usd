// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Building prim indexes, after `Pcp_BuildPrimIndex` and
//! `PcpComputePrimIndex` (`pxr/usd/pcp/primIndex.cpp`). A prim's index
//! starts as its parent's with every site extended by the prim's name
//! (ancestral opinions); a variant selection's starts as its one site. Then
//! tasks run in priority order (`tasks.rs`): each node's references,
//! payloads, inherits, specializes and variant sets, and the variants those
//! select. An arc to a prim that is not a root prim brings that prim's own
//! ancestral opinions, so its target's index is built first (one stack frame
//! down, variants left for the outer index to choose) and added as a
//! subtree. The finished index culls subtrees with no opinions and is stored
//! in strength order.

use std::collections::HashMap;
use std::sync::Arc;

use super::Problems;
use super::graph::{ArcType, PrimIndex};
use super::layer_stack::Stacks;
use super::layers::Layers;
use super::map::{MapFunction, namespace_depth};
use super::tasks::{Frame, Indexer, kind};
use crate::usd::error::{Error, Result};
use crate::usd::sdf::{Path, PathKind, PathMap, SpecType, Token, field};

/// The identifier of a stage's anonymous session layer (`UsdStage::Open`
/// makes one).
pub const ANONYMOUS_SESSION: &str = "anon:session.usda";

/// The most prim indexes built in one composition before giving up, and
/// the deepest nesting of arc builds.
const MOST_NESTED_BUILDS: usize = 512;

/// The most nodes one prim index holds. Arcs that meet again (a diamond of
/// references) each bring their own nodes, as in Pcp, so a few levels of
/// them double the nodes level by level; past this many the index fails.
const MOST_NODES: usize = 1 << 16;

/// A composition: the layers and layer stacks it opened, its root layer
/// stack, and the indexes of the root stack's prims (each once).
pub struct Composer {
    /// Every layer opened.
    pub layers: Layers,
    /// Every layer stack built.
    pub stacks: Stacks,
    /// Composition errors.
    pub problems: Problems,
    /// The root layer stack.
    pub root_stack: usize,
    /// Path texts whose prims' payloads are not loaded (a prefix each).
    pub(super) unloaded: Vec<String>,
    /// Globs one of which a prim's path must match for its payloads to load.
    pub(super) loaded_matching: Vec<String>,
    /// Variant fallbacks: (set, selection), preferred first.
    pub(super) variant_fallbacks: Vec<(Token, Token)>,
    /// The prim whose index is being built (payload loading asks about it).
    pub(super) composing: Path,
    cache: PathMap<Arc<PrimIndex>>,
    /// Each arc's map, made once ([`Composer::arc_map`]).
    pub(super) arc_maps: HashMap<ArcKey, MapFunction>,
    /// Nested builds under way, against runaway recursion.
    nesting: usize,
}

/// What an arc's map is made of: the layer stack whose relocations it goes
/// through, its source and target paths, its time offset (by bits), and
/// whether every other path maps to itself.
pub(super) type ArcKey = (usize, Path, Path, [u64; 2], bool);

impl Composer {
    /// A composition of the layer stack rooted at `root` (an identifier),
    /// with `session` (an identifier, [`ANONYMOUS_SESSION`], or none) before
    /// it, its layers read through `layers`.
    pub fn new(mut layers: Layers, root: &str, session: Option<&str>) -> Result<Composer> {
        let mut stacks = Stacks::default();
        let mut problems = Problems::default();
        let Some(root_layer) = layers.find_or_open(root)? else {
            return Err(Error::invalid(format!(
                "cannot read the root layer {root}: {}",
                layers.problem(root)
            ))
            .in_file(root));
        };
        let session_layer = match session {
            // UsdStage::Open's empty session layer: no opinions, but the
            // root layer stack differs from its root layer referenced alone.
            Some(ANONYMOUS_SESSION) => Some(layers.anonymous(ANONYMOUS_SESSION)),
            Some(identifier) => match layers.find_or_open(identifier)? {
                Some(layer) => Some(layer),
                None => {
                    return Err(Error::invalid(format!(
                        "cannot read the session layer {identifier}: {}",
                        layers.problem(identifier)
                    )));
                }
            },
            None => None,
        };
        let root_stack =
            stacks.find_or_build(&mut layers, root_layer, session_layer, None, &mut problems)?;
        Ok(Composer {
            layers,
            stacks,
            problems,
            root_stack,
            unloaded: Vec::new(),
            loaded_matching: Vec::new(),
            variant_fallbacks: Vec::new(),
            composing: Path::absolute_root(),
            cache: PathMap::default(),
            arc_maps: HashMap::new(),
            nesting: 0,
        })
    }

    /// `Err` of kind [`Cancelled`](crate::usd::ErrorKind::Cancelled) once the
    /// stage is no longer wanted.
    pub(super) fn check(&self) -> Result<()> {
        Ok(self.layers.budget().check()?)
    }

    /// `Err` once `index` holds more nodes than one may.
    pub(super) fn admit(index: &PrimIndex) -> Result<()> {
        match index.nodes.len() <= MOST_NODES {
            true => Ok(()),
            false => Err(Error::limit(format!(
                "a prim index of more than {MOST_NODES} nodes"
            ))),
        }
    }

    /// The root layer's index.
    pub fn root_layer_index(&self) -> usize {
        self.stacks.stack(self.root_stack).root
    }

    /// The index of prim `path` of the root layer stack (built once):
    /// culled and stored in strength order (`PcpComputePrimIndex`).
    pub fn index(&mut self, path: &Path) -> Result<Arc<PrimIndex>> {
        if let Some(found) = self.cache.get(path) {
            return Ok(found.clone());
        }
        // Build uncached ancestors first, root side first, so deep
        // namespaces do not recurse.
        let missing: Vec<Path> = path
            .ancestors()
            .skip(1)
            .take_while(|ancestor| !self.cache.contains_key(ancestor))
            .collect();
        for ancestor in missing.iter().rev() {
            self.index(ancestor)?;
        }
        let outer = std::mem::replace(&mut self.composing, path.clone());
        let built = self.build(self.root_stack, path, None, true, true);
        self.composing = outer;
        let mut built = built?;
        cull(&mut built, self.root_stack);
        built.instanceable = self.is_instanceable(&built);
        let made = Arc::new(finalized(&built));
        let size = size_of::<super::graph::Node>() * made.nodes.len();
        self.layers.budget().allocate(size as u64)?;
        self.cache.insert(path.clone(), made.clone());
        Ok(made)
    }

    /// The index of `path` if it was built.
    pub(super) fn cached_index(&self, path: &Path) -> Option<&Arc<PrimIndex>> {
        self.cache.get(path)
    }

    /// The index of `path` in layer stack `stack` (`Pcp_BuildPrimIndex`).
    /// `outer`: an outermost build, which evaluates variant sets and
    /// implied specializes.
    pub(super) fn build(
        &mut self,
        stack: usize,
        path: &Path,
        frame: Option<&Frame<'_>>,
        contribute_root: bool,
        outer: bool,
    ) -> Result<PrimIndex> {
        if path.is_absolute_root() {
            let mut index = PrimIndex::rooted(stack, path.clone());
            index.nodes[0].has_specs = self.site_has_specs(stack, path);
            return Ok(index);
        }
        self.nesting += 1;
        if self.nesting > MOST_NESTED_BUILDS {
            self.nesting -= 1;
            return Err(Error::limit("composition arcs nest too deeply"));
        }
        let result = self.build_nested(stack, path, frame, contribute_root, outer);
        self.nesting -= 1;
        result
    }

    fn build_nested(
        &mut self,
        stack: usize,
        path: &Path,
        frame: Option<&Frame<'_>>,
        contribute_root: bool,
        outer: bool,
    ) -> Result<PrimIndex> {
        let start = if path.kind() == PathKind::Variant {
            // A variant selection's ancestral opinions come with its prim's.
            let mut index = PrimIndex::rooted(stack, path.clone());
            index.nodes[0].has_specs = self.site_has_specs(stack, path);
            index
        } else {
            self.ancestral(stack, path, frame, outer)?
        };
        let mut indexer = Indexer::new(start, frame, outer);
        if !contribute_root {
            indexer.index.set_inert(0);
        }
        // A prim a relocation moved away has no opinions here.
        if path.kind() != PathKind::Variant && self.prohibited(&indexer.index, 0) {
            indexer.index.set_inert(0);
            for child in indexer.index.nodes[0].children.clone() {
                indexer.index.elide_subtree(child);
            }
            return Ok(indexer.index);
        }
        indexer.add_tasks(0, kind::ALL & !kind::ANCESTRAL_VARIANT_SETS);
        self.run_tasks(&mut indexer)?;
        Ok(indexer.index)
    }

    /// The parent's index with every site extended by the prim's name.
    fn ancestral(
        &mut self,
        stack: usize,
        path: &Path,
        frame: Option<&Frame<'_>>,
        outer: bool,
    ) -> Result<PrimIndex> {
        let parent_path = path.parent();
        let (mut index, instance) = if frame.is_none() && stack == self.root_stack {
            let cached = self.index(&parent_path)?;
            let instance = cached.instanceable;
            ((*cached).clone(), instance)
        } else {
            let index = self.build(stack, &parent_path, frame, true, outer)?;
            let instance = self.ancestor_is_instance(stack, &parent_path, &index);
            (index, instance)
        };
        // Below an instance only its instanceable opinions count.
        if instance {
            super::instancing::disable_non_instanceable(&mut index);
        }
        index.has_payloads = false;
        let name = path.name_token();
        for at in 0..index.nodes.len() {
            let node = &mut index.nodes[at];
            node.path = if node.path == parent_path {
                path.clone()
            } else {
                node.path.child(name.clone())
            };
            if at > 0 {
                // Every node but the root now comes from an ancestor.
                node.due_to_ancestor = true;
                node.direct = false;
                node.ancestral_arc = true;
            }
        }
        for at in 0..index.nodes.len() {
            if index.nodes[at].has_specs {
                let (stack, path) = (index.nodes[at].stack, index.nodes[at].path.clone());
                index.nodes[at].has_specs = self.site_has_specs(stack, &path);
            }
        }
        Ok(index)
    }

    /// Whether the stage is its root layer alone: no sublayers, session
    /// layer with opinions or composition arcs, so the layer's specs are the
    /// stage's.
    pub fn is_single_layer(&self) -> bool {
        let stack = self.stacks.stack(self.root_stack);
        let mut count = stack.layers.len();
        if let Some(session) = stack.session {
            if self.layers.layer(session).len() > 1 {
                return false;
            }
            count -= 1;
        }
        if count != 1 {
            return false;
        }
        let arcs = [
            field::REFERENCES,
            field::PAYLOAD,
            field::INHERIT_PATHS,
            field::SPECIALIZES,
            field::VARIANT_SET_NAMES,
            field::RELOCATES,
            field::INSTANCEABLE,
        ];
        !self.layers.layer(stack.root).iter().any(|(_, spec)| {
            spec.kind == SpecType::Prim
                && spec
                    .fields
                    .iter()
                    .any(|(name, _)| arcs.contains(&name.as_str()))
        })
    }

    /// Whether any layer of `stack` has a spec at `path`.
    pub fn site_has_specs(&self, stack: usize, path: &Path) -> bool {
        self.stacks
            .stack(stack)
            .layers
            .iter()
            .any(|&layer| self.layers.has_spec(layer, path))
    }

    /// Run the queue's tasks until none is left; tasks add nodes, and the
    /// nodes' own tasks.
    fn run_tasks(&mut self, indexer: &mut Indexer<'_>) -> Result<()> {
        while let Some(task) = indexer.pop() {
            self.check()?;
            match task.kind {
                kind::RELOCATIONS => self.eval_relocations(indexer, task.node)?,
                kind::IMPLIED_RELOCATIONS => self.eval_implied_relocations(indexer, task.node)?,
                kind::REFERENCES => self.eval_arcs(indexer, task.node, false)?,
                kind::PAYLOADS => self.eval_arcs(indexer, task.node, true)?,
                kind::INHERITS => self.eval_class_arcs(indexer, task.node, ArcType::Inherit)?,
                kind::SPECIALIZES => {
                    self.eval_class_arcs(indexer, task.node, ArcType::Specialize)?
                }
                kind::IMPLIED_CLASSES => self.eval_implied_classes(indexer, task.node)?,
                kind::IMPLIED_SPECIALIZES => self.eval_implied_specializes(indexer, task.node)?,
                kind::VARIANT_SETS => self.eval_variant_sets(indexer, task.node),
                kind::ANCESTRAL_VARIANT_SETS => {
                    self.eval_ancestral_variant_sets(indexer, task.node)
                }
                kind::VARIANT_AUTHORED | kind::ANCESTRAL_VARIANT_AUTHORED => {
                    let ancestral = task.kind == kind::ANCESTRAL_VARIANT_AUTHORED;
                    self.eval_authored_variant(indexer, task, ancestral)?;
                }
                kind::VARIANT_FALLBACK | kind::ANCESTRAL_VARIANT_FALLBACK => {
                    let ancestral = task.kind == kind::ANCESTRAL_VARIANT_FALLBACK;
                    self.eval_fallback_variant(indexer, task, ancestral)?;
                }
                // None-found tasks only wait for retry_variants.
                _ => {}
            }
        }
        Ok(())
    }
}

/// `index` finalized (`PcpPrimIndex_Graph::Finalize`): nodes renumbered in
/// strength order, and culled nodes left out unless a kept node's chain of
/// origins passes through them.
fn finalized(index: &PrimIndex) -> PrimIndex {
    let order = index.order(false);
    let mut erasable: Vec<bool> = index.nodes.iter().map(|node| node.culled).collect();
    for at in 0..index.nodes.len() {
        if index.nodes[at].origin.is_some() {
            keep_origin_chain(index, &mut erasable, at as u32);
        }
    }
    let mut mapping = vec![None; index.nodes.len()];
    let mut kept = 0u32;
    for &node in &order {
        if !erasable[node as usize] {
            mapping[node as usize] = Some(kept);
            kept += 1;
        }
    }
    let mut made = PrimIndex {
        nodes: Vec::with_capacity(kept as usize),
        ..PrimIndex::default()
    };
    for &node in &order {
        if mapping[node as usize].is_none() {
            continue;
        }
        let mut copied = index.node(node).clone();
        copied.children = copied
            .children
            .iter()
            .filter_map(|&child| mapping[child as usize])
            .collect();
        copied.parent = copied.parent.and_then(|parent| mapping[parent as usize]);
        copied.origin = copied.origin.and_then(|origin| mapping[origin as usize]);
        made.nodes.push(copied);
    }
    made.instanceable = index.instanceable;
    made.has_payloads = index.has_payloads;
    made
}

/// Culled nodes on a kept node's chain of origins, and their parents, stay.
fn keep_origin_chain(index: &PrimIndex, erasable: &mut [bool], start: u32) {
    let mut node = start;
    let mut subsequent = false;
    let mut steps = 0;
    loop {
        steps += 1;
        if steps > index.nodes.len() {
            return;
        }
        let entry = index.node(node);
        if !erasable[node as usize] {
            subsequent = true;
        } else if subsequent {
            let mut at = Some(node);
            while let Some(index_at) = at {
                if !erasable[index_at as usize] {
                    break;
                }
                erasable[index_at as usize] = false;
                if index_at != node {
                    keep_origin_chain(index, erasable, index_at);
                }
                at = index.node(index_at).parent;
            }
        }
        match entry.origin {
            Some(origin) if Some(origin) != entry.parent => node = origin,
            _ => return,
        }
    }
}

/// Mark subtrees without opinions culled (`_CullSubtreesWithNoOpinions`).
fn cull(index: &mut PrimIndex, root_stack: usize) {
    if index.nodes.is_empty() {
        return;
    }
    for child in index.nodes[0].children.clone() {
        cull_subtree(index, child, root_stack);
    }
}

fn cull_subtree(index: &mut PrimIndex, node: u32, root_stack: usize) {
    for child in index.node(node).children.clone() {
        cull_subtree(index, child, root_stack);
    }
    if can_cull(index, node, root_stack) {
        index.set_culled(node);
    }
}

/// `_NodeCanBeCulled`: a node stays when it is the root, when its arc was
/// introduced at its own namespace depth, when it is a subroot inherit in
/// the root layer stack, when a child stays, or when it contributes specs.
fn can_cull(index: &PrimIndex, node: u32, root_stack: usize) -> bool {
    let entry = index.node(node);
    if entry.culled {
        return true;
    }
    let Some(parent) = entry.parent else {
        return false;
    };
    if entry.depth == namespace_depth(&index.node(parent).path) {
        return false;
    }
    if entry.arc == ArcType::Inherit && entry.stack == root_stack {
        // A subroot class exists in the composed scene; keep its node.
        let origin = if entry.origin == entry.parent {
            node
        } else {
            index.origin_root(node)
        };
        let intro = index.path_at_introduction(origin);
        if !(intro.depth() == 1 && intro.is_prim()) {
            return false;
        }
    }
    if entry
        .children
        .iter()
        .any(|&child| !index.node(child).culled)
    {
        return false;
    }
    !(entry.has_specs && entry.contributes())
}
