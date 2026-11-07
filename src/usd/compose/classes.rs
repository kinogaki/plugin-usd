// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Inherits and specializes, after `_EvalNodeInherits`,
//! `_EvalNodeSpecializes`, `_AddClassBasedArc`, `_EvalImpliedClasses`,
//! `_EvalImpliedClassTree` and `_EvalImpliedSpecializes`
//! (`pxr/usd/pcp/primIndex.cpp`). A class arc maps the class to the instance
//! and every other path to itself. Classes are implied up the graph: when a
//! node inherits a class, its parent inherits the class mapped across the arc
//! between them (an implied class), so a referencing layer stack's opinions
//! about the class apply too. Specializes nodes are added as inert
//! placeholders where authored and propagated (copied) under the root, so
//! their opinions come after every other arc's.

use super::arcs::{ArcOptions, is_root_prim};
use super::composer::Composer;
use super::graph::ArcType;
use super::map::{MapFunction, namespace_depth};
use super::tasks::{Indexer, Task, kind};
use crate::usd::error::Result;
use crate::usd::sdf::{LayerOffset, Path, PathKind, Value, field};

impl Composer {
    /// Add the inherits (or specializes) the node's site authors.
    pub(super) fn eval_class_arcs(
        &mut self,
        indexer: &mut Indexer<'_>,
        node: u32,
        arc: ArcType,
    ) -> Result<()> {
        if !indexer.index.node(node).contributes() {
            return Ok(());
        }
        let stack = indexer.index.node(node).stack;
        let node_path = indexer.index.node(node).path.clone();
        let name = if arc == ArcType::Inherit {
            field::INHERIT_PATHS
        } else {
            field::SPECIALIZES
        };
        let mut paths: Vec<Path> = Vec::new();
        for &layer in self.stacks.stack(stack).layers.iter().rev() {
            if let Some(Value::PathListOp(list)) = self.layers.layer(layer).get(&node_path, name) {
                list.apply(&mut paths);
            }
        }
        for (number, path) in paths.into_iter().enumerate() {
            if !path.is_empty() && !(path.is_prim() && !path.has_variant_selection()) {
                self.problems.add("Invalid inherit or specialize prim path");
                continue;
            }
            // The class maps to the instance, every other path to itself.
            let map = self.arc_map(stack, &path, &node_path, LayerOffset::IDENTITY, true)?;
            self.add_class_arc(indexer, path, arc, node, node, map, number as i32, None)?;
        }
        Ok(())
    }

    /// Add one class arc under `parent` (`_AddClassBasedArc`), or find the
    /// one there already. An implied class to the site `ignore` it comes
    /// from, or to the parent's own site, contributes nothing but keeps
    /// propagating.
    #[allow(clippy::too_many_arguments)]
    fn add_class_arc(
        &mut self,
        indexer: &mut Indexer<'_>,
        path: Path,
        arc: ArcType,
        parent: u32,
        origin: u32,
        map: MapFunction,
        sibling: i32,
        ignore: Option<(usize, &Path)>,
    ) -> Result<Option<u32>> {
        if path.is_empty() {
            // Outside the arc's co-domain: no site to inherit from here.
            return Ok(None);
        }
        let stack = indexer.index.node(parent).stack;
        if let Some(child) = existing_class_arc(indexer, &path, arc, parent, origin, &map) {
            // An implied specializes to the root prefers the stronger origin.
            let implied = arc == ArcType::Specialize && parent == 0 && parent != origin;
            let existing_origin = indexer.index.node(child).origin.unwrap_or(child);
            if !implied || indexer.index.compare_nodes(origin, existing_origin) >= 0 {
                return Ok(Some(child));
            }
            indexer.index.inert_subtree(child);
        }
        let depth = namespace_depth(&indexer.index.node(parent).path);
        if arc == ArcType::Specialize && (parent != 0 || indexer.frame.is_some()) {
            // An inert placeholder; its copy under the root contributes.
            let options = ArcOptions {
                contribute: false,
                tasks: 0,
                ..ArcOptions::plain()
            };
            let Some(added) = self.add_arc(
                indexer,
                arc,
                parent,
                Some(origin),
                stack,
                path,
                map,
                sibling,
                depth,
                options,
            )?
            else {
                return Ok(None);
            };
            if indexer.frame.is_some() || indexer.index.relocates_placeholder(added) {
                return Ok(Some(added));
            }
            let propagated = self.propagate_to_root(indexer, added)?;
            if let Some(copy) = propagated
                && indexer.index.node(copy).origin != Some(added)
            {
                indexer.add_tasks_for_node(added, kind::IMPLIED_CLASSES);
            }
            return Ok(propagated);
        }
        let ignored = ignore.is_some_and(|(ignore_stack, ignore_path)| {
            stack == ignore_stack && path == *ignore_path
        });
        let contribute = path != indexer.index.node(parent).path && !ignored;
        let options = ArcOptions {
            contribute,
            skip_duplicates: contribute,
            // Only subroot classes bring ancestral opinions.
            ancestral: contribute && !is_root_prim(&path),
            ..ArcOptions::plain()
        };
        self.add_arc(
            indexer,
            arc,
            parent,
            Some(origin),
            stack,
            path,
            map,
            sibling,
            depth,
            options,
        )
    }

