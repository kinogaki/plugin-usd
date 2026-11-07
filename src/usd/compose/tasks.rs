// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The work queue of one index build, after `Pcp_PrimIndexer`'s tasks
//! (`pxr/usd/pcp/primIndex.cpp`): each node's arcs are tasks, taken in the
//! priority order of their kinds (relocations, references, payloads,
//! inherits, specializes, implied specializes and classes, ancestral
//! variants, variants), and among tasks of one kind in node order, except
//! that variant selections go strongest node first, so a stronger node's
//! selection is found before a weaker one asks, and implied classes go
//! descendants first, then strongest first.

use super::graph::{ArcType, PrimIndex};
use super::map::MapFunction;
use crate::usd::sdf::{Path, Token};

/// Task kinds, in priority order (Pcp's `Task::Type`).
pub mod kind {
    #![allow(missing_docs)]
    pub const RELOCATIONS: u32 = 1;
    pub const IMPLIED_RELOCATIONS: u32 = 2;
    pub const REFERENCES: u32 = 4;
    pub const PAYLOADS: u32 = 8;
    pub const INHERITS: u32 = 16;
    pub const SPECIALIZES: u32 = 32;
    pub const IMPLIED_SPECIALIZES: u32 = 64;
    pub const IMPLIED_CLASSES: u32 = 128;
    pub const ANCESTRAL_VARIANT_SETS: u32 = 256;
    pub const ANCESTRAL_VARIANT_AUTHORED: u32 = 512;
    pub const ANCESTRAL_VARIANT_FALLBACK: u32 = 1024;
    pub const ANCESTRAL_VARIANT_NONE: u32 = 2048;
    pub const VARIANT_SETS: u32 = 8192;
    pub const VARIANT_AUTHORED: u32 = 16384;
    pub const VARIANT_FALLBACK: u32 = 32768;
    pub const VARIANT_NONE: u32 = 65536;

    /// Every kind.
    pub const ALL: u32 = 0x1FFFF;
    /// The arcs a node's own opinions express.
    pub const EXPRESSED_ARCS: u32 = RELOCATIONS | REFERENCES | PAYLOADS | INHERITS | SPECIALIZES;
}

/// One task: a kind and a node, and for variant tasks the prim path the
/// variant set is at, its name and its position among the node's sets.
#[derive(Clone, Debug)]
pub struct Task {
    /// The task kind (one bit of [`kind`]).
    pub kind: u32,
    /// The node.
    pub node: u32,
    /// For variant tasks: the prim path.
    pub path: Path,
    /// For variant tasks: the set name.
    pub name: Token,
    /// For variant tasks: the set's position.
    pub number: i32,
}

impl Task {
    /// A task of `kind` for `node`.
    pub fn of(kind: u32, node: u32) -> Task {
        Task {
            kind,
            node,
            path: Path::empty(),
            name: Token::empty(),
            number: 0,
        }
    }
}

/// One level of nested index building (`PcpPrimIndex_StackFrame`): the
/// graph and node the index being built will hang under, and the site
/// requested.
pub struct Frame<'a> {
    /// The enclosing graph.
    pub index: &'a PrimIndex,
    /// The node there the built index hangs under.
    pub parent: u32,
    /// The requested site's path.
    pub path: Path,
    /// The frame above.
    pub previous: Option<&'a Frame<'a>>,
    /// The arc the built index will hang by.
    pub kind: ArcType,
    /// Whether sites the outer graphs already hold are skipped.
    pub skip_duplicates: bool,
    /// The arc's map to the parent node.
    pub map: &'a MapFunction,
}

/// One index being built and its queue.
pub struct Indexer<'a> {
    /// The index.
    pub index: PrimIndex,
    queue: Vec<Task>,
    /// The enclosing build, for nested builds.
    pub frame: Option<&'a Frame<'a>>,
    /// Whether this is an outermost build, where variants and implied
    /// specializes are evaluated.
    pub outer: bool,
}

/// Where a class hierarchy's implied classes start propagating from
/// (`_FindStartingNodeForImpliedClasses`).
fn implied_classes_start(index: &PrimIndex, node: u32) -> u32 {
    let mut start = node;
    let mut steps = 0;
    while index.node(start).arc.is_class_based() && steps <= index.nodes.len() {
        steps += 1;
        let found = index.hierarchy_start(start);
        start = found.instance;
        if index.node(start).arc.is_class_based()
            && let Some(class_node) = found.class_node
        {
            let ancestral = index.path_at_introduction(start);
            if index.node(class_node).path.has_prefix(&ancestral) {
                break;
            }
        }
    }
    start
}

