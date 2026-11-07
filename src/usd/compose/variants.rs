// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Variants, after `_EvalNodeVariantSets`, `_EvalNodeAncestralVariantSets`,
//! `_EvalNodeAuthoredVariant`, `_EvalNodeFallbackVariant`,
//! `_ComposeVariantSelection`, `_AddVariantArc` and `_AddAncestralVariantArc`
//! (`pxr/usd/pcp/primIndex.cpp`) and `PcpComposeSiteVariantSets` and
//! `Selection` (`composeSite.cpp`). A node's variant sets are its site's
//! variantSetNames list ops applied weakest layer first. A set's selection
//! is the one an earlier variant node chose for the same prim, else the
//! strongest authored one met walking the index in strength order from the
//! root (or the node closest to it the prim's path maps to), else the first
//! fallback the set offers. A variant arc's site is the node's path with the
//! selection appended, and its map is the identity. Ancestral variant sets,
//! those of prims above a node that arrived with its ancestral opinions,
//! select alike and bring the whole index below the selection.

use super::arcs::ArcOptions;
use super::composer::Composer;
use super::expressions::{Evaluated, evaluate, is_expression};
use super::graph::{ArcType, PrimIndex};
use super::map::{MapFunction, namespace_depth};
use super::tasks::{Indexer, Task, kind};
use crate::usd::error::Result;
use crate::usd::sdf::{Path, PathKind, Token, Value, field};

impl Composer {
    pub(super) fn eval_variant_sets(&mut self, indexer: &mut Indexer<'_>, node: u32) {
        if !indexer.index.node(node).contributes() {
            return;
        }
        let path = indexer.index.node(node).path.clone();
        self.variant_sets_at(indexer, node, &path, false);
    }

    /// The variant sets of the prims above the node, up to the nearest
    /// variant selection (whose sets were evaluated already).
    pub(super) fn eval_ancestral_variant_sets(&mut self, indexer: &mut Indexer<'_>, node: u32) {
        let mut path = indexer.index.node(node).path.parent();
        while !path.is_empty() && !path.is_root() {
            if indexer.index.contributes_at(node, &path) {
                self.variant_sets_at(indexer, node, &path, true);
                if path.kind() == PathKind::Variant {
                    return;
                }
            }
            path = path.parent();
        }
    }

    /// Queue a selection task for each variant set at `path` of the node's
    /// layer stack.
    fn variant_sets_at(&self, indexer: &mut Indexer<'_>, node: u32, path: &Path, ancestral: bool) {
        let mut sets: Vec<Token> = Vec::new();
        for &layer in self
            .stacks
            .stack(indexer.index.node(node).stack)
            .layers
            .iter()
            .rev()
        {
            if let Some(Value::StringListOp(list) | Value::TokenListOp(list)) =
                self.layers.layer(layer).get(path, field::VARIANT_SET_NAMES)
            {
                list.apply(&mut sets);
            }
        }
        let kind = if ancestral {
            kind::ANCESTRAL_VARIANT_AUTHORED
        } else {
            kind::VARIANT_AUTHORED
        };
        for (number, name) in sets.into_iter().enumerate() {
            indexer.add(Task {
                kind,
                node,
                path: path.clone(),
                name,
                number: number as i32,
            });
        }
    }

    /// Select the task's variant from authored opinions, or defer it to the
    /// fallbacks.
    pub(super) fn eval_authored_variant(
        &mut self,
        indexer: &mut Indexer<'_>,
        task: Task,
        ancestral: bool,
    ) -> Result<()> {
        if !indexer.index.contributes_at(task.node, &task.path) {
            return Ok(());
        }
        let prim = task.path.strip_variants();
        match self.composed_selection(&indexer.index, task.node, &prim, &task.name) {
            Some(selection) => self.add_variant_arc(indexer, &task, selection, ancestral),
            None => {
                let kind = if ancestral {
                    kind::ANCESTRAL_VARIANT_FALLBACK
                } else {
                    kind::VARIANT_FALLBACK
                };
                indexer.add(Task { kind, ..task });
                Ok(())
            }
        }
    }