    /// Copy specializes node `node` under the root (`_PropagateNodeToRoot`),
    /// unless the root has a child at its site already.
    fn propagate_to_root(&mut self, indexer: &mut Indexer<'_>, node: u32) -> Result<Option<u32>> {
        let entry = indexer.index.node(node);
        if let Some(found) = indexer.index.matching_child(0, entry.stack, &entry.path) {
            return Ok(Some(found));
        }
        let (arc, stack, path, sibling) =
            (entry.arc, entry.stack, entry.path.clone(), entry.sibling);
        let map = indexer.index.map_to_root(node);
        let options = ArcOptions {
            skip_duplicates: true,
            copy_ancestor: true,
            ancestral: !is_root_prim(&path),
            ..ArcOptions::plain()
        };
        let depth = namespace_depth(&indexer.index.nodes[0].path);
        self.add_arc(
            indexer,
            arc,
            0,
            Some(node),
            stack,
            path,
            map,
            sibling,
            depth,
            options,
        )
    }

    /// Imply the class arcs under `node` to its parent (`_EvalImpliedClasses`).
    pub(super) fn eval_implied_classes(
        &mut self,
        indexer: &mut Indexer<'_>,
        node: u32,
    ) -> Result<()> {
        if node == 0 || !indexer.index.has_class_based_child(node) {
            return Ok(());
        }
        // Classes map across any arc: add the root identity.
        let transfer = indexer.index.node(node).map_to_parent.with_root_identity();
        let parent = indexer.index.node(node).parent.unwrap_or(0);
        self.implied_class_tree(indexer, parent, node, &transfer, true, 0)
    }

