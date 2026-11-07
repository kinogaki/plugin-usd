// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Scene description paths, after OpenUSD's `SdfPath`.
//!
//! A [`Path`] is a chain of shared nodes from a root: `/A/B{set=sel}C.prop[/target].rel`.
//! Nodes share their parents, so `parent()` and `child()` are O(1) and
//! common prefixes are stored once. Each node carries its structural hash,
//! so paths hash in O(1) and compare in O(depth) at worst (shared parents
//! compare by pointer).

mod parse;

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock};

use super::token::Token;
use crate::usd::error::{Error, Result};

/// What a path's last element is (`Sdf_PathNode::NodeType`, in its order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PathKind {
    /// The empty path.
    Empty,
    /// `/` or `.`.
    Root,
    /// A prim name, or `..` in a relative path.
    Prim,
    /// A variant selection `{set=variant}`.
    Variant,
    /// `.name` after a prim or variant selection.
    Property,
    /// `[path]` after a property: a relationship target or connection.
    Target,
    /// `.mapper[path]` after a property.
    Mapper,
    /// `.name` after a target.
    Relational,
    /// `.name` after a mapper.
    MapperArg,
    /// `.expression` after a property.
    Expression,
}

#[derive(Debug)]
enum Element {
    Root,
    Name(Token),
    Variant(Token, Token),
    Target(Path),
    Expression,
}

#[derive(Debug)]
struct Node {
    parent: Path,
    kind: PathKind,
    element: Element,
    absolute: bool,
    depth: u32,
    hash: u64,
}

/// A scene description path; the empty path is the default.
#[derive(Clone, Default)]
pub struct Path(Option<Arc<Node>>);

/// The deepest path accepted, against hostile input.
pub(crate) const MOST_DEPTH: u32 = 4096;

static ABSOLUTE_ROOT: LazyLock<Path> = LazyLock::new(|| Path::root_node(true));
static RELATIVE_ROOT: LazyLock<Path> = LazyLock::new(|| Path::root_node(false));

fn mix(seed: u64, value: u64) -> u64 {
    let x = (seed ^ value).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^ (x >> 29)
}