    /// Select the first fallback the set offers, or wait for a retry.
    pub(super) fn eval_fallback_variant(
        &mut self,
        indexer: &mut Indexer<'_>,
        task: Task,
        ancestral: bool,
    ) -> Result<()> {
        if !indexer.index.contributes_at(task.node, &task.path) {
            return Ok(());
        }
        let set_path = task.path.variant(task.name.clone(), "");
        let layers = &self
            .stacks
            .stack(indexer.index.node(task.node).stack)
            .layers;
        let selection = self
            .variant_fallbacks
            .iter()
            .filter(|(set, _)| *set == task.name)
            .map(|(_, wanted)| wanted)
            .find(|wanted| {
                layers.iter().any(|&layer| {
                    self.layers
                        .layer(layer)
                        .spec(&set_path)
                        .is_some_and(|spec| spec.tokens(field::VARIANT_CHILDREN).contains(wanted))
                })
            })
            .cloned();
        match selection {
            Some(selection) => self.add_variant_arc(indexer, &task, selection, ancestral),
            None => {
                let kind = if ancestral {
                    kind::ANCESTRAL_VARIANT_NONE
                } else {
                    kind::VARIANT_NONE
                };
                indexer.add(Task { kind, ..task });
                Ok(())
            }
        }
    }

    /// The selection for variant set `set` of the prim at `path` in the
    /// node's namespace; none when none (or the empty one) is authored.
    fn composed_selection(
        &self,
        index: &PrimIndex,
        node: u32,
        path: &Path,
        set: &Token,
    ) -> Option<Token> {
        let (start_node, start_path) = toward_root(index, node, path);
        let mut visits = Vec::new();
        walk_translated(index, start_node, start_path, &mut visits);
        // A variant node chose this set's selection for this prim before.
        for (visit, visit_path) in &visits {
            if visit_path.is_empty() || index.node(*visit).arc != ArcType::Variant {
                continue;
            }
            let intro = index.path_at_introduction(*visit);
            let Some((intro_set, intro_selection)) = intro.variant_selection() else {
                continue;
            };
            if intro_set != set {
                continue;
            }
            if intro.prim_path_of_variant() == *visit_path {
                return non_empty(intro_selection.clone());
            }
        }
        // The strongest authored selection.
        for (visit, visit_path) in &visits {
            if visit_path.is_empty() || !index.contributes_at(*visit, visit_path) {
                continue;
            }
            let mut site = visit_path.clone();
            if index.node(*visit).arc == ArcType::Variant {
                let intro = index.path_at_introduction(*visit);
                site = site.replace_prefix(&intro.strip_variants(), &intro);
            }
            if let Some(found) = self.site_selection(index.node(*visit).stack, &site, set) {
                return non_empty(found);
            }
        }
        None
    }

    /// The strongest selection for `set` authored at the site, when any
    /// layer authors one; a selection expression that fails leaves the
    /// choice to weaker layers.
    fn site_selection(&self, stack: usize, path: &Path, set: &Token) -> Option<Token> {
        for &layer in &self.stacks.stack(stack).layers {
            let Some(Value::VariantSelections(selections)) =
                self.layers.layer(layer).get(path, field::VARIANT_SELECTION)
            else {
                continue;
            };
            if let Some(selection) = selections.get(set)
                && let Some(found) = self.selection_value(stack, selection)
            {
                return Some(found);
            }
        }
        None
    }

    /// A selection as authored, or its expression's string; none when the
    /// expression fails (None gives the empty selection).
    pub(super) fn selection_value(&self, stack: usize, selection: &Token) -> Option<Token> {
        if !is_expression(selection) {
            return Some(selection.clone());
        }
        match evaluate(selection, &self.stacks.stack(stack).variables) {
            Evaluated::Nothing => Some(Token::empty()),
            Evaluated::String(text) => Some(text),
            _ => None,
        }
    }