    /// Imply the class arcs under `source` (a whole class hierarchy) to
    /// `destination` through `transfer` (`_EvalImpliedClassTree`).
    fn implied_class_tree(
        &mut self,
        indexer: &mut Indexer<'_>,
        destination: u32,
        source: u32,
        transfer: &MapFunction,
        start: bool,
        depth: usize,
    ) -> Result<()> {
        if depth > 256 {
            return Ok(());
        }
        if indexer.index.node(destination).arc == ArcType::Relocate {
            // Relocation nodes hold no implied classes: go to their parent.
            let outer = indexer
                .index
                .node(destination)
                .map_to_parent
                .with_root_identity();
            let composed = outer.compose(transfer);
            let parent = indexer.index.node(destination).parent.unwrap_or(0);
            self.implied_class_tree(indexer, parent, source, &composed, start, depth + 1)?;
            indexer.add(Task::of(kind::IMPLIED_CLASSES, destination));
            return Ok(());
        }
        let owner = indexer.index.propagated_of(source).unwrap_or(source);
        for child in indexer.index.node(owner).children.clone() {
            if !indexer.index.node(child).arc.is_class_based() {
                continue;
            }
            // The arc continuing the destination's own chain of classes.
            if start
                && indexer.index.node(source).arc.is_class_based()
                && indexer.index.depth_below_introduction(source)
                    == indexer.index.depth_below_introduction(child)
            {
                continue;
            }
            let mut implied = indexer
                .index
                .node(destination)
                .children
                .iter()
                .copied()
                .find(|&there| {
                    indexer.index.node(there).origin == Some(child)
                        && !indexer.index.is_propagated(there)
                });
            if implied.is_none() {
                let path = inherit_path(
                    &indexer.index.node(destination).path,
                    &indexer.index.node(child).path,
                    transfer,
                );
                let class_map = transfer.implied_class(&indexer.index.node(child).map_to_parent);
                let entry = indexer.index.node(child);
                let (arc, sibling, stack, child_path) =
                    (entry.arc, entry.sibling, entry.stack, entry.path.clone());
                implied = self.add_class_arc(
                    indexer,
                    path,
                    arc,
                    destination,
                    child,
                    class_map,
                    sibling,
                    Some((stack, &child_path)),
                )?;
            }
            let Some(found) = implied else { continue };
            if !indexer.index.has_class_based_child(child) {
                continue;
            }
            let below = indexer.index.propagated_of(found).unwrap_or(found);
            self.implied_class_tree(indexer, below, child, transfer, false, depth + 1)?;
        }
        Ok(())
    }

    /// Copy the specializes nodes at and under `node` under the root
    /// (`_EvalImpliedSpecializes`).
    pub(super) fn eval_implied_specializes(
        &mut self,
        indexer: &mut Indexer<'_>,
        node: u32,
    ) -> Result<()> {
        if node == 0 {
            return Ok(());
        }
        self.propagate_specializes(indexer, node, 0)
    }

    fn propagate_specializes(
        &mut self,
        indexer: &mut Indexer<'_>,
        node: u32,
        depth: usize,
    ) -> Result<()> {
        if depth > 1024 || indexer.index.relocates_placeholder(node) {
            return Ok(());
        }
        if indexer.index.node(node).arc == ArcType::Specialize {
            self.propagate_to_root(indexer, node)?;
        }
        for child in indexer.index.node(node).children.clone() {
            self.propagate_specializes(indexer, child, depth + 1)?;
        }
        Ok(())
    }
}

/// A child of `parent` that already is this class arc: the same site, or
/// under a relocation, the same kind, origin depth and map.
fn existing_class_arc(
    indexer: &Indexer<'_>,
    path: &Path,
    arc: ArcType,
    parent: u32,
    origin: u32,
    map: &MapFunction,
) -> Option<u32> {
    let index = &indexer.index;
    let mut parent_arc = index.node(parent).arc;
    if parent == 0
        && let Some(frame) = indexer.frame
    {
        parent_arc = frame.kind;
    }
    if parent_arc != ArcType::Relocate {
        return index.matching_child(parent, index.node(parent).stack, path);
    }
    let depth = index.depth_below_introduction(origin);
    index.node(parent).children.iter().copied().find(|&child| {
        let entry = index.node(child);
        entry.arc == arc
            && entry
                .origin
                .is_some_and(|o| index.depth_below_introduction(o) == depth)
            && entry.map_to_parent.equals(map)
    })
}

/// The class site under the destination: the source's path mapped across
/// the transfer, variant selections of the destination's path put back
/// (`_DetermineInheritPath`).
fn inherit_path(destination: &Path, source: &Path, transfer: &MapFunction) -> Path {
    let mapped = transfer.map(&source.strip_variants(), false);
    if !destination.has_variant_selection() {
        return mapped;
    }
    let mut selection = destination.clone();
    while selection.kind() != PathKind::Variant {
        selection = selection.parent();
    }
    mapped.replace_prefix(&selection.strip_variants(), &selection)
}
