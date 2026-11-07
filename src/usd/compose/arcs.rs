// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! References and payloads, after `_EvalRefOrPayloadArcs` and `_AddArc`
//! (`pxr/usd/pcp/primIndex.cpp`) and `PcpComposeSiteReferences` /
//! `Payloads` (`composeSite.cpp`). A node's arcs are its layer stack's list
//! ops at its path applied weakest layer first, each item's asset path
//! anchored at the layer that authored it, and each remembering that layer
//! for its time offset. An arc targets a prim of the referenced layer's own
//! layer stack (the node's own, for internal references), or its
//! defaultPrim.

use super::assets;
use super::composer::Composer;
use super::expressions::{evaluate_to_string, is_expression};
use super::graph::{ArcType, Node, PrimIndex};
use super::layer_stack::valid_offset;
use super::map::{MapFunction, namespace_depth};
use super::relocations::relocation_pairs;
use super::tasks::{Frame, Indexer, kind};
use crate::usd::error::{Error, Result};
use crate::usd::sdf::{LayerOffset, ListOp, Path, Value, field};

/// How `_AddArc` adds a node (`_ArcOptions`).
#[derive(Clone, Copy)]
pub(super) struct ArcOptions {
    /// Whether the new site contributes specs.
    pub contribute: bool,
    /// Whether its ancestral opinions come along (its index is built and
    /// added as a subtree).
    pub ancestral: bool,
    /// Whether a site some graph up the stack frames already holds is skipped.
    pub skip_duplicates: bool,
    /// Whether the new node takes its origin's dependency flags.
    pub copy_ancestor: bool,
    /// The tasks queued for the new nodes.
    pub tasks: u32,
}

impl ArcOptions {
    pub fn plain() -> ArcOptions {
        ArcOptions {
            contribute: true,
            ancestral: false,
            skip_duplicates: false,
            copy_ancestor: false,
            tasks: kind::ALL,
        }
    }
}

/// One composed reference or payload: the asset (anchored identifier), the
/// prim, its offset, and the position in the layer stack of the layer that
/// authored it.
#[derive(Clone, Debug, PartialEq)]
struct ArcItem {
    asset: String,
    prim: Path,
    offset: LayerOffset,
    layer: usize,
}

impl ArcItem {
    fn same(&self, other: &ArcItem) -> bool {
        self.asset == other.asset && self.prim == other.prim && self.offset == other.offset
    }
}

/// A layer's defaultPrim as a path: a root prim's name, or a prim path.
fn default_prim_path(text: &str) -> Option<Path> {
    if text.starts_with('/') {
        return Path::parse(text).ok();
    }
    if text.contains('/') {
        return Path::parse(text)
            .ok()?
            .make_absolute(&Path::absolute_root())
            .ok();
    }
    if text.is_empty() {
        return None;
    }
    Some(Path::absolute_root().child(text))
}

/// Whether a path is a root prim's.
pub(super) fn is_root_prim(path: &Path) -> bool {
    path.depth() == 1 && path.is_prim()
}

impl Composer {
    /// Add the references (or payloads) of `node`.
    pub(super) fn eval_arcs(
        &mut self,
        indexer: &mut Indexer<'_>,
        node: u32,
        payloads: bool,
    ) -> Result<()> {
        if !indexer.index.node(node).contributes() {
            return Ok(());
        }
        let stack = indexer.index.node(node).stack;
        let path = indexer.index.node(node).path.clone();
        let arcs = self.compose_arcs(stack, &path, payloads);
        if payloads && !arcs.is_empty() {
            indexer.index.has_payloads = true;
            if !self.loads_payload(indexer, node) {
                return Ok(());
            }
        }
        for (number, arc) in arcs.into_iter().enumerate() {
            self.add_ref_arc(indexer, node, arc, number as i32, payloads)?;
        }
        Ok(())
    }

    /// Whether the node's payloads load: its path mapped to the prim being
    /// indexed, across stack frames; a path that does not map there always
    /// loads.
    fn loads_payload(&self, indexer: &Indexer<'_>, node: u32) -> bool {
        if self.loads_all_payloads() {
            return true;
        }
        let mut path = inclusion_path(
            &indexer.index,
            node,
            &indexer.index.node(node).path.strip_variants(),
        );
        let mut frame = indexer.frame;
        while !path.is_empty() {
            let Some(up) = frame else { break };
            path = to_parent_inclusion(up.map, up.kind, &path);
            if path.is_empty() {
                break;
            }
            path = inclusion_path(up.index, up.parent, &path);
            frame = up.previous;
        }
        path.is_empty() || self.payload_included(&path)
    }