    /// Add the variant arc for `selection` of the task's set; a new variant
    /// may author selections pending tasks are waiting for.
    fn add_variant_arc(
        &mut self,
        indexer: &mut Indexer<'_>,
        task: &Task,
        selection: Token,
        ancestral: bool,
    ) -> Result<()> {
        let stack = indexer.index.node(task.node).stack;
        let node_path = indexer.index.node(task.node).path.clone();
        if !ancestral {
            let path = node_path.variant(task.name.clone(), selection);
            let depth = namespace_depth(&node_path);
            let added = self.add_arc(
                indexer,
                ArcType::Variant,
                task.node,
                Some(task.node),
                stack,
                path.clone(),
                MapFunction::identity(),
                task.number,
                depth,
                ArcOptions::plain(),
            )?;
            if added.is_some() && self.might_select(stack, &path) {
                indexer.retry_variants();
            }
            return Ok(());
        }
        let selected = task.path.variant(task.name.clone(), selection);
        let path = node_path.replace_prefix(&task.path, &selected);
        let options = ArcOptions {
            ancestral: true,
            skip_duplicates: under_local_class(&indexer.index, task.node),
            ..ArcOptions::plain()
        };
        let depth = namespace_depth(&task.path);
        let added = self.add_arc(
            indexer,
            ArcType::Variant,
            task.node,
            Some(task.node),
            stack,
            path,
            MapFunction::identity(),
            task.number,
            depth,
            options,
        )?;
        if added.is_some() && self.might_select(stack, &selected) {
            indexer.retry_variants();
        }
        Ok(())
    }

    /// Whether the site authors selections or arcs that could bring some.
    fn might_select(&self, stack: usize, path: &Path) -> bool {
        self.stacks.stack(stack).layers.iter().any(|&layer| {
            self.layers.layer(layer).spec(path).is_some_and(|spec| {
                [
                    field::VARIANT_SELECTION,
                    field::REFERENCES,
                    field::INHERIT_PATHS,
                    field::SPECIALIZES,
                    field::PAYLOAD,
                ]
                .iter()
                .any(|name| spec.get(name).is_some())
            })
        })
    }

    /// Prefer `selection` for variant set `set` when nothing selects one
    /// (`PcpCache::SetVariantFallbacks`); earlier calls are preferred.
    pub fn add_variant_fallback(&mut self, set: &str, selection: &str) {
        self.variant_fallbacks
            .push((Token::new(set), Token::new(selection)));
    }
}

fn non_empty(selection: Token) -> Option<Token> {
    (!selection.is_empty()).then_some(selection)
}

/// `path` (in `node`'s namespace) mapped to the root node, or to the node
/// closest to it that it maps to, with that node's variant selections put
/// back (`Pcp_TranslatePathFromNodeToRootOrClosestNode`).
fn toward_root(index: &PrimIndex, node: u32, path: &Path) -> (u32, Path) {
    if node == 0 {
        return (0, path.clone());
    }
    let mut here = (node, path.strip_variants());
    let at_root = index.map_to_root(node).map(&here.1, false);
    if !at_root.is_empty() {
        here = (0, at_root);
    } else {
        while let Some(parent) = index.node(here.0).parent {
            let up = index.node(here.0).map_to_parent.map(&here.1, false);
            if up.is_empty() {
                break;
            }
            here = (parent, up);
        }
    }
    let intro = index.path_at_introduction(here.0);
    if intro.has_variant_selection() {
        here.1 = here.1.replace_prefix(&intro.strip_variants(), &intro);
    }
    here
}

/// The nodes under `node` in strength order, each with `path` mapped down
/// into its namespace (empty where it does not map).
fn walk_translated(index: &PrimIndex, node: u32, path: Path, out: &mut Vec<(u32, Path)>) {
    let mut stack = vec![(node, path)];
    while let Some((at, path)) = stack.pop() {
        for &child in index.node(at).children.iter().rev() {
            let below = if path.is_empty() {
                Path::empty()
            } else {
                index.node(child).map_to_parent.map(&path, true)
            };
            stack.push((child, below));
        }
        out.push((at, path));
    }
}

/// Whether a node lies in a subtree rooted at a contributing class arc
/// introduced at its own level of namespace, where class arcs skip
/// duplicate sites; ancestral variants from there skip them too.
fn under_local_class(index: &PrimIndex, node: u32) -> bool {
    let mut at = node;
    while at != 0 {
        let entry = index.node(at);
        if entry.arc.is_class_based()
            && index.depth_below_introduction(at) == 0
            && !(entry.inert || entry.culled)
        {
            return true;
        }
        let Some(parent) = entry.parent else { break };
        at = parent;
    }
    false
}

trait VariantPrim {
    /// The nearest prim path at or above a variant selection path.
    fn prim_path_of_variant(&self) -> Path;
}

impl VariantPrim for Path {
    fn prim_path_of_variant(&self) -> Path {
        let mut at = self.clone();
        while !at.is_empty() && !at.is_root() && at.kind() != PathKind::Prim {
            at = at.parent();
        }
        at
    }
}