fn text_hash(text: &str) -> u64 {
    // FNV-1a: stable across runs, so paths hash the same everywhere.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

impl Path {
    fn root_node(absolute: bool) -> Path {
        Path(Some(Arc::new(Node {
            parent: Path(None),
            kind: PathKind::Root,
            element: Element::Root,
            absolute,
            depth: 0,
            hash: if absolute { 1 } else { 2 },
        })))
    }

    /// The empty path.
    pub fn empty() -> Path {
        Path(None)
    }

    /// The absolute root, `/`.
    pub fn absolute_root() -> Path {
        ABSOLUTE_ROOT.clone()
    }

    /// The relative root, `.`.
    pub fn relative_root() -> Path {
        RELATIVE_ROOT.clone()
    }

    fn node(&self) -> Option<&Node> {
        self.0.as_deref()
    }

    fn append(&self, kind: PathKind, element: Element) -> Path {
        let parent = self.node().expect("appending to the empty path");
        let element_hash = match &element {
            Element::Root | Element::Expression => 0,
            Element::Name(name) => text_hash(name),
            Element::Variant(set, variant) => mix(text_hash(set), text_hash(variant)),
            Element::Target(path) => path.hash_value(),
        };
        let hash = mix(mix(parent.hash, kind as u64), element_hash);
        Path(Some(Arc::new(Node {
            parent: self.clone(),
            kind,
            element,
            absolute: parent.absolute,
            depth: parent.depth + 1,
            hash,
        })))
    }

    fn hash_value(&self) -> u64 {
        self.node().map_or(0, |node| node.hash)
    }

    /// The child prim `name` of a prim, variant selection or root path.
    pub fn child(&self, name: impl Into<Token>) -> Path {
        self.append(PathKind::Prim, Element::Name(name.into()))
    }

    /// The property `name` of a prim, variant selection or root path.
    pub fn property(&self, name: impl Into<Token>) -> Path {
        self.append(PathKind::Property, Element::Name(name.into()))
    }

    /// The variant selection `{set=variant}` of a prim path.
    pub fn variant(&self, set: impl Into<Token>, variant: impl Into<Token>) -> Path {
        self.append(
            PathKind::Variant,
            Element::Variant(set.into(), variant.into()),
        )
    }

    /// The target `[target]` of a property path.
    pub fn target(&self, target: Path) -> Path {
        self.append(PathKind::Target, Element::Target(target))
    }

    /// The relational attribute `name` of a target path.
    pub fn relational(&self, name: impl Into<Token>) -> Path {
        self.append(PathKind::Relational, Element::Name(name.into()))
    }

    /// The mapper `.mapper[target]` of a property path.
    pub fn mapper(&self, target: Path) -> Path {
        self.append(PathKind::Mapper, Element::Target(target))
    }

    /// The mapper argument `name` of a mapper path.
    pub fn mapper_arg(&self, name: impl Into<Token>) -> Path {
        self.append(PathKind::MapperArg, Element::Name(name.into()))
    }

    /// The `.expression` of a property path.
    pub fn expression(&self) -> Path {
        self.append(PathKind::Expression, Element::Expression)
    }

    /// What the last element is.
    pub fn kind(&self) -> PathKind {
        self.node().map_or(PathKind::Empty, |node| node.kind)
    }

    /// Whether this is the empty path.
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// Whether the path starts at `/`.
    pub fn is_absolute(&self) -> bool {
        self.node().is_some_and(|node| node.absolute)
    }

    /// Whether this is `/`.
    pub fn is_absolute_root(&self) -> bool {
        self.node()
            .is_some_and(|node| node.kind == PathKind::Root && node.absolute)
    }

    /// Whether this is `/` or `.`.
    pub fn is_root(&self) -> bool {
        self.kind() == PathKind::Root
    }

    /// Elements below the root (the roots are 0, the empty path 0).
    pub fn depth(&self) -> u32 {
        self.node().map_or(0, |node| node.depth)
    }

    /// A prim path: its last element names a prim (not `..`).
    pub fn is_prim(&self) -> bool {
        self.kind() == PathKind::Prim && self.name() != ".."
    }

    /// A prim or variant selection path, where prims are authored.
    pub fn is_prim_or_variant(&self) -> bool {
        matches!(self.kind(), PathKind::Prim | PathKind::Variant)
    }

    /// A property path (a prim's property or a relational attribute).
    pub fn is_property(&self) -> bool {
        matches!(self.kind(), PathKind::Property | PathKind::Relational)
    }

    /// The parent path; the empty path for a root or the empty path.
    pub fn parent(&self) -> Path {
        self.node()
            .map_or_else(Path::empty, |node| node.parent.clone())
    }

    /// The name of a prim, property, relational attribute or mapper
    /// argument, or the variant set of a variant selection; "" otherwise.
    pub fn name(&self) -> &str {
        match self.node().map(|node| &node.element) {
            Some(Element::Name(name)) => name,
            Some(Element::Variant(set, _)) => set,
            _ => "",
        }
    }

    /// The name as a token (see [`Path::name`]).
    pub fn name_token(&self) -> Token {
        match self.node().map(|node| &node.element) {
            Some(Element::Name(name)) => name.clone(),
            Some(Element::Variant(set, _)) => set.clone(),
            _ => Token::empty(),
        }
    }

    /// The `(set, variant)` of a variant selection path.
    pub fn variant_selection(&self) -> Option<(&Token, &Token)> {
        match self.node().map(|node| &node.element) {
            Some(Element::Variant(set, variant)) => Some((set, variant)),
            _ => None,
        }
    }

    /// The path in a target or mapper element.
    pub fn target_path(&self) -> Option<&Path> {
        match self.node().map(|node| &node.element) {
            Some(Element::Target(path)) => Some(path),
            _ => None,
        }
    }

    /// The path itself for a prim, variant or root path; else the prim (or
    /// variant selection) its property part hangs from.
    pub fn prim_path(&self) -> Path {
        let mut at = self.clone();
        while let Some(node) = at.node() {
            if matches!(
                node.kind,
                PathKind::Root | PathKind::Prim | PathKind::Variant
            ) {
                return at;
            }
            at = node.parent.clone();
        }
        at
    }

    /// Whether a variant selection appears anywhere in the path.
    pub fn has_variant_selection(&self) -> bool {
        self.ancestors()
            .any(|path| path.kind() == PathKind::Variant)
    }

    /// The path and its ancestors, deepest first, down to the root.
    pub fn ancestors(&self) -> impl Iterator<Item = Path> + '_ {
        let mut at = self.clone();
        std::iter::from_fn(move || {
            let node = at.node()?;
            let current = at.clone();
            at = node.parent.clone();
            Some(current)
        })
    }

    fn ancestor_at(&self, depth: u32) -> Path {
        let mut at = self.clone();
        while at.depth() > depth {
            at = at.parent();
        }
        at
    }

    /// Whether `prefix` is this path or one of its ancestors. The empty
    /// path is nobody's prefix.
    pub fn has_prefix(&self, prefix: &Path) -> bool {
        let (Some(have), Some(want)) = (self.node(), prefix.node()) else {
            return false;
        };
        if have.absolute != want.absolute || want.depth > have.depth {
            return false;
        }
        self.ancestor_at(want.depth) == *prefix
    }

    /// The elements of the path below `depth`, root-side first.
    fn chain_below(&self, depth: u32) -> Vec<&Node> {
        let mut chain = Vec::with_capacity((self.depth().saturating_sub(depth)) as usize);
        let mut at = self;
        while let Some(node) = at.node() {
            if node.depth <= depth {
                break;
            }
            chain.push(node);
            at = &node.parent;
        }
        chain.reverse();
        chain
    }

    fn with_element(base: &Path, node: &Node, element: Element) -> Path {
        base.append(node.kind, element)
    }

    fn same_element(node: &Node) -> Element {
        match &node.element {
            Element::Root => Element::Root,
            Element::Name(name) => Element::Name(name.clone()),
            Element::Variant(set, variant) => Element::Variant(set.clone(), variant.clone()),
            Element::Target(path) => Element::Target(path.clone()),
            Element::Expression => Element::Expression,
        }
    }

    /// The path with its prefix `prefix` replaced by `replacement`, and the
    /// same in every target path inside it; itself when nothing matches.
    pub fn replace_prefix(&self, prefix: &Path, replacement: &Path) -> Path {
        if self.is_empty() || prefix == replacement {
            return self.clone();
        }
        if self.has_prefix(prefix) {
            return self.rebuilt(prefix.depth(), replacement.clone(), prefix, replacement);
        }
        if !self.has_target() {
            return self.clone();
        }
        let root = if self.is_absolute() {
            Path::absolute_root()
        } else {
            Path::relative_root()
        };
        self.rebuilt(0, root, prefix, replacement)
    }

    fn has_target(&self) -> bool {
        self.ancestors()
            .any(|path| matches!(path.kind(), PathKind::Target | PathKind::Mapper))
    }

    fn rebuilt(&self, depth: u32, base: Path, prefix: &Path, replacement: &Path) -> Path {
        let mut at = base;
        for node in self.chain_below(depth) {
            let element = match &node.element {
                Element::Target(path) => Element::Target(path.replace_prefix(prefix, replacement)),
                _ => Self::same_element(node),
            };
            at = Self::with_element(&at, node, element);
        }
        at
    }

    /// The path with every variant selection removed, targets included.
    pub fn strip_variants(&self) -> Path {
        if !self.has_variant_selection() && !self.has_target() {
            return self.clone();
        }
        let Some(first) = self.node() else {
            return Path::empty();
        };
        let mut at = if first.absolute {
            Path::absolute_root()
        } else {
            Path::relative_root()
        };
        for node in self.chain_below(0) {
            match &node.element {
                Element::Variant(..) => continue,
                Element::Target(path) => {
                    at = Self::with_element(&at, node, Element::Target(path.strip_variants()))
                }
                _ => at = Self::with_element(&at, node, Self::same_element(node)),
            }
        }
        at
    }

    /// The path anchored at `anchor` (a prim, variant or root path) when it
    /// is relative: `..` steps to the parent prim. Target paths inside are
    /// anchored too. Fails when `..` climbs above the root.
    pub fn make_absolute(&self, anchor: &Path) -> Result<Path> {
        if self.is_empty() || self.is_absolute() {
            return Ok(self.clone());
        }
        if self.is_root() {
            return Ok(anchor.clone());
        }
        let mut at = anchor.clone();
        for node in self.chain_below(0) {
            match &node.element {
                Element::Name(name) if node.kind == PathKind::Prim && name == ".." => {
                    if at.is_root() || at.is_empty() {
                        return Err(Error::invalid(format!(
                            "the path {self} climbs above the root"
                        )));
                    }
                    at = at.prim_path().parent();
                }
                Element::Target(path) => {
                    at =
                        Self::with_element(&at, node, Element::Target(path.make_absolute(anchor)?));
                }
                _ => at = Self::with_element(&at, node, Self::same_element(node)),
            }
        }
        Ok(at)
    }

    /// The path relative to the prim (or variant, or root) path `anchor`,
    /// with the fewest `..` steps (`SdfPath::MakeRelativePath`).
    pub fn make_relative(&self, anchor: &Path) -> Result<Path> {
        if self.is_empty() {
            return Ok(Path::empty());
        }
        if !self.is_absolute() {
            return self.make_absolute(anchor)?.make_relative(anchor);
        }
        let prim = self.prim_path();
        let tail = self.chain_below(prim.depth());
        let mut this = prim;
        let mut base = anchor.clone();
        let mut kept = Vec::new();
        let mut dot_dots = 0;
        while this.depth() > base.depth() {
            kept.push(this.clone());
            this = this.parent();
        }
        while this.depth() < base.depth() {
            dot_dots += 1;
            base = base.parent();
        }
        while this != base {
            dot_dots += 1;
            kept.push(this.clone());
            this = this.parent();
            base = base.parent();
        }
        let mut result = Path::relative_root();
        for _ in 0..dot_dots {
            result = result.child("..");
        }
        for path in kept.iter().rev() {
            let node = path.node().expect("a kept element");
            result = Self::with_element(&result, node, Self::same_element(node));
        }
        for node in tail {
            result = Self::with_element(&result, node, Self::same_element(node));
        }
        Ok(result)
    }

    /// Write the path's text (as `SdfPath::GetString`), root first: a loop
    /// over its nodes, so a path of any depth writes on any stack.
    fn write_text(&self, out: &mut String) {
        let Some(node) = self.node() else { return };
        if node.kind == PathKind::Root {
            out.push(if node.absolute { '/' } else { '.' });
            return;
        }
        let mut chain = Vec::with_capacity(node.depth as usize);
        let mut at = Some(node);
        while let Some(node) = at.filter(|n| n.kind != PathKind::Root) {
            chain.push(node);
            at = node.parent.node();
        }
        for node in chain.into_iter().rev() {
            let parent = &node.parent;
            // Every element but a prim's name follows its parent as written;
            // a prim's name follows a prim with a slash.
            let prim_in_prim = node.kind == PathKind::Prim && parent.kind() == PathKind::Prim;
            if parent.is_absolute_root() || prim_in_prim {
                out.push('/');
            }
            match &node.element {
                Element::Name(name) if node.kind == PathKind::Prim => out.push_str(name),
                Element::Variant(set, variant) => {
                    out.push('{');
                    out.push_str(set);
                    out.push('=');
                    out.push_str(variant);
                    out.push('}');
                }
                Element::Name(name) => {
                    // `../.property`: a property of a parent step keeps a slash.
                    if node.kind == PathKind::Property
                        && parent.kind() == PathKind::Prim
                        && parent.name() == ".."
                    {
                        out.push('/');
                    }
                    out.push('.');
                    out.push_str(name);
                }
                Element::Target(path) => {
                    out.push_str(if node.kind == PathKind::Mapper {
                        ".mapper["
                    } else {
                        "["
                    });
                    path.write_text(out);
                    out.push(']');
                }
                Element::Expression => out.push_str(".expression"),
                Element::Root => {}
            }
        }
    }

    /// The path's text.
    pub fn text(&self) -> String {
        let mut out = String::with_capacity(32);
        self.write_text(&mut out);
        out
    }
}

