// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! What a prim index composes, after `PcpPrimIndex::ComputePrimChildNames`,
//! `ComputePrimPropertyNames` and `PcpComputeRelationshipTargetPaths`
//! (`pxr/usd/pcp/primIndex.cpp`, `targetIndex.cpp`). Names compose weakest
//! node first (a node after its children, weaker children first), and within
//! a node weakest layer first, each name once, primOrder reordering after
//! each layer.

use std::collections::{HashMap, HashSet};

use super::composer::Composer;
use super::graph::PrimIndex;
use crate::usd::error::Result;
use crate::usd::sdf::{ListOp, Path, Token, Value, field};

impl Composer {
    /// The prim's child names in order; an instance's come from its
    /// instanceable nodes only. Relocations rename, remove and add children;
    /// names relocated away are left out.
    pub fn child_names(&self, index: &PrimIndex) -> Vec<Token> {
        let mut out = Vec::new();
        let mut prohibited = Vec::new();
        if !index.nodes.is_empty() {
            self.names_at(
                index,
                0,
                true,
                index.instanceable,
                &mut out,
                &mut prohibited,
            );
        }
        if !prohibited.is_empty() {
            out.retain(|name| !prohibited.contains(name));
        }
        out
    }

    /// The prim's property names in order.
    pub fn property_names(&self, index: &PrimIndex) -> Vec<Token> {
        let mut out = Vec::new();
        let mut unused = Vec::new();
        if !index.nodes.is_empty() {
            self.names_at(index, 0, false, false, &mut out, &mut unused);
        }
        out
    }

    fn names_at(
        &self,
        index: &PrimIndex,
        node: u32,
        prims: bool,
        instance: bool,
        out: &mut Vec<Token>,
        prohibited: &mut Vec<Token>,
    ) {
        // Post-order, children weakest first (a pre-order with children
        // strongest first, reversed), without recursion.
        let mut visit = Vec::new();
        let mut stack = vec![node];
        while let Some(at) = stack.pop() {
            let entry = index.node(at);
            if entry.culled {
                continue;
            }
            visit.push(at);
            stack.extend(entry.children.iter().rev().copied());
        }
        let mut seen: HashSet<Token> = out.iter().cloned().collect();
        for &at in visit.iter().rev() {
            let entry = index.node(at);
            if instance {
                if at == 0 || !index.node_instanceable(at) {
                    continue;
                }
            } else if !entry.contributes() {
                continue;
            }
            if prims && self.stacks.stack(entry.stack).has_relocates() {
                self.relocate_names(entry.stack, &entry.path, out, prohibited);
                seen = out.iter().cloned().collect();
            }
            for &layer in self.stacks.stack(entry.stack).layers.iter().rev() {
                let Some(spec) = self.layers.layer(layer).spec(&entry.path) else {
                    continue;
                };
                for name in spec.tokens(if prims {
                    field::PRIM_CHILDREN
                } else {
                    field::PROPERTIES
                }) {
                    if seen.insert(name.clone()) {
                        out.push(name.clone());
                    }
                }
                if prims {
                    let order = spec.tokens(field::PRIM_ORDER);
                    if !order.is_empty() {
                        apply_ordering(out, order);
                    }
                }
            }
        }
    }

    /// The children relocations change at a node's path: a child relocated
    /// under the same parent is renamed, one relocated elsewhere removed
    /// (both prohibited), and one relocated here from elsewhere added (in
    /// name order).
    fn relocate_names(
        &self,
        stack: usize,
        path: &Path,
        out: &mut Vec<Token>,
        prohibited: &mut Vec<Token>,
    ) {
        // Each found by its name, so the work is linear in the names.
        let mut renamed: HashMap<Token, Token> = HashMap::new();
        let mut removed: HashSet<Token> = HashSet::new();
        let mut banned: HashSet<Token> = prohibited.iter().cloned().collect();
        let relocates = &self.stacks.stack(stack).relocates;
        for relocate in relocates {
            if relocate.source.parent() != *path {
                continue;
            }
            let old = relocate.source.name_token();
            if !relocate.target.is_empty() && relocate.target.parent() == *path {
                renamed.insert(old.clone(), relocate.target.name_token());
            } else {
                removed.insert(old.clone());
            }
            if banned.insert(old.clone()) {
                prohibited.push(old);
            }
        }
        let held: HashSet<Token> = out.iter().cloned().collect();
        let mut added: Vec<Token> = Vec::new();
        let mut adding: HashSet<Token> = HashSet::new();
        for relocate in relocates {
            if relocate.target.is_empty()
                || relocate.target.parent() != *path
                || relocate.source.parent() == *path
            {
                continue;
            }
            let name = relocate.target.name_token();
            if !held.contains(&name) && adding.insert(name.clone()) {
                added.push(name);
            }
        }
        if !renamed.is_empty() || !removed.is_empty() {
            let mut kept: Vec<Token> = Vec::new();
            let mut keeping: HashSet<Token> = HashSet::new();
            for name in out.iter() {
                if let Some(to) = renamed.get(name) {
                    if !held.contains(to) && keeping.insert(to.clone()) {
                        kept.push(to.clone());
                    }
                    continue;
                }
                if !removed.contains(name) && keeping.insert(name.clone()) {
                    kept.push(name.clone());
                }
            }
            *out = kept;
        }
        // std::set order: by name text.
        added.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        out.extend(added);
    }

