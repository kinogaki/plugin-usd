// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! A prim index's node graph, after `PcpPrimIndex_Graph` and `PcpNodeRef`
//! (`pxr/usd/pcp/primIndex_Graph.cpp`, `node.cpp`). Each node is a site (a
//! layer stack and a path in it) reached from its parent by an arc. A
//! parent's children are kept strongest first, so the graph's strength order
//! is its depth-first pre-order (`strength.rs` orders siblings).

use super::map::{MapFunction, namespace_depth};
use crate::usd::sdf::{LayerOffset, Path, PathKind};

/// Arc types in strength order (`PcpArcType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArcType {
    /// The index's root node.
    Root,
    /// An inherit.
    Inherit,
    /// A variant selection.
    Variant,
    /// A relocation.
    Relocate,
    /// A reference.
    Reference,
    /// A payload.
    Payload,
    /// A specialize.
    Specialize,
}

impl ArcType {
    /// Whether the arc is an inherit or a specialize (`PcpIsClassBasedArc`).
    pub fn is_class_based(self) -> bool {
        matches!(self, ArcType::Inherit | ArcType::Specialize)
    }
}

/// One node of a prim index.
#[derive(Clone, Debug)]
pub struct Node {
    /// The layer stack.
    pub stack: usize,
    /// The path in that layer stack.
    pub path: Path,
    /// The arc from the parent.
    pub arc: ArcType,
    /// The parent node (none for the root).
    pub parent: Option<u32>,
    /// The node whose opinion introduced this arc (the parent, for direct
    /// arcs).
    pub origin: Option<u32>,
    /// Children, strongest first.
    pub children: Vec<u32>,
    /// The arc's position among its origin's arcs of its type.
    pub sibling: i32,
    /// Non-variant path elements of the path the arc was introduced at.
    pub depth: i32,
    /// Maps this node's namespace to its parent's.
    pub map_to_parent: MapFunction,
    /// The node contributes no opinions.
    pub inert: bool,
    /// The node and its subtree have no opinions (left out when finalized).
    pub culled: bool,
    /// Whether some layer of its stack has a spec at its path.
    pub has_specs: bool,
    /// The path depth at which the node was first kept from contributing
    /// (`GetSpecContributionRestrictedDepth`); 0 while it is not.
    pub restricted: u32,
    /// The node came with a namespace ancestor's index.
    pub due_to_ancestor: bool,
    /// It, or an arc above it, was added directly at this prim.
    pub direct: bool,
    /// It, or an arc above it, was added ancestrally.
    pub ancestral_arc: bool,
}

impl Node {
    /// A node for `stack`, `path`, reached by `arc`.
    pub fn new(stack: usize, path: Path, arc: ArcType, map_to_parent: MapFunction) -> Node {
        Node {
            stack,
            path,
            arc,
            parent: None,
            origin: None,
            children: Vec::new(),
            sibling: 0,
            depth: 0,
            map_to_parent,
            inert: false,
            culled: false,
            has_specs: false,
            restricted: 0,
            due_to_ancestor: false,
            direct: false,
            ancestral_arc: false,
        }
    }

    /// `PcpNodeRef::CanContributeSpecs` (Usd mode: no permissions).
    pub fn contributes(&self) -> bool {
        !self.inert && !self.culled
    }
}

/// The composed index of one prim.
#[derive(Clone, Debug, Default)]
pub struct PrimIndex {
    /// The nodes; 0 is the root.
    pub nodes: Vec<Node>,
    /// Whether the prim is an instance.
    pub instanceable: bool,
    /// Whether this prim's own nodes author payloads.
    pub has_payloads: bool,
}

impl PrimIndex {
    /// The root node alone at `path` of layer stack `stack`.
    pub fn rooted(stack: usize, path: Path) -> PrimIndex {
        PrimIndex {
            nodes: vec![Node::new(
                stack,
                path,
                ArcType::Root,
                MapFunction::identity(),
            )],
            instanceable: false,
            has_payloads: false,
        }
    }

    /// Node `index`.
    pub fn node(&self, index: u32) -> &Node {
        &self.nodes[index as usize]
    }

    /// Node `index`, to change.
    pub fn node_mut(&mut self, index: u32) -> &mut Node {
        &mut self.nodes[index as usize]
    }

