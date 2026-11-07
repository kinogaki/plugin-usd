// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Instancing, after `Pcp_PrimIndexIsInstanceable`, the instanceable
//! traversals of `instancing.h` and `PcpInstanceKey`
//! (`pxr/usd/pcp/instancing.cpp`, `instanceKey.cpp`). A node is
//! instanceable when it contributes specs and it, or an arc above it, was
//! added directly at this prim rather than coming with a namespace ancestor.
//! A prim is an instance when some node is instanceable and its strongest
//! `instanceable` opinion is true; below an instance only instanceable nodes
//! contribute. Instances whose instanceable arcs and variant selections
//! match share one prototype.

use super::composer::Composer;
use super::graph::{ArcType, PrimIndex};
use crate::usd::sdf::{LayerOffset, Path, Token, Value, field};

/// One arc of an instance key: its type, site and time offset to the root.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceArc {
    arc: ArcType,
    stack: usize,
    path: Path,
    offset: LayerOffset,
}

/// What decides which instances share a prototype (`PcpInstanceKey`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstanceKey {
    arcs: Vec<InstanceArc>,
    /// (set, selection), sorted by set name.
    selections: Vec<(Token, Token)>,
}

impl PrimIndex {
    /// `Pcp_ChildNodeIsInstanceable`.
    pub fn node_instanceable(&self, node: u32) -> bool {
        let entry = self.node(node);
        entry.direct && entry.contributes() && entry.has_specs
    }

    /// Whether an instanceable node lies under `node`, culled subtrees skipped.
    fn has_instanceable_below(&self, node: u32) -> bool {
        self.node(node).children.iter().any(|&child| {
            !self.node(child).culled
                && (self.node_instanceable(child) || self.has_instanceable_below(child))
        })
    }
}

/// Make the index's nodes that are not instanceable inert, down to the
/// instanceable ones.
pub fn disable_non_instanceable(index: &mut PrimIndex) {
    if index.nodes.is_empty() {
        return;
    }
    index.set_inert(0);
    for child in index.nodes[0].children.clone() {
        disable_below(index, child);
    }
}

fn disable_below(index: &mut PrimIndex, node: u32) {
    if index.node(node).culled || index.node_instanceable(node) {
        return;
    }
    index.set_inert(node);
    for child in index.node(node).children.clone() {
        disable_below(index, child);
    }
}

impl Composer {
    /// `Pcp_PrimIndexIsInstanceable`: instanceable data, and the strongest
    /// `instanceable` opinion true.
    pub(super) fn is_instanceable(&self, index: &PrimIndex) -> bool {
        if index.nodes.is_empty() || !index.has_instanceable_below(0) {
            return false;
        }
        for node in index.order(false) {
            let entry = index.node(node);
            if !entry.contributes() {
                continue;
            }
            for &layer in &self.stacks.stack(entry.stack).layers {
                if let Some(found) = self
                    .layers
                    .layer(layer)
                    .get(&entry.path, field::INSTANCEABLE)
                {
                    return found.as_bool() == Some(true);
                }
            }
        }
        false
    }

    /// Whether the parent a nested build extends is an instance: the cached
    /// index's answer when the parent is an ancestor of the prim being
    /// composed, else the rebuilt parent's.
    pub(super) fn ancestor_is_instance(
        &self,
        stack: usize,
        parent: &Path,
        index: &PrimIndex,
    ) -> bool {
        if stack == self.root_stack
            && self.composing.has_prefix(parent)
            && let Some(found) = self.cached_index(parent)
        {
            return found.instanceable;
        }
        self.is_instanceable(index)
    }

    /// The index's instance key: the instanceable arcs met strongest first
    /// (below one only when the index has payloads), and the authored
    /// variant selections.
    pub fn instance_key(&self, index: &PrimIndex) -> InstanceKey {
        let mut key = InstanceKey::default();
        if !index.instanceable {
            return key;
        }
        for &child in &index.node(0).children {
            collect_arcs(index, child, &mut key);
        }
        for node in index.order(false) {
            let entry = index.node(node);
            if !entry.contributes() {
                continue;
            }
            for &layer in &self.stacks.stack(entry.stack).layers {
                let Some(Value::VariantSelections(selections)) = self
                    .layers
                    .layer(layer)
                    .get(&entry.path, field::VARIANT_SELECTION)
                else {
                    continue;
                };
                for (set, selection) in selections.iter() {
                    if let Some(selection) = self.selection_value(entry.stack, selection) {
                        // Keep the strongest selection per set, sorted by name.
                        if let Err(at) = key
                            .selections
                            .binary_search_by(|(known, _)| known.as_bytes().cmp(set.as_bytes()))
                        {
                            key.selections.insert(at, (set.clone(), selection));
                        }
                    }
                }
            }
        }
        key
    }
}

fn collect_arcs(index: &PrimIndex, node: u32, key: &mut InstanceKey) {
    if index.node(node).culled {
        return;
    }
    if index.node_instanceable(node) {
        let entry = index.node(node);
        key.arcs.push(InstanceArc {
            arc: entry.arc,
            stack: entry.stack,
            path: entry.path.clone(),
            offset: index.map_to_root(node).offset,
        });
        if !index.has_payloads {
            return;
        }
    }
    for &child in &index.node(node).children {
        collect_arcs(index, child, key);
    }
}
