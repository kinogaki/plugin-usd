// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Maps between namespaces, after `PcpMapFunction`
//! (`pxr/usd/pcp/mapFunction.cpp`): source to target path pairs, optionally
//! the root identity (paths no pair covers map to themselves), and a time
//! offset. A path maps through the pair whose source is its longest prefix;
//! a result that a pair with a longer target would also claim is ambiguous
//! and does not map. Composing f after g maps g's targets through f, then
//! adds f's pairs whose sources g's inverse reaches and g's pairs do not
//! already cover. A layer stack's relocations defer composition
//! (`DeferredComposition`): such a function is a chain of pair sets a path
//! goes through in turn, innermost first, folded into one set only where a
//! single set is needed.

use crate::usd::sdf::{LayerOffset, Path, PathKind};

/// One set of path pairs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PathPairs {
    /// Source paths.
    pub sources: Vec<Path>,
    /// Target paths, one per source.
    pub targets: Vec<Path>,
    /// Whether paths no pair covers map to themselves.
    pub root_identity: bool,
}

/// The index in `side` of the deepest path (at least `floor` deep) that is
/// a prefix of `path`, or `side.len()`.
fn best_match(path: &Path, side: &[Path], floor: u32) -> usize {
    let mut best = side.len();
    let mut best_depth = 0;
    for (at, candidate) in side.iter().enumerate() {
        if candidate.is_empty() {
            continue;
        }
        let depth = candidate.depth();
        if depth < floor {
            continue;
        }
        if (best == side.len() || depth > best_depth) && path.has_prefix(candidate) {
            best = at;
            best_depth = depth;
        }
    }
    best
}

impl PathPairs {
    /// No pairs.
    pub fn empty(root_identity: bool) -> PathPairs {
        PathPairs {
            sources: Vec::new(),
            targets: Vec::new(),
            root_identity,
        }
    }

    fn is_identity(&self) -> bool {
        self.root_identity && self.sources.is_empty()
    }

    fn push(&mut self, source: Path, target: Path) {
        self.sources.push(source);
        self.targets.push(target);
    }

    /// `path` mapped from the source namespace to the target one (target to
    /// source when `invert`); the empty path when it does not map.
    pub fn map(&self, path: &Path, invert: bool) -> Path {
        let (from, to) = if invert {
            (&self.targets, &self.sources)
        } else {
            (&self.sources, &self.targets)
        };
        let best = best_match(path, from, 0);
        let result = if best == from.len() {
            if self.root_identity {
                path.clone()
            } else {
                Path::empty()
            }
        } else {
            path.replace_prefix(&from[best], &to[best])
        };
        if result.is_empty() {
            return result;
        }
        // A pair with a longer target claiming the result makes it ambiguous.
        let floor = to.get(best).map_or(0, Path::depth);
        let better = best_match(&result, to, floor);
        if better < to.len() && better != best {
            return Path::empty();
        }
        result
    }

    /// `self` after `inner` (`_ComposePathPairs`).
    fn compose(&self, inner: &PathPairs) -> PathPairs {
        let mut made = PathPairs::empty(self.root_identity && inner.root_identity);
        for (source, target) in inner.sources.iter().zip(&inner.targets) {
            made.push(source.clone(), self.map(target, false));
        }
        let covered = made.sources.len();
        for (source, target) in self.sources.iter().zip(&self.targets) {
            let source = inner.map(source, true);
            if source.is_empty() || made.sources[..covered].contains(&source) {
                continue;
            }
            made.push(source, target.clone());
        }
        made
    }

    /// Whether two sets hold the same pairs, in any order.
    fn same_as(&self, other: &PathPairs) -> bool {
        self.root_identity == other.root_identity
            && self.sources.len() == other.sources.len()
            && self
                .sources
                .iter()
                .zip(&self.targets)
                .all(|(source, target)| {
                    other
                        .sources
                        .iter()
                        .zip(&other.targets)
                        .any(|(s, t)| s == source && t == target)
                })
    }

    fn inverted(&self) -> PathPairs {
        PathPairs {
            sources: self.targets.clone(),
            targets: self.sources.clone(),
            root_identity: self.root_identity,
        }
    }
}

/// A map from one namespace to another, with a time offset.
#[derive(Clone, Debug, PartialEq)]
pub struct MapFunction {
    /// The pairs, unless composition was deferred.
    pairs: PathPairs,
    /// A deferred composition's sets, outermost first (empty otherwise).
    chain: Vec<PathPairs>,
    /// The time offset.
    pub offset: LayerOffset,
}