impl PartialEq for Path {
    /// Node by node toward the root, until two share their parents: a loop,
    /// so paths of any depth compare on any stack.
    fn eq(&self, other: &Path) -> bool {
        let (mut left, mut right) = (self, other);
        loop {
            match (&left.0, &right.0) {
                (None, None) => return true,
                (Some(a), Some(b)) => {
                    if Arc::ptr_eq(a, b) {
                        return true;
                    }
                    let same = a.hash == b.hash
                        && a.depth == b.depth
                        && a.kind == b.kind
                        && a.absolute == b.absolute
                        && match (&a.element, &b.element) {
                            (Element::Root, Element::Root)
                            | (Element::Expression, Element::Expression) => true,
                            (Element::Name(x), Element::Name(y)) => x == y,
                            (Element::Variant(s, v), Element::Variant(t, w)) => s == t && v == w,
                            (Element::Target(x), Element::Target(y)) => x == y,
                            _ => false,
                        };
                    if !same {
                        return false;
                    }
                    (left, right) = (&a.parent, &b.parent);
                }
                _ => return false,
            }
        }
    }
}

/// A node frees the chain of parents only it held one at a time, so a path
/// of any depth drops on any stack.
impl Drop for Node {
    fn drop(&mut self) {
        let mut parent = std::mem::take(&mut self.parent);
        while let Some(shared) = parent.0.take() {
            match Arc::try_unwrap(shared) {
                Ok(mut node) => parent = std::mem::take(&mut node.parent),
                Err(_) => return,
            }
        }
    }
}

