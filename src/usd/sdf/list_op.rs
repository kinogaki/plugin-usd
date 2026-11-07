// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! List ops, after `SdfListOp` (`pxr/usd/sdf/listOp.cpp`): an explicit
//! list, or edits (delete, add, prepend, append, reorder) applied to a
//! weaker opinion's list.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash, RandomState};

/// One part of a list op.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListPart {
    /// The whole list.
    Explicit,
    /// Added if absent (legacy `add`).
    Added,
    /// Moved or inserted at the front.
    Prepended,
    /// Moved or inserted at the back.
    Appended,
    /// Removed.
    Deleted,
    /// Reordered (legacy `reorder`).
    Ordered,
}

impl ListPart {
    /// The edit parts in the order usda writes them.
    pub const EDITS: [ListPart; 5] = [
        ListPart::Deleted,
        ListPart::Added,
        ListPart::Prepended,
        ListPart::Appended,
        ListPart::Ordered,
    ];

    /// The usda keyword of an edit part ("" for explicit).
    pub fn keyword(self) -> &'static str {
        match self {
            ListPart::Explicit => "",
            ListPart::Added => "add",
            ListPart::Prepended => "prepend",
            ListPart::Appended => "append",
            ListPart::Deleted => "delete",
            ListPart::Ordered => "reorder",
        }
    }
}

/// A list op over items of `T`.
#[derive(Clone, Debug, PartialEq)]
pub struct ListOp<T> {
    /// Whether the list is explicit (the explicit part replaces everything).
    pub explicit: bool,
    /// The explicit items.
    pub explicit_items: Vec<T>,
    /// Added items.
    pub added: Vec<T>,
    /// Prepended items.
    pub prepended: Vec<T>,
    /// Appended items.
    pub appended: Vec<T>,
    /// Deleted items.
    pub deleted: Vec<T>,
    /// The reorder list.
    pub ordered: Vec<T>,
}

impl<T> Default for ListOp<T> {
    fn default() -> Self {
        ListOp {
            explicit: false,
            explicit_items: Vec::new(),
            added: Vec::new(),
            prepended: Vec::new(),
            appended: Vec::new(),
            deleted: Vec::new(),
            ordered: Vec::new(),
        }
    }
}

impl<T: Clone + PartialEq> ListOp<T> {
    /// An explicit list of `items`.
    pub fn explicit(items: Vec<T>) -> Self {
        ListOp {
            explicit: true,
            explicit_items: items,
            ..ListOp::default()
        }
    }

    /// The items of one part.
    pub fn items(&self, part: ListPart) -> &[T] {
        match part {
            ListPart::Explicit => &self.explicit_items,
            ListPart::Added => &self.added,
            ListPart::Prepended => &self.prepended,
            ListPart::Appended => &self.appended,
            ListPart::Deleted => &self.deleted,
            ListPart::Ordered => &self.ordered,
        }
    }

    /// The items of one part, to change.
    pub fn items_mut(&mut self, part: ListPart) -> &mut Vec<T> {
        match part {
            ListPart::Explicit => &mut self.explicit_items,
            ListPart::Added => &mut self.added,
            ListPart::Prepended => &mut self.prepended,
            ListPart::Appended => &mut self.appended,
            ListPart::Deleted => &mut self.deleted,
            ListPart::Ordered => &mut self.ordered,
        }
    }

    /// Set one part (`SdfListOp::SetItems`): setting the explicit part makes
    /// the list explicit and clears the edits; setting an edit part of an
    /// explicit list clears it first.
    pub fn set(&mut self, part: ListPart, items: Vec<T>) {
        if part == ListPart::Explicit {
            *self = ListOp::explicit(items);
            return;
        }
        if self.explicit {
            *self = ListOp::default();
        }
        *self.items_mut(part) = items;
    }

    /// Whether the list op edits anything (an explicit list always does).
    pub fn has_edits(&self) -> bool {
        self.explicit
            || !(self.added.is_empty()
                && self.prepended.is_empty()
                && self.appended.is_empty()
                && self.deleted.is_empty()
                && self.ordered.is_empty())
    }