impl MapFunction {
    /// The identity map.
    pub fn identity() -> MapFunction {
        MapFunction {
            pairs: PathPairs::empty(true),
            chain: Vec::new(),
            offset: LayerOffset::IDENTITY,
        }
    }

    /// The map of one pair; `/` to `/` is the root identity.
    pub fn of_pair(source: Path, target: Path, offset: LayerOffset) -> MapFunction {
        let mut pairs = PathPairs::empty(false);
        if source.is_absolute_root() && target.is_absolute_root() {
            pairs.root_identity = true;
        } else {
            pairs.push(source, target);
        }
        MapFunction {
            pairs,
            chain: Vec::new(),
            offset,
        }
    }

    /// The map of `pairs`, its composition deferred; the identity stays plain.
    pub fn deferred_of(pairs: PathPairs) -> MapFunction {
        if pairs.is_identity() {
            return MapFunction {
                pairs,
                chain: Vec::new(),
                offset: LayerOffset::IDENTITY,
            };
        }
        MapFunction {
            pairs: PathPairs::empty(true),
            chain: vec![pairs],
            offset: LayerOffset::IDENTITY,
        }
    }

    fn deferred(&self) -> bool {
        !self.chain.is_empty()
    }

    /// Every path maps to itself, with no offset.
    pub fn is_identity(&self) -> bool {
        self.is_identity_mapping() && self.offset.is_identity()
    }

    fn is_identity_mapping(&self) -> bool {
        if !self.deferred() {
            return self.pairs.is_identity();
        }
        self.chain.iter().all(PathPairs::is_identity)
    }

    /// Whether paths no pair covers map to themselves.
    pub fn has_root_identity(&self) -> bool {
        if !self.deferred() {
            return self.pairs.root_identity;
        }
        self.chain.iter().all(|set| set.root_identity)
    }

    /// `path` mapped from the source namespace to the target one (target to
    /// source when `invert`); the empty path when it does not map.
    pub fn map(&self, path: &Path, invert: bool) -> Path {
        if !self.deferred() {
            return self.pairs.map(path, invert);
        }
        let mut result = path.clone();
        let count = self.chain.len();
        for step in 0..count {
            // Inverted: outermost first; else innermost first.
            let set = &self.chain[if invert { step } else { count - 1 - step }];
            result = set.map(&result, invert);
            if result.is_empty() {
                break;
            }
        }
        result
    }

    /// `self` after `inner`: inner's source namespace to self's target one.
    pub fn compose(&self, inner: &MapFunction) -> MapFunction {
        if self.is_identity() {
            return inner.clone();
        }
        if inner.is_identity() {
            return self.clone();
        }
        let offset = self.offset.concatenate(&inner.offset);
        if !self.deferred() && !inner.deferred() {
            return MapFunction {
                pairs: self.pairs.compose(&inner.pairs),
                chain: Vec::new(),
                offset,
            };
        }
        let mut chain = Vec::new();
        for other in [self, inner] {
            if other.deferred() {
                chain.extend(other.chain.iter().cloned());
            } else {
                chain.push(other.pairs.clone());
            }
        }
        MapFunction {
            pairs: PathPairs::empty(true),
            chain,
            offset,
        }
    }

    /// The pairs a deferred chain folds into (`_GetNormalized`).
    pub fn normalized(&self) -> PathPairs {
        if !self.deferred() {
            return self.pairs.clone();
        }
        let mut result = self.chain[self.chain.len() - 1].clone();
        for set in self.chain[..self.chain.len() - 1].iter().rev() {
            result = set.compose(&result);
        }
        result
    }

    /// The same map with the root identity added (`AddRootIdentity`).
    pub fn with_root_identity(&self) -> MapFunction {
        if self.has_root_identity() {
            return self.clone();
        }
        let mut pairs = self.normalized();
        pairs.root_identity = true;
        MapFunction {
            pairs,
            chain: Vec::new(),
            offset: self.offset,
        }
    }

    /// The pairs with the root identity removed (payload inclusion skips an
    /// internal arc's root identity).
    pub fn without_root_identity(&self) -> PathPairs {
        let mut pairs = self.normalized();
        pairs.root_identity = false;
        pairs
    }