    /// Nodes strongest first (pre-order), culled subtrees left out when
    /// `skip_culled`.
    pub fn order(&self, skip_culled: bool) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.nodes.len());
        if self.nodes.is_empty() {
            return out;
        }
        let mut stack = vec![0u32];
        while let Some(node) = stack.pop() {
            let entry = self.node(node);
            if skip_culled && entry.culled {
                continue;
            }
            out.push(node);
            stack.extend(entry.children.iter().rev());
        }
        out
    }

    /// A node's map to the root's namespace (`PcpNodeRef::GetMapToRoot`).
    pub fn map_to_root(&self, node: u32) -> MapFunction {
        let mut made = self.node(node).map_to_parent.clone();
        let mut at = self.node(node).parent;
        while let Some(parent) = at {
            let entry = self.node(parent);
            made = entry.map_to_parent.compose(&made);
            at = entry.parent;
        }
        made
    }

    /// A node's time offset to the root's.
    pub fn offset_to_root(&self, node: u32) -> LayerOffset {
        let mut offset = self.node(node).map_to_parent.offset;
        let mut at = self.node(node).parent;
        while let Some(parent) = at {
            offset = self.node(parent).map_to_parent.offset.concatenate(&offset);
            at = self.node(parent).parent;
        }
        offset
    }

    /// Add `node` as a child of `parent`, among its siblings by strength;
    /// its index.
    pub fn insert_child(&mut self, parent: u32, mut node: Node) -> u32 {
        node.parent = Some(parent);
        let index = self.nodes.len() as u32;
        self.nodes.push(node);
        self.link_child(parent, index);
        index
    }

    /// List node `index` among `parent`'s children by strength
    /// (`_InsertChildInStrengthOrder`: first, last, else before the first
    /// weaker one).
    pub fn link_child(&mut self, parent: u32, index: u32) {
        let children = &self.node(parent).children;
        let count = children.len();
        let mut at = count;
        if count > 0 {
            if self.compare_siblings(index, children[0]) < 0 {
                at = 0;
            } else if self.compare_siblings(index, children[count - 1]) < 0 {
                at = 1;
                while at < count && self.compare_siblings(index, children[at]) >= 0 {
                    at += 1;
                }
            }
        }
        self.node_mut(parent).children.insert(at, index);
    }

    /// The first node of the site that is neither inert nor culled.
    pub fn node_using_site(&self, stack: usize, path: &Path) -> Option<u32> {
        self.nodes
            .iter()
            .position(|node| {
                !node.inert && !node.culled && node.stack == stack && node.path == *path
            })
            .map(|at| at as u32)
    }

    /// The child of `parent` at the site, if any.
    pub fn matching_child(&self, parent: u32, stack: usize, path: &Path) -> Option<u32> {
        self.node(parent).children.iter().copied().find(|&child| {
            let node = self.node(child);
            node.stack == stack && node.path == *path
        })
    }

    /// Make `node` inert, recording where it was first restricted.
    pub fn set_inert(&mut self, node: u32) {
        let entry = self.node_mut(node);
        let was = entry.inert || entry.culled;
        entry.inert = true;
        if !was && entry.restricted == 0 {
            entry.restricted = entry.path.depth();
        }
    }

    /// Mark `node` culled, recording where it was first restricted.
    pub fn set_culled(&mut self, node: u32) {
        let entry = self.node_mut(node);
        if entry.culled {
            return;
        }
        if entry.restricted == 0 {
            entry.restricted = entry.path.depth();
        }
        entry.culled = true;
    }

    /// Whether `node` may contribute opinions at the ancestral `path`.
    pub fn contributes_at(&self, node: u32, path: &Path) -> bool {
        let restricted = self.node(node).restricted;
        restricted == 0 || restricted > path.depth()
    }

    /// Whether a node is, or lies under, an inherit added at this prim.
    pub fn due_to_direct_inherit(&self, node: u32) -> bool {
        let mut at = Some(node);
        while let Some(index) = at {
            let entry = self.node(index);
            if entry.arc == ArcType::Inherit && !entry.due_to_ancestor {
                return true;
            }
            at = entry.parent;
        }
        false
    }

    /// The node's path at the namespace depth its arc was introduced at
    /// (`GetPathAtIntroduction`): variant selections above that depth stay.
    pub fn path_at_introduction(&self, node: u32) -> Path {
        let entry = self.node(node);
        let Some(parent) = entry.parent else {
            return entry.path.clone();
        };
        let mut below = namespace_depth(&self.node(parent).path) - entry.depth;
        let mut path = entry.path.clone();
        while below > 0 && !path.is_empty() {
            while path.kind() == PathKind::Variant {
                path = path.parent();
            }
            path = path.parent();
            below -= 1;
        }
        path
    }

    /// Make a node and everything under it inert (`_InertSubtree`).
    pub fn inert_subtree(&mut self, node: u32) {
        self.set_inert(node);
        for child in self.node(node).children.clone() {
            self.inert_subtree(child);
        }
    }

    /// Cull a subtree, no longer contributing even ancestrally (`_ElideSubtree`).
    pub fn elide_subtree(&mut self, node: u32) {
        self.set_culled(node);
        self.node_mut(node).restricted = 1;
        for child in self.node(node).children.clone() {
            self.elide_subtree(child);
        }
    }
}
