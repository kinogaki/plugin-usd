//! Material bindings as UsdShade resolves them
//! (`UsdShadeMaterialBindingAPI::ComputeBoundMaterial`,
//! materialBindingAPI.cpp:701-845): direct bindings and collection-based
//! ones (`material:binding:collection:<name>`, its two targets a
//! collection and a material), of the full purpose and of all purposes.
//!
//! A prim's material is the full purpose's binding when one reaches it,
//! else the all-purpose one. For a purpose, the prim and its ancestors are
//! met from the prim up: the first binding met wins unless an ancestor's is
//! `strongerThanDescendants`; at one prim, a collection binding including
//! the prim beats the direct one, the first of them in property order
//! winning. A collection's members are UsdCollectionAPI's: its `includes`
//! and `excludes` (a collection among them brings its own), expanded to
//! the prims beneath unless `explicitOnly`, the most specific rule winning;
//! `includeRoot` takes in everything.
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::usd::sdf::{LayerData, Path, Value, field};

/// The full purpose's bindings, then all purposes'.
pub(super) const FULL: usize = 0;
pub(super) const ALL: usize = 1;

/// A material binding: the material, and whether it overrides the
/// bindings of descendants (`bindMaterialAs = "strongerThanDescendants"`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Binding {
    pub material: usize,
    pub stronger: bool,
}

/// The bindings authored on one prim, by purpose ([`FULL`], [`ALL`]): its
/// direct one, and its collection-based ones in property order.
#[derive(Default)]
pub(super) struct Level {
    pub direct: [Option<Binding>; 2],
    pub collections: [Vec<(Rc<Membership>, Binding)>; 2],
}

/// What a binding relationship's name says: its purpose, and whether it
/// binds a collection; none for one of another purpose (`preview`) or no
/// binding.
pub(super) fn binding_name(name: &str) -> Option<(usize, bool)> {
    let rest = name.strip_prefix("material:binding")?;
    match rest {
        "" => Some((ALL, false)),
        ":full" => Some((FULL, false)),
        _ => {
            let named = rest.strip_prefix(":collection:")?;
            match named.split(':').collect::<Vec<_>>()[..] {
                [_] => Some((ALL, true)),
                ["full", _] => Some((FULL, true)),
                _ => None,
            }
        }
    }
}

/// Whether the binding relationship at `relationship` is
/// `strongerThanDescendants`.
pub(super) fn stronger(layer: &LayerData, relationship: &Path) -> bool {
    matches!(
        layer.get(relationship, "bindMaterialAs"),
        Some(Value::Token(t)) if t.as_str() == "strongerThanDescendants"
    )
}

/// The material bound to the prim at `path` beneath `levels` (outermost
/// first, the prim's own last).
pub(super) fn resolve(levels: &[Level], path: &Path) -> Option<Binding> {
    for purpose in [FULL, ALL] {
        // The winner, and the level it is on.
        let mut won: Option<(Binding, usize)> = None;
        for (at, level) in levels.iter().enumerate().rev() {
            if let Some(direct) = level.direct[purpose]
                && (won.is_none() || direct.stronger)
            {
                won = Some((direct, at));
            }
            let here = won.is_none_or(|(_, on)| on == at);
            let includes = level.collections[purpose]
                .iter()
                .find(|(members, binding)| members.includes(path) && (here || binding.stronger));
            if let Some((_, binding)) = includes {
                won = Some((*binding, at));
            }
        }
        if let Some((binding, _)) = won {
            return Some(binding);
        }
    }
    None
}

/// A collection's members: each rule's path and whether it includes, and
/// whether only the paths named are members.
#[derive(Debug, Default)]
pub(super) struct Membership {
    rules: HashMap<Path, bool>,
    explicit: bool,
}

impl Membership {
    /// Whether the prim at `path` is a member: named (or beneath one named,
    /// expanded), by the most specific rule.
    pub(super) fn includes(&self, path: &Path) -> bool {
        if self.explicit {
            return self.rules.get(path) == Some(&true);
        }
        let named = path.ancestors().find_map(|a| self.rules.get(&a).copied());
        named
            .or_else(|| self.rules.get(&Path::absolute_root()).copied())
            .unwrap_or(false)
    }
}

/// Each collection's members, made once; whether a membership expression
/// was said.
#[derive(Default)]
pub(super) struct Memberships {
    known: HashMap<Path, Rc<Membership>>,
    said: bool,
}

impl Memberships {
    /// The members of the collection at `collection` (`/Prim.collection:
    /// name`), a membership expression said once and left out.
    pub(super) fn of(
        &mut self,
        layer: &LayerData,
        collection: &Path,
        warnings: &mut Vec<String>,
    ) -> Rc<Membership> {
        if let Some(known) = self.known.get(collection) {
            return known.clone();
        }
        let mut membership = Membership::default();
        let mut seen = HashSet::new();
        let expression = rules(layer, collection, &mut membership, &mut seen);
        if expression && !self.said {
            self.said = true;
            warnings.push(format!(
                "{collection}: collection membership expressions are not read"
            ));
        }
        let made = Rc::new(membership);
        self.known.insert(collection.clone(), made.clone());
        made
    }
}

/// `membership` with the rules of the collection at `collection` (those of
/// the collections it includes first, its own over them), its expansion
/// its own; whether one holds a membership expression.
fn rules(
    layer: &LayerData,
    collection: &Path,
    membership: &mut Membership,
    seen: &mut HashSet<Path>,
) -> bool {
    if !collection.is_property() || !seen.insert(collection.clone()) {
        return false;
    }
    let (prim, name) = (collection.prim_path(), collection.name());
    let at = |what: &str| prim.property(format!("{name}:{what}"));
    let targets = |what: &str| match layer.get(&at(what), field::TARGET_PATHS) {
        Some(Value::PathListOp(list)) => list.applied(),
        _ => Vec::new(),
    };
    let top = seen.len() == 1;
    if top {
        let rule = layer.get(&at("expansionRule"), field::DEFAULT);
        membership.explicit = matches!(rule, Some(Value::Token(t)) if t.as_str() == "explicitOnly");
    }
    let mut expression = layer.spec(&at("membershipExpression")).is_some();
    let (includes, excludes) = (targets("includes"), targets("excludes"));
    for nested in includes.iter().filter(|t| t.is_property()) {
        expression |= rules(layer, nested, membership, seen);
    }
    for path in includes.into_iter().filter(Path::is_prim) {
        membership.rules.insert(path, true);
    }
    for path in excludes {
        membership.rules.insert(path, false);
    }
    if matches!(
        layer.get(&at("includeRoot"), field::DEFAULT),
        Some(Value::Bool(true))
    ) {
        membership.rules.insert(Path::absolute_root(), true);
    }
    expression
}