    /// The map the other way (the offset inverted too).
    pub fn inverse(&self) -> MapFunction {
        MapFunction {
            pairs: self.pairs.inverted(),
            chain: self.chain.iter().rev().map(PathPairs::inverted).collect(),
            offset: self.offset.inverse(),
        }
    }

    /// Whether two maps are the same once folded.
    pub fn equals(&self, other: &MapFunction) -> bool {
        self.offset == other.offset && self.normalized().same_as(&other.normalized())
    }

    /// The map of the class arc `class_arc` implied across `self`, the
    /// transfer from a node to its parent (`PcpMapFunction`'s implied class
    /// mapping): each class pair mapped through the transfer, plus the pairs
    /// the transfer gives descendants of class targets; always with the root
    /// identity, and the class arc's offset.
    pub fn implied_class(&self, class_arc: &MapFunction) -> MapFunction {
        if self.is_identity() || class_arc.is_identity() {
            return class_arc.clone();
        }
        let transfer = self.normalized();
        let arc = class_arc.normalized();
        let mut made = PathPairs::empty(true);
        for (source, target) in arc.sources.iter().zip(&arc.targets) {
            let mapped = transfer.map(source, false);
            if mapped.is_empty() {
                continue;
            }
            made.push(mapped, transfer.map(target, false));
        }
        for (transfer_source, transfer_target) in transfer.sources.iter().zip(&transfer.targets) {
            for (class_source, class_target) in arc.sources.iter().zip(&arc.targets) {
                if transfer_source == class_target || !transfer_source.has_prefix(class_target) {
                    continue;
                }
                let source = transfer_source.replace_prefix(class_target, class_source);
                if arc.map(&source, false) != *transfer_source {
                    continue;
                }
                let mapped = transfer.map(&source, false);
                if mapped.is_empty() {
                    continue;
                }
                made.push(mapped, transfer_target.clone());
            }
        }
        MapFunction {
            pairs: made,
            chain: Vec::new(),
            offset: class_arc.offset,
        }
    }
}

/// Path elements that are not variant selections
/// (`PcpNode_GetNonVariantPathElementCount`).
pub fn namespace_depth(path: &Path) -> i32 {
    path.ancestors()
        .filter(|at| !matches!(at.kind(), PathKind::Variant | PathKind::Root))
        .count() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Path {
        Path::parse(text).unwrap()
    }

    #[test]
    fn maps_through_the_longest_prefix() {
        let map = MapFunction::of_pair(p("/Model"), p("/Instance"), LayerOffset::IDENTITY);
        assert_eq!(
            map.map(&p("/Model/Geom.points"), false),
            p("/Instance/Geom.points")
        );
        assert!(map.map(&p("/Other"), false).is_empty());
        assert_eq!(map.map(&p("/Instance/A"), true), p("/Model/A"));
        let rooted = map.with_root_identity();
        assert_eq!(rooted.map(&p("/Other"), false), p("/Other"));
        // /Instance/A claimed by both the pair and the identity: the pair wins.
        assert_eq!(rooted.map(&p("/Model/A"), false), p("/Instance/A"));
        assert!(rooted.map(&p("/Instance/A"), false).is_empty());
    }

    #[test]
    fn composes_and_inverts() {
        let inner = MapFunction::of_pair(
            p("/A"),
            p("/B"),
            LayerOffset {
                offset: 10.0,
                scale: 1.0,
            },
        );
        let outer = MapFunction::of_pair(
            p("/B"),
            p("/C"),
            LayerOffset {
                offset: 0.0,
                scale: 2.0,
            },
        );
        let both = outer.compose(&inner);
        assert_eq!(both.map(&p("/A/x"), false), p("/C/x"));
        assert_eq!(
            both.offset,
            LayerOffset {
                offset: 20.0,
                scale: 2.0
            }
        );
        assert_eq!(both.inverse().map(&p("/C/y"), false), p("/A/y"));
        let deferred = MapFunction::deferred_of(outer.normalized()).compose(&inner);
        assert_eq!(deferred.map(&p("/A/x"), false), p("/C/x"));
        assert!(
            deferred.equals(&both.compose(&MapFunction::identity()))
                || deferred.normalized() == both.normalized()
        );
        assert_eq!(namespace_depth(&p("/A{v=x}B")), 2);
        assert!(MapFunction::identity().is_identity());
    }
}