    /// The site's reference (payload) list ops applied weakest layer first.
    fn compose_arcs(&self, stack: usize, path: &Path, payloads: bool) -> Vec<ArcItem> {
        let mut out: Vec<ArcItem> = Vec::new();
        let layers = &self.stacks.stack(stack).layers;
        for position in (0..layers.len()).rev() {
            let layer = layers[position];
            let Some(spec) = self.layers.layer(layer).spec(path) else {
                continue;
            };
            let name = if payloads {
                field::PAYLOAD
            } else {
                field::REFERENCES
            };
            let items: ListOp<(String, Path, LayerOffset)> = match spec.get(name) {
                Some(Value::ReferenceListOp(list)) => map_list(list, |item| {
                    (item.asset.to_string(), item.prim.clone(), item.offset)
                }),
                Some(Value::PayloadListOp(list)) => map_list(list, |item| {
                    (item.asset.to_string(), item.prim.clone(), item.offset)
                }),
                _ => continue,
            };
            self.apply_arcs(&items, stack, layer, position, &mut out);
        }
        out
    }

    /// Apply one layer's list op, anchoring asset paths first and tagging
    /// new items with the layer; an item whose asset expression evaluates
    /// to nothing is left out.
    fn apply_arcs(
        &self,
        list: &ListOp<(String, Path, LayerOffset)>,
        stack: usize,
        layer: usize,
        position: usize,
        out: &mut Vec<ArcItem>,
    ) {
        let anchor = |item: &(String, Path, LayerOffset)| -> Option<ArcItem> {
            let mut asset = item.0.clone();
            if is_expression(&asset) {
                asset =
                    evaluate_to_string(&asset, &self.stacks.stack(stack).variables)?.to_string();
            }
            let asset = if asset.is_empty() {
                String::new()
            } else {
                assets::anchored(&asset, self.layers.identifier(layer))
            };
            Some(ArcItem {
                asset,
                prim: item.1.clone(),
                offset: item.2,
                layer: position,
            })
        };
        let find =
            |out: &Vec<ArcItem>, item: &ArcItem| out.iter().position(|known| known.same(item));
        if list.explicit {
            out.clear();
            for item in list.explicit_items.iter().filter_map(anchor) {
                if find(out, &item).is_none() {
                    out.push(item);
                }
            }
            return;
        }
        for item in list.deleted.iter().filter_map(anchor) {
            if let Some(at) = find(out, &item) {
                out.remove(at);
            }
        }
        for item in list.added.iter().filter_map(anchor) {
            if find(out, &item).is_none() {
                out.push(item);
            }
        }
        for item in list.prepended.iter().rev().filter_map(anchor) {
            if let Some(at) = find(out, &item) {
                out.remove(at);
            }
            out.insert(0, item);
        }
        for item in list.appended.iter().filter_map(anchor) {
            if let Some(at) = find(out, &item) {
                out.remove(at);
            }
            out.push(item);
        }
    }