    /// A relationship's targets (`connections`: an attribute's connections)
    /// in the root namespace: each node's opinions applied weakest first,
    /// relative paths anchored at the property, then mapped to the root.
    pub fn target_paths(
        &self,
        index: &PrimIndex,
        name: &Token,
        connections: bool,
    ) -> Result<Vec<Path>> {
        let mut items: Vec<Path> = Vec::new();
        let order = index.order(true);
        let key = if connections {
            field::CONNECTION_PATHS
        } else {
            field::TARGET_PATHS
        };
        for &node in order.iter().rev() {
            let entry = index.node(node);
            if !entry.contributes() {
                continue;
            }
            let property = entry.path.property(name.clone());
            let mut map = None;
            for &layer in self.stacks.stack(entry.stack).layers.iter().rev() {
                let Some(Value::PathListOp(list)) = self.layers.layer(layer).get(&property, key)
                else {
                    continue;
                };
                let map = map.get_or_insert_with(|| index.map_to_root(node));
                // Translate this layer's opinion into the root namespace, then apply it.
                let translate = |paths: &[Path]| -> Result<Vec<Path>> {
                    let mut out = Vec::with_capacity(paths.len());
                    for path in paths {
                        let absolute = path.make_absolute(&property)?;
                        let mapped = map.map(&absolute, false);
                        if !mapped.is_empty() {
                            out.push(mapped);
                        }
                    }
                    Ok(out)
                };
                let translated = ListOp {
                    explicit: list.explicit,
                    explicit_items: translate(&list.explicit_items)?,
                    added: translate(&list.added)?,
                    prepended: translate(&list.prepended)?,
                    appended: translate(&list.appended)?,
                    deleted: translate(&list.deleted)?,
                    ordered: translate(&list.ordered)?,
                };
                translated.apply(&mut items);
            }
        }
        Ok(items)
    }
}

/// `SdfApplyListOrdering`: each name of `order` that is present starts a
/// run taking the names after it up to the next ordered one; the runs follow
/// `order`, and names before any ordered one stay first.
pub fn apply_ordering(names: &mut Vec<Token>, order: &[Token]) {
    if order.is_empty() || names.is_empty() {
        return;
    }
    let mut ordered: HashSet<&Token> = HashSet::new();
    let unique: Vec<&Token> = order.iter().filter(|name| ordered.insert(name)).collect();
    let scratch = std::mem::take(names);
    // Where each name is, found by the name.
    let mut places: HashMap<&Token, Vec<usize>> = HashMap::new();
    for (at, name) in scratch.iter().enumerate() {
        places.entry(name).or_default().push(at);
    }
    let mut taken = vec![false; scratch.len()];
    let mut tail = Vec::new();
    for name in unique {
        // The last occurrence not yet taken.
        let last = places
            .get(name)
            .and_then(|at| at.iter().rev().find(|&&k| !taken[k]));
        let Some(&start) = last else {
            continue;
        };
        let mut end = start;
        loop {
            tail.push(scratch[end].clone());
            taken[end] = true;
            end += 1;
            if end >= scratch.len() || ordered.contains(&scratch[end]) {
                break;
            }
        }
    }
    for (at, name) in scratch.iter().enumerate() {
        if !taken[at] {
            names.push(name.clone());
        }
    }
    names.extend(tail);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(names: &[&str]) -> Vec<Token> {
        names.iter().map(|name| Token::new(name)).collect()
    }

    #[test]
    fn orders_as_sdf() {
        let mut names = tokens(&["a", "b", "c", "d", "e"]);
        apply_ordering(&mut names, &tokens(&["d", "b"]));
        assert_eq!(names, tokens(&["a", "d", "e", "b", "c"]));
    }
}