impl Eq for Path {}

/// A hasher for paths: their structural hash is computed once, when the
/// path is made, so maps keyed by paths pass it through rather than
/// rehashing it.
#[derive(Clone, Copy, Debug, Default)]
pub struct PathHasher(u64);

impl Hasher for PathHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        // Only paths (one u64 each) are expected; mix anything else in.
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = mix(self.0, value);
    }
}

/// A map keyed by paths with the pass-through [`PathHasher`].
pub type PathMap<V> = std::collections::HashMap<Path, V, std::hash::BuildHasherDefault<PathHasher>>;

impl Hash for Path {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash_value());
    }
}

impl Ord for Path {
    /// `SdfPath`'s order: absolute before relative, the prim part before
    /// the property part, then element by element from the root (kinds in
    /// node type order, names by bytes), a prefix first.
    fn cmp(&self, other: &Path) -> Ordering {
        if self == other {
            return Ordering::Equal;
        }
        match (self.is_empty(), other.is_empty()) {
            (true, _) => return Ordering::Less,
            (_, true) => return Ordering::Greater,
            _ => {}
        }
        if self.is_absolute() != other.is_absolute() {
            return if self.is_absolute() {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let (left_prim, right_prim) = (self.prim_path(), other.prim_path());
        if left_prim != right_prim {
            return chain_cmp(&left_prim, &right_prim, 0);
        }
        chain_cmp(self, other, left_prim.depth())
    }
}

impl PartialOrd for Path {
    fn partial_cmp(&self, other: &Path) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn chain_cmp(left: &Path, right: &Path, base: u32) -> Ordering {
    let (l_depth, r_depth) = (left.depth(), right.depth());
    if l_depth == base || r_depth == base {
        return l_depth.cmp(&r_depth);
    }
    let mut l = left.ancestor_at(r_depth.min(l_depth));
    let mut r = right.ancestor_at(r_depth.min(l_depth));
    if l == r {
        return l_depth.cmp(&r_depth);
    }
    while l.parent() != r.parent() {
        l = l.parent();
        r = r.parent();
    }
    node_cmp(&l, &r)
}

fn node_cmp(left: &Path, right: &Path) -> Ordering {
    let (a, b) = (left.node().expect("a node"), right.node().expect("a node"));
    if a.kind != b.kind {
        return a.kind.cmp(&b.kind);
    }
    match (&a.element, &b.element) {
        (Element::Name(x), Element::Name(y)) => x.as_bytes().cmp(y.as_bytes()),
        (Element::Variant(s, v), Element::Variant(t, w)) => s
            .as_bytes()
            .cmp(t.as_bytes())
            .then_with(|| v.as_bytes().cmp(w.as_bytes())),
        (Element::Target(x), Element::Target(y)) => x.cmp(y),
        _ => Ordering::Equal,
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

impl fmt::Debug for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{}>", self.text())
    }
}