    /// One reference or payload arc from `node`.
    fn add_ref_arc(
        &mut self,
        indexer: &mut Indexer<'_>,
        node: u32,
        arc: ArcItem,
        number: i32,
        payload: bool,
    ) -> Result<()> {
        let stack = indexer.index.node(node).stack;
        let node_path = indexer.index.node(node).path.clone();
        let mut prim = arc.prim.clone();
        if !prim.is_empty()
            && !(prim.is_absolute() && prim.is_prim() && !prim.has_variant_selection())
        {
            self.problems.add("Invalid reference or payload prim path");
            return Ok(());
        }
        let internal = arc.asset.is_empty();
        let mut offset = arc.offset;
        if !valid_offset(&offset) {
            offset = LayerOffset::IDENTITY;
        } else if !internal {
            offset = self.stacks.stack(stack).offsets[arc.layer].concatenate(&offset);
        }
        let mut target_stack = stack;
        let mut target_layer = self.stacks.stack(stack).root;
        if !internal && super::composer::is_foreign(&arc.asset) {
            let foreign = super::composer::ForeignArc {
                prim: self.composing.clone(),
                asset: arc.asset.clone(),
                target: prim,
            };
            if !self.foreign.contains(&foreign) {
                self.foreign.push(foreign);
            }
            return Ok(());
        }
        if !internal {
            let Some(opened) = self.layers.find_or_open(&arc.asset)? else {
                self.problems
                    .add(format!("Could not open asset @{}@", arc.asset));
                return Ok(());
            };
            target_layer = opened;
            // The referenced stack's expression variables are overridden by
            // the referencing stack's.
            let overrides = self.stacks.stack(stack).source;
            target_stack = self.stacks.find_or_build(
                &mut self.layers,
                target_layer,
                None,
                overrides,
                &mut self.problems,
            )?;
            let source_rate = self
                .layers
                .time_codes_per_second(self.stacks.stack(stack).layers[arc.layer]);
            let target_rate = self.stacks.stack(target_stack).time_codes_per_second;
            if source_rate != target_rate {
                offset.scale = offset.scale * source_rate / target_rate;
            }
        }
        let mut contribute = true;
        if prim.is_empty() {
            match self
                .layers
                .token_field(target_layer, field::DEFAULT_PRIM)
                .and_then(|name| default_prim_path(&name))
            {
                Some(path) => prim = path,
                None => {
                    self.problems.add("Unresolved defaultPrim");
                    prim = Path::absolute_root();
                    contribute = false;
                }
            }
        }
        let mut map = self.arc_map(stack, &prim, &node_path, offset);
        // An internal arc also maps every other path to itself.
        if internal {
            map = map.with_root_identity();
        }
        let options = ArcOptions {
            contribute,
            ancestral: !is_root_prim(&prim),
            ..ArcOptions::plain()
        };
        let kind = if payload {
            ArcType::Payload
        } else {
            ArcType::Reference
        };
        let depth = namespace_depth(&node_path);
        self.add_arc(
            indexer,
            kind,
            node,
            Some(node),
            target_stack,
            prim,
            map,
            number,
            depth,
            options,
        )?;
        Ok(())
    }

    /// The map of an arc from `source` to a node at `target_path`: source
    /// to target (variant selections stripped), after the target layer
    /// stack's relocations at and below the target when it has any.
    pub(super) fn arc_map(
        &self,
        stack: usize,
        source: &Path,
        target_path: &Path,
        offset: LayerOffset,
    ) -> MapFunction {
        let target = target_path.strip_variants();
        let arc = MapFunction::of_pair(source.clone(), target.clone(), offset);
        let layer_stack = self.stacks.stack(stack);
        if !layer_stack.has_relocates() {
            return arc;
        }
        MapFunction::deferred_of(relocation_pairs(layer_stack, &target)).compose(&arc)
    }

