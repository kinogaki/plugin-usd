// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Strength order, after `PcpCompareSiblingNodeStrength` and
//! `PcpCompareNodeStrength` (`pxr/usd/pcp/strengthOrdering.cpp`) and the
//! class hierarchy helpers of `utils.cpp`. Siblings compare by arc type
//! (LIVRPS), then the namespace depth their arcs were introduced at (deeper
//! is stronger), then their origins' strength, then their order among the
//! arcs authored. Specializes nodes are copied under the root (propagated)
//! so they end up weakest; among those, origins nested under one another,
//! implied copies and chains of origins decide. A node is stronger than
//! another when a pre-order walk of the graph meets it first.

use std::cmp::Ordering;

use super::graph::{ArcType, PrimIndex};
use super::map::namespace_depth;

/// A class hierarchy's start: the instance node that inherits (or
/// specializes) it and the first class node below it.
pub struct Hierarchy {
    /// The instance node.
    pub instance: u32,
    /// The first class node, if any.
    pub class_node: Option<u32>,
}

fn sign(ordering: Ordering) -> i32 {
    match ordering {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

impl PrimIndex {
    /// Negative when sibling `a` is stronger, positive when `b` is.
    pub fn compare_siblings(&self, a: u32, b: u32) -> i32 {
        if a == b {
            return 0;
        }
        let (x, y) = (self.node(a), self.node(b));
        if x.arc != y.arc {
            return sign(x.arc.cmp(&y.arc));
        }
        if x.arc == ArcType::Specialize {
            let result = self.compare_specializes(a, b);
            if result != 0 {
                return result;
            }
        } else {
            if x.depth != y.depth {
                return sign(y.depth.cmp(&x.depth));
            }
            if x.origin != y.origin
                && let (Some(xo), Some(yo)) = (x.origin, y.origin)
            {
                let result = self.compare_nodes(xo, yo);
                if result != 0 {
                    return result;
                }
            }
        }
        sign(x.sibling.cmp(&y.sibling))
    }

    /// The specializes part of the sibling order; 0 to fall through to the
    /// arcs' sibling numbers.
    fn compare_specializes(&self, a: u32, b: u32) -> i32 {
        let (x, y) = (self.node(a), self.node(b));
        let a_root = self.origin_root(a);
        let b_root = self.origin_root(b);
        // Deeper opinions are stronger, unless one origin is nested under
        // the other: then a specializes arc lies between them.
        if !(self.nested_under(a_root, b_root) || self.nested_under(b_root, a_root))
            && x.depth != y.depth
        {
            return sign(y.depth.cmp(&x.depth));
        }
        let (Some(a_origin), Some(b_origin)) = (x.origin, y.origin) else {
            return 0;
        };
        let a_authored = x.origin == x.parent;
        let b_authored = y.origin == y.parent;
        if a_origin == b_origin {
            if a_authored || b_authored {
                return 0;
            }
            // An implied copy (its site not its origin's) beats the propagated one.
            let a_implied = !self.same_site(a, a_origin);
            let b_implied = !self.same_site(b, b_origin);
            if a_implied != b_implied {
                return if a_implied { -1 } else { 1 };
            }
            return 0;
        }
        if a_root != b_root {
            return self.compare_nodes(a_root, b_root);
        }
        // The same authored specialize arc: the shallower instance of the
        // class hierarchy, then the longer chain of origins, then implied
        // over propagated in the root layer stack, then the origins.
        let a_depth = if a_authored {
            0
        } else {
            self.hierarchy_depth(a_origin)
        };
        let b_depth = if b_authored {
            0
        } else {
            self.hierarchy_depth(b_origin)
        };
        if a_depth != b_depth {
            return sign(a_depth.cmp(&b_depth));
        }
        let a_steps = self.origin_steps(a);
        let b_steps = self.origin_steps(b);
        if a_steps != b_steps {
            return sign(b_steps.cmp(&a_steps));
        }
        let root_stack = self.nodes[0].stack;
        if x.stack == root_stack && y.stack == root_stack && !a_authored && !b_authored {
            let a_implied = !self.same_site(a, a_origin);
            let b_implied = !self.same_site(b, b_origin);
            if a_implied != b_implied {
                return if a_implied { -1 } else { 1 };
            }
        }
        self.compare_nodes(a_origin, b_origin)
    }

    /// `PcpCompareNodeStrength`: negative when `a` is stronger (a pre-order
    /// walk meets it first), positive when `b` is, 0 for the same node.
    pub fn compare_nodes(&self, a: u32, b: u32) -> i32 {
        if a == b {
            return 0;
        }
        let mut stack = vec![0u32];
        while let Some(node) = stack.pop() {
            if node == a {
                return -1;
            }
            if node == b {
                return 1;
            }
            stack.extend(self.node(node).children.iter().rev());
        }
        0
    }

    /// Whether two nodes are the same site.
    pub fn same_site(&self, a: u32, b: u32) -> bool {
        let (x, y) = (self.node(a), self.node(b));
        x.stack == y.stack && x.path == y.path
    }

    /// The node an implied chain of origins starts at (`GetOriginRootNode`).
    pub fn origin_root(&self, node: u32) -> u32 {
        let mut at = node;
        while let Some(origin) = self.node(at).origin {
            if Some(origin) == self.node(at).parent {
                break;
            }
            at = origin;
        }
        at
    }

    /// How many origins lie between a node and its origin root.
    fn origin_steps(&self, node: u32) -> u32 {
        let mut steps = 0;
        let mut at = node;
        while let Some(origin) = self.node(at).origin {
            if Some(origin) == self.node(at).parent {
                break;
            }
            at = origin;
            steps += 1;
        }
        steps
    }

    /// Whether `x` lies under `y`, through parents or, from propagated
    /// specializes nodes, their origins (`_OriginsAreNestedArcs`).
    fn nested_under(&self, x: u32, y: u32) -> bool {
        let mut at = Some(x);
        let mut steps = 0;
        while let Some(index) = at {
            if index == y {
                return true;
            }
            steps += 1;
            if steps > self.nodes.len() {
                return false;
            }
            at = if self.is_propagated(index) {
                self.node(index).origin
            } else {
                self.node(index).parent
            };
        }
        false
    }

    /// The namespace depth of the instance of the class hierarchy a node
    /// belongs to, relocates skipped.
    fn hierarchy_depth(&self, node: u32) -> i32 {
        let mut instance = self.hierarchy_start(node).instance;
        while self.node(instance).arc == ArcType::Relocate {
            match self.node(instance).parent {
                Some(parent) => instance = parent,
                None => break,
            }
        }
        self.node(instance).depth
    }

    /// `Pcp_FindStartingNodeOfClassHierarchy`: walk up the class arcs
    /// introduced at the node's depth to the instance inheriting them.
    pub fn hierarchy_start(&self, node: u32) -> Hierarchy {
        let mut instance = node;
        if self.is_propagated(instance) {
            instance = self.node(instance).origin.unwrap_or(instance);
        }
        let depth = self.depth_below_introduction(instance);
        let mut class_node = None;
        while self.node(instance).arc.is_class_based()
            && self.depth_below_introduction(instance) == depth
        {
            class_node = Some(instance);
            let Some(parent) = self.node(instance).parent else {
                break;
            };
            instance = parent;
            if self.is_propagated(instance) {
                instance = self.node(instance).origin.unwrap_or(instance);
            }
        }
        Hierarchy {
            instance,
            class_node,
        }
    }

    /// Namespace levels between where the node's arc was introduced and its
    /// parent's path (`GetDepthBelowIntroduction`).
    pub fn depth_below_introduction(&self, node: u32) -> i32 {
        match self.node(node).parent {
            Some(parent) => namespace_depth(&self.node(parent).path) - self.node(node).depth,
            None => 0,
        }
    }

    /// A specializes node copied under the root from its origin.
    pub fn is_propagated(&self, node: u32) -> bool {
        let entry = self.node(node);
        entry.arc == ArcType::Specialize
            && entry.parent == Some(0)
            && entry
                .origin
                .is_some_and(|origin| self.same_site(node, origin))
    }

    /// The propagated copy of specializes node `node`, if any.
    pub fn propagated_of(&self, node: u32) -> Option<u32> {
        if self.node(node).arc != ArcType::Specialize {
            return None;
        }
        for &child in self.nodes[0].children.iter().rev() {
            if self.node(child).arc < ArcType::Specialize {
                return None;
            }
            if self.node(child).origin == Some(node) && self.is_propagated(child) {
                return Some(child);
            }
        }
        None
    }

    /// Whether the node (or its propagated copy) has class-based children.
    pub fn has_class_based_child(&self, node: u32) -> bool {
        let owner = self.propagated_of(node).unwrap_or(node);
        self.node(owner)
            .children
            .iter()
            .any(|&child| self.node(child).arc.is_class_based())
    }

    /// Whether any node at or under `node` is a specializes node.
    pub fn has_specializes_below(&self, node: u32) -> bool {
        self.node(node).arc == ArcType::Specialize
            || self
                .node(node)
                .children
                .iter()
                .any(|&child| self.has_specializes_below(child))
    }

    /// Whether `ancestor` is `node` or above it, the root excluded.
    pub fn is_ancestor(&self, ancestor: u32, node: u32) -> bool {
        let mut at = Some(node);
        while let Some(index) = at {
            if index == 0 {
                return false;
            }
            if index == ancestor {
                return true;
            }
            at = self.node(index).parent;
        }
        false
    }

    /// A placeholder implied class under a relocation, only there to keep
    /// propagating (`_IsRelocatesPlaceholderImpliedArc`).
    pub fn relocates_placeholder(&self, node: u32) -> bool {
        let entry = self.node(node);
        let Some(parent) = entry.parent else {
            return false;
        };
        if Some(parent) == entry.origin {
            return false;
        }
        self.node(parent).arc == ArcType::Relocate && self.same_site(node, parent)
    }
}

#[cfg(test)]
mod tests {
    use super::super::graph::{ArcType, Node, PrimIndex};
    use super::super::map::MapFunction;
    use crate::usd::sdf::Path;

    fn child(index: &mut PrimIndex, arc: ArcType, depth: i32, sibling: i32) -> u32 {
        let mut node = Node::new(0, Path::parse("/X").unwrap(), arc, MapFunction::identity());
        node.origin = Some(0);
        node.depth = depth;
        node.sibling = sibling;
        index.insert_child(0, node)
    }

    #[test]
    fn siblings_order_by_livrps_then_depth_then_authoring() {
        let mut index = PrimIndex::rooted(0, Path::parse("/A").unwrap());
        let reference = child(&mut index, ArcType::Reference, 1, 0);
        let specialize = child(&mut index, ArcType::Specialize, 1, 0);
        let inherit = child(&mut index, ArcType::Inherit, 1, 0);
        let payload = child(&mut index, ArcType::Payload, 1, 0);
        let variant = child(&mut index, ArcType::Variant, 1, 0);
        let second_reference = child(&mut index, ArcType::Reference, 1, 1);
        let deeper_reference = child(&mut index, ArcType::Reference, 2, 5);
        assert_eq!(
            index.nodes[0].children,
            [
                inherit,
                variant,
                deeper_reference,
                reference,
                second_reference,
                payload,
                specialize
            ]
        );
        assert_eq!(
            index.order(false),
            [
                0,
                inherit,
                variant,
                deeper_reference,
                reference,
                second_reference,
                payload,
                specialize
            ]
        );
        assert!(index.compare_nodes(inherit, specialize) < 0);
    }
}