    /// Whether any part holds items.
    pub fn has_items(&self) -> bool {
        !(self.explicit_items.is_empty()
            && self.added.is_empty()
            && self.prepended.is_empty()
            && self.appended.is_empty()
            && self.deleted.is_empty()
            && self.ordered.is_empty())
    }
}

impl<T: Clone + PartialEq + Hash> ListOp<T> {
    /// Apply the list op to `items` in place (`SdfListOp::ApplyOperations`):
    /// an explicit list replaces them; else deleted items go, added ones
    /// join at the back unless there, prepended ones move or join at the
    /// front and appended ones at the back, each once, then the reorder.
    /// Work in proportion to the items (each found by its hash).
    pub fn apply(&self, items: &mut Vec<T>) {
        if self.explicit {
            *items = unique(&self.explicit_items);
            return;
        }
        if !self.deleted.is_empty() {
            let deleted = Found::of(&self.deleted);
            items.retain(|x| !deleted.contains(x));
        }
        if !self.added.is_empty() {
            let mut held = Found::of(items.iter());
            let added: Vec<T> = (self.added.iter())
                .filter(|&x| held.insert(x, 0))
                .cloned()
                .collect();
            items.extend(added);
        }
        if !self.prepended.is_empty() {
            let front = unique(&self.prepended);
            let moved = Found::of(&front);
            items.retain(|x| !moved.contains(x));
            items.splice(0..0, front.iter().cloned());
        }
        if !self.appended.is_empty() {
            // Each at its last place in the list.
            let mut back = unique(self.appended.iter().rev());
            back.reverse();
            let moved = Found::of(&back);
            items.retain(|x| !moved.contains(x));
            items.extend(back.iter().cloned());
        }
        reorder(items, &self.ordered);
    }

    /// The list op applied to nothing.
    pub fn applied(&self) -> Vec<T> {
        let mut items = Vec::new();
        self.apply(&mut items);
        items
    }

    /// `self` composed over the weaker `weak` (`SdfListOp::ApplyOperations`
    /// on list ops): an explicit strong list wins; deletes, prepends and
    /// appends edit an explicit weak list into an explicit list or merge
    /// with weak edits. None when either uses added or reordered items.
    pub fn compose_over(&self, weak: &ListOp<T>) -> Option<ListOp<T>> {
        if self.explicit {
            return Some(ListOp::explicit(self.explicit_items.clone()));
        }
        if !self.added.is_empty() || !self.ordered.is_empty() {
            return None;
        }
        if weak.explicit {
            let mut items = weak.explicit_items.clone();
            self.apply(&mut items);
            return Some(ListOp::explicit(items));
        }
        if !weak.added.is_empty() || !weak.ordered.is_empty() {
            return None;
        }
        // The weak edits less what the strong ones delete or move, the
        // strong deletes joining the weak ones.
        let gone = Found::of(&self.deleted);
        let moved = Found::of(self.prepended.iter().chain(&self.appended));
        let kept = |x: &&T| !gone.contains(x) && !moved.contains(x);
        let mut said = Found::of(&weak.deleted);
        let deleted = (weak.deleted.iter())
            .chain(self.deleted.iter().filter(|&x| said.insert(x, 0)))
            .filter(|x| !moved.contains(x));
        Some(ListOp {
            deleted: deleted.cloned().collect(),
            prepended: (self.prepended.iter())
                .chain(weak.prepended.iter().filter(kept))
                .cloned()
                .collect(),
            appended: (weak.appended.iter().filter(kept))
                .chain(&self.appended)
                .cloned()
                .collect(),
            ..ListOp::default()
        })
    }
}

/// Whether an item appears twice in `items`.
pub fn has_duplicates<T: PartialEq + Hash>(items: &[T]) -> bool {
    let mut seen = Found::default();
    !items.iter().all(|x| seen.insert(x, 0))
}

/// `items`, each at its first place.
fn unique<'a, T: Clone + PartialEq + Hash + 'a>(items: impl IntoIterator<Item = &'a T>) -> Vec<T> {
    let mut seen = Found::default();
    (items.into_iter())
        .filter(|&x| seen.insert(x, 0))
        .cloned()
        .collect()
}