    /// Add an arc (`_AddArc`): unless it closes a cycle or its site is a
    /// duplicate to skip, a new child node, or (bringing ancestral opinions)
    /// its target's own index as a subtree; then queue the new nodes' tasks.
    /// The new node, if any.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn add_arc(
        &mut self,
        indexer: &mut Indexer<'_>,
        kind: ArcType,
        parent: u32,
        origin: Option<u32>,
        stack: usize,
        path: Path,
        map: MapFunction,
        sibling: i32,
        depth: i32,
        options: ArcOptions,
    ) -> Result<Option<u32>> {
        let skip =
            options.skip_duplicates || indexer.frame.is_some_and(|frame| frame.skip_duplicates);
        if self.refuses_arc(
            &indexer.index,
            kind,
            parent,
            origin,
            stack,
            &path,
            indexer.frame,
            skip,
        ) {
            return Ok(None);
        }
        let deepest = self.layers.budget().limits().depth;
        if arc_depth(&indexer.index, parent, indexer.frame) >= deepest {
            return Err(Error::limit(format!(
                "composition arcs nest deeper than {deepest}"
            )));
        }
        let added = if !options.ancestral {
            let mut node = Node::new(stack, path.clone(), kind, map);
            node.origin = origin;
            node.sibling = sibling;
            node.depth = depth;
            node.has_specs = self.site_has_specs(stack, &path);
            let added = indexer.index.insert_child(parent, node);
            if !options.contribute {
                indexer.index.set_inert(added);
                // Not even ancestrally.
                indexer.index.node_mut(added).restricted = 1;
            }
            added
        } else {
            let sub = {
                let below = Frame {
                    index: &indexer.index,
                    parent,
                    path: path.clone(),
                    previous: indexer.frame,
                    kind,
                    skip_duplicates: skip,
                    map: &map,
                };
                // Variants and implied specializes wait for the outer index,
                // which knows stronger opinions.
                self.build(stack, &path, Some(&below), options.contribute, false)?
            };
            let added = append_subtree(
                &mut indexer.index,
                parent,
                &sub,
                kind,
                origin,
                sibling,
                depth,
                map,
            );
            if sub.has_payloads {
                indexer.index.has_payloads = true;
            }
            added
        };
        // Dependency flags: a new node is direct unless it copies its
        // origin's; an ancestral subtree takes them from its new root.
        let (due, direct, ancestral_arc) = match origin {
            Some(origin) if options.copy_ancestor => {
                let source = indexer.index.node(origin);
                (source.due_to_ancestor, source.direct, source.ancestral_arc)
            }
            _ => (false, true, indexer.index.node(parent).ancestral_arc),
        };
        let entry = indexer.index.node_mut(added);
        entry.due_to_ancestor = due;
        entry.direct = direct;
        entry.ancestral_arc = ancestral_arc;
        if options.ancestral && indexer.frame.is_none() {
            spread_dependencies(&mut indexer.index, added, direct, ancestral_arc);
        }
        // A nested build evaluated the subtree's expressed arcs; its
        // ancestral variants are evaluated only where variants are.
        let mut tasks = options.tasks;
        if options.ancestral {
            tasks &= !kind::EXPRESSED_ARCS;
        }
        if !(indexer.outer && options.ancestral) {
            tasks &= !kind::ANCESTRAL_VARIANT_SETS;
        }
        if !options.ancestral {
            tasks &= !kind::IMPLIED_SPECIALIZES;
        }
        indexer.add_tasks_for_node(added, tasks);
        if path.is_absolute_root() {
            indexer.index.inert_subtree(added);
        }
        Composer::admit(&indexer.index)?;
        Ok(Some(added))
    }

    /// Whether an arc must not be added: it closes a cycle (variant arcs
    /// never do), or with `skip` its site is one a graph up the stack frames
    /// already holds.
    #[allow(clippy::too_many_arguments)]
    fn refuses_arc(
        &mut self,
        index: &PrimIndex,
        kind: ArcType,
        parent: u32,
        origin: Option<u32>,
        stack: usize,
        path: &Path,
        frame: Option<&Frame<'_>>,
        skip: bool,
    ) -> bool {
        if kind != ArcType::Variant
            && !implied_under_relocate(index, kind, parent, origin, frame)
            && closes_cycle(index, parent, stack, path, frame)
        {
            self.problems.add("Cycle detected");
            return true;
        }
        if !skip {
            return false;
        }
        let mut graph = index;
        let mut site = path.clone();
        let mut level = frame;
        loop {
            if graph.node_using_site(stack, &site).is_some() {
                return true;
            }
            let Some(up) = level else { return false };
            let root_path = &graph.nodes[0].path;
            site = if *root_path == site {
                up.path.clone()
            } else {
                up.path.replace_prefix(root_path, &site)
            };
            graph = up.index;
            level = up.previous;
        }
    }
}

fn map_list<T, U: Clone + PartialEq>(list: &ListOp<T>, convert: impl Fn(&T) -> U) -> ListOp<U> {
    let all = |items: &[T]| items.iter().map(&convert).collect::<Vec<U>>();
    ListOp {
        explicit: list.explicit,
        explicit_items: all(&list.explicit_items),
        added: all(&list.added),
        prepended: all(&list.prepended),
        appended: all(&list.appended),
        deleted: all(&list.deleted),
        ordered: all(&list.ordered),
    }
}

/// `path` mapped from `node` to its graph's root for payload inclusion:
/// step by step, skipping internal arcs' root identities, when the map to
/// the root leaves it unchanged through its root identity.
fn inclusion_path(index: &PrimIndex, node: u32, path: &Path) -> Path {
    let map = index.map_to_root(node);
    let mut mapped = map.map(path, false);
    if mapped != *path || !map.has_root_identity() {
        return mapped;
    }
    let mut at = node;
    while !mapped.is_empty() && at != 0 {
        let entry = index.node(at);
        mapped = to_parent_inclusion(&entry.map_to_parent, entry.arc, &mapped);
        at = entry.parent.unwrap_or(0);
    }
    mapped
}

/// `path` across one arc for payload inclusion: an internal reference's or
/// payload's root identity does not count.
fn to_parent_inclusion(map: &MapFunction, arc: ArcType, path: &Path) -> Path {
    if !map.has_root_identity() || !matches!(arc, ArcType::Reference | ArcType::Payload) {
        return map.map(path, false);
    }
    map.without_root_identity().map(path, false)
}