impl<'a> Indexer<'a> {
    /// A build of `index`.
    pub fn new(index: PrimIndex, frame: Option<&'a Frame<'a>>, outer: bool) -> Indexer<'a> {
        Indexer {
            index,
            queue: Vec::new(),
            frame,
            outer,
        }
    }

    /// Queue a task; implied class and specializes tasks once per node.
    pub fn add(&mut self, task: Task) {
        if (task.kind == kind::IMPLIED_CLASSES || task.kind == kind::IMPLIED_SPECIALIZES)
            && self
                .queue
                .iter()
                .any(|queued| queued.kind == task.kind && queued.node == task.node)
        {
            return;
        }
        self.queue.push(task);
    }

    /// Queue a new node's tasks (`AddTasksForNode`): the implied classes of
    /// the class hierarchy it joins or brings, its subtree's specializes to
    /// propagate, then `kinds` for it and every node under it.
    pub fn add_tasks_for_node(&mut self, node: u32, kinds: u32) {
        if kinds & kind::IMPLIED_CLASSES != 0 {
            if self.index.node(node).arc.is_class_based() {
                let start = implied_classes_start(&self.index, node);
                self.add(Task::of(kind::IMPLIED_CLASSES, start));
            } else if self.index.has_class_based_child(node) {
                self.add(Task::of(kind::IMPLIED_CLASSES, node));
            }
        }
        if kinds & kind::IMPLIED_SPECIALIZES != 0
            && self.outer
            && self.index.has_specializes_below(node)
        {
            self.add(Task::of(kind::IMPLIED_SPECIALIZES, node));
        }
        self.add_tasks(node, kinds);
    }

    /// Queue `kinds` for `node` and every node under it.
    pub fn add_tasks(&mut self, node: u32, kinds: u32) {
        let mut mask = kinds;
        if !self.outer {
            mask &= !kind::VARIANT_SETS;
        }
        for child in self.index.node(node).children.clone() {
            self.add_tasks(child, mask);
        }
        // Only a relocation node implies relocations to its grandparent.
        if self.index.node(node).arc != ArcType::Relocate {
            mask &= !kind::IMPLIED_RELOCATIONS;
        }
        for task in [
            kind::ANCESTRAL_VARIANT_SETS,
            kind::VARIANT_SETS,
            kind::SPECIALIZES,
            kind::INHERITS,
            kind::PAYLOADS,
            kind::REFERENCES,
            kind::RELOCATIONS,
            kind::IMPLIED_RELOCATIONS,
        ] {
            if mask & task != 0 {
                self.add(Task::of(task, node));
            }
        }
    }

    /// The next task, highest priority first (none when done).
    pub fn pop(&mut self) -> Option<Task> {
        if self.queue.is_empty() {
            return None;
        }
        let mut best = 0;
        for at in 1..self.queue.len() {
            if self.before(&self.queue[at], &self.queue[best]) {
                best = at;
            }
        }
        Some(self.queue.swap_remove(best))
    }

    /// Whether task `a` runs before task `b` (`Task::PriorityOrder`).
    fn before(&self, a: &Task, b: &Task) -> bool {
        if a.kind != b.kind {
            return a.kind < b.kind;
        }
        let selection = matches!(
            a.kind,
            kind::VARIANT_AUTHORED
                | kind::VARIANT_FALLBACK
                | kind::ANCESTRAL_VARIANT_AUTHORED
                | kind::ANCESTRAL_VARIANT_FALLBACK
        );
        if selection {
            if a.node != b.node {
                return self.index.compare_nodes(a.node, b.node) < 0;
            }
            if a.path != b.path {
                return a.path < b.path;
            }
            return a.number < b.number;
        }
        if a.kind == kind::IMPLIED_CLASSES {
            // Descendants before their ancestors, else strongest first.
            if b.node > a.node && self.index.is_ancestor(a.node, b.node) {
                return false;
            }
            if a.node > b.node && self.index.is_ancestor(b.node, a.node) {
                return true;
            }
            return self.index.compare_nodes(a.node, b.node) < 0;
        }
        if a.node != b.node {
            return a.node < b.node;
        }
        if a.path != b.path {
            return a.path < b.path;
        }
        a.number < b.number
    }

    /// Queued fallback and none-found variant tasks become authored ones
    /// again: a new variant may author selections (`RetryVariantTasks`).
    pub fn retry_variants(&mut self) {
        for task in &mut self.queue {
            task.kind = match task.kind {
                kind::VARIANT_FALLBACK | kind::VARIANT_NONE => kind::VARIANT_AUTHORED,
                kind::ANCESTRAL_VARIANT_FALLBACK | kind::ANCESTRAL_VARIANT_NONE => {
                    kind::ANCESTRAL_VARIANT_AUTHORED
                }
                other => other,
            };
        }
    }
}