/// Items found by their hash and compared exactly, each with where it was
/// first met. An item type whose equality holds floats hashes what compares
/// exactly ([`Reference`](super::Reference)'s custom data is compared alone).
struct Found<'a, T> {
    hasher: RandomState,
    first: HashMap<u64, Vec<(&'a T, usize)>>,
}

impl<T> Default for Found<'_, T> {
    fn default() -> Self {
        Found {
            hasher: RandomState::new(),
            first: HashMap::new(),
        }
    }
}

impl<'a, T: PartialEq + Hash> Found<'a, T> {
    /// `items`, each where it was first met.
    fn of(items: impl IntoIterator<Item = &'a T>) -> Self {
        let mut found = Found::default();
        for (at, item) in items.into_iter().enumerate() {
            found.insert(item, at);
        }
        found
    }

    /// Where `item` was first met.
    fn position(&self, item: &T) -> Option<usize> {
        let bucket = self.first.get(&self.hasher.hash_one(item))?;
        bucket.iter().find(|(x, _)| *x == item).map(|(_, at)| *at)
    }

    fn contains(&self, item: &T) -> bool {
        self.position(item).is_some()
    }

    /// `item`, met at `at`; whether it is new.
    fn insert(&mut self, item: &'a T, at: usize) -> bool {
        let bucket = self.first.entry(self.hasher.hash_one(item)).or_default();
        let new = !bucket.iter().any(|(x, _)| *x == item);
        if new {
            bucket.push((item, at));
        }
        new
    }
}

/// `SdfListOp`'s reorder: each ordered item present starts a run that takes
/// the items after it up to the next ordered one; runs follow the order, and
/// items before any ordered item stay first.
fn reorder<T: Clone + PartialEq + Hash>(items: &mut Vec<T>, order: &[T]) {
    if order.is_empty() || items.is_empty() {
        return;
    }
    let ordered = Found::of(order);
    let scratch = std::mem::take(items);
    let at = Found::of(&scratch);
    let mut taken = vec![false; scratch.len()];
    let mut tail = Vec::new();
    let firsts = (order.iter().enumerate()).filter(|(k, x)| ordered.position(x) == Some(*k));
    for (_, item) in firsts {
        let Some(start) = at.position(item) else {
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
    for (index, item) in scratch.iter().enumerate() {
        if !taken[index] {
            items.push(item.clone());
        }
    }
    items.extend(tail);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(parts: &[(ListPart, &[i32])]) -> ListOp<i32> {
        let mut list = ListOp::default();
        for (part, items) in parts {
            list.set(*part, items.to_vec());
        }
        list
    }

    #[test]
    fn applies_edits_in_sdf_order() {
        let mut items = vec![1, 2, 3, 4];
        op(&[
            (ListPart::Deleted, &[2]),
            (ListPart::Prepended, &[4, 9]),
            (ListPart::Appended, &[1]),
        ])
        .apply(&mut items);
        assert_eq!(items, [4, 9, 3, 1]);
        let mut items = vec![1, 2, 3, 4, 5];
        op(&[(ListPart::Ordered, &[4, 2])]).apply(&mut items);
        assert_eq!(items, [1, 4, 5, 2, 3]);
        assert_eq!(op(&[(ListPart::Explicit, &[3, 3, 1])]).applied(), [3, 1]);
    }

    #[test]
    fn composes_over_weaker_lists() {
        let strong = op(&[(ListPart::Prepended, &[1]), (ListPart::Deleted, &[5])]);
        let weak = op(&[(ListPart::Explicit, &[5, 6])]);
        assert_eq!(
            strong.compose_over(&weak).unwrap(),
            ListOp::explicit(vec![1, 6])
        );
        let weak = op(&[(ListPart::Appended, &[5, 7])]);
        let made = strong.compose_over(&weak).unwrap();
        assert_eq!(
            (made.prepended, made.appended, made.deleted),
            (vec![1], vec![7], vec![5])
        );
        assert!(op(&[(ListPart::Added, &[1])]).compose_over(&weak).is_none());
    }
}