/// An implied class arc under a relocation's class arcs, which is no cycle.
fn implied_under_relocate(
    index: &PrimIndex,
    kind: ArcType,
    parent: u32,
    origin: Option<u32>,
    frame: Option<&Frame<'_>>,
) -> bool {
    if !kind.is_class_based() || origin == Some(parent) {
        return false;
    }
    let mut graph = index;
    let mut node = Some(parent);
    let mut level = frame;
    let mut steps = 0;
    loop {
        steps += 1;
        if steps > 1 << 16 {
            return false;
        }
        // The arc to the node's parent, across stack frames.
        let arc = match node {
            Some(at) if at > 0 => graph.node(at).arc,
            _ => level.map_or(ArcType::Root, |up| up.kind),
        };
        if !arc.is_class_based() {
            return arc == ArcType::Relocate;
        }
        match node {
            Some(at) if at > 0 => node = graph.node(at).parent,
            _ => {
                let Some(up) = level else { return false };
                graph = up.index;
                node = Some(up.parent);
                level = up.previous;
            }
        }
    }
}

/// How many arcs lead from the root of the outermost build to `parent`,
/// through every stack frame: what an arc from it nests under. The node
/// trees' walks recurse this deep.
fn arc_depth(index: &PrimIndex, parent: u32, frame: Option<&Frame<'_>>) -> usize {
    let mut depth = 0;
    let (mut graph, mut node, mut level) = (index, Some(parent), frame);
    loop {
        while let Some(at) = node {
            depth += 1;
            node = graph.node(at).parent;
        }
        let Some(up) = level else { return depth };
        (graph, node, level) = (up.index, Some(up.parent), up.previous);
    }
}

/// Whether an arc from `parent` to (`stack`, `path`) returns to a site at,
/// above or below one on the way to the root, through every stack frame
/// (`_CheckForCycle`).
fn closes_cycle(
    index: &PrimIndex,
    parent: u32,
    stack: usize,
    path: &Path,
    frame: Option<&Frame<'_>>,
) -> bool {
    let mut graph = index;
    let mut node = Some(parent);
    let mut site = path.clone();
    let mut level = frame;
    loop {
        while let Some(at) = node {
            let entry = graph.node(at);
            if entry.stack == stack
                && (entry.path.has_prefix(&site) || site.has_prefix(&entry.path))
            {
                return true;
            }
            node = entry.parent;
        }
        let Some(up) = level else { return false };
        let root_path = &graph.nodes[0].path;
        // The requested site with this graph's root replaced by the arc's
        // target: where the arc would lead in the enclosing graph.
        site = if *root_path == site {
            up.path.clone()
        } else {
            up.path.replace_prefix(root_path, &site)
        };
        graph = up.index;
        node = Some(up.parent);
        level = up.previous;
    }
}

/// Append `sub`'s nodes under `parent`, its root reached by the given arc;
/// the new root's index.
#[allow(clippy::too_many_arguments)]
fn append_subtree(
    index: &mut PrimIndex,
    parent: u32,
    sub: &PrimIndex,
    kind: ArcType,
    origin: Option<u32>,
    sibling: i32,
    depth: i32,
    map: MapFunction,
) -> u32 {
    let base = index.nodes.len() as u32;
    let mut map = Some(map);
    for (at, node) in sub.nodes.iter().enumerate() {
        let mut copied = node.clone();
        copied.children = node.children.iter().map(|child| child + base).collect();
        if at == 0 {
            copied.map_to_parent = map.take().expect("the root's map");
            copied.arc = kind;
            copied.parent = Some(parent);
            copied.origin = origin;
            copied.sibling = sibling;
            copied.depth = depth;
        } else {
            copied.parent = node.parent.map(|p| p + base);
            copied.origin = node.origin.map(|o| o + base);
        }
        index.nodes.push(copied);
    }
    index.link_child(parent, base);
    base
}

/// Give a subtree its new root's dependency flags too.
fn spread_dependencies(index: &mut PrimIndex, node: u32, direct: bool, ancestral: bool) {
    let entry = index.node_mut(node);
    entry.direct = entry.direct || direct;
    entry.ancestral_arc = entry.ancestral_arc || ancestral;
    for child in index.node(node).children.clone() {
        spread_dependencies(index, child, direct, ancestral);
    }
}
