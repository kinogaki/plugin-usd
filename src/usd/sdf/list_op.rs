// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! List ops, after `SdfListOp` (`pxr/usd/sdf/listOp.cpp`): an explicit
//! list, or edits (delete, add, prepend, append, reorder) applied to a
//! weaker opinion's list.

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

    /// Apply the list op to `items` in place (`SdfListOp::ApplyOperations`).
    pub fn apply(&self, items: &mut Vec<T>) {
        if self.explicit {
            items.clear();
            for item in &self.explicit_items {
                if !items.contains(item) {
                    items.push(item.clone());
                }
            }
            return;
        }
        for item in &self.deleted {
            if let Some(at) = items.iter().position(|x| x == item) {
                items.remove(at);
            }
        }
        for item in &self.added {
            if !items.contains(item) {
                items.push(item.clone());
            }
        }
        for item in self.prepended.iter().rev() {
            if let Some(at) = items.iter().position(|x| x == item) {
                items.remove(at);
            }
            items.insert(0, item.clone());
        }
        for item in &self.appended {
            if let Some(at) = items.iter().position(|x| x == item) {
                items.remove(at);
            }
            items.push(item.clone());
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
        let mut deleted = weak.deleted.clone();
        let mut prepended = weak.prepended.clone();
        let mut appended = weak.appended.clone();
        for item in &self.deleted {
            prepended.retain(|x| x != item);
            appended.retain(|x| x != item);
            if !deleted.contains(item) {
                deleted.push(item.clone());
            }
        }
        for item in self.prepended.iter().chain(&self.appended) {
            deleted.retain(|x| x != item);
            prepended.retain(|x| x != item);
            appended.retain(|x| x != item);
        }
        let mut made = ListOp {
            deleted,
            ..ListOp::default()
        };
        made.prepended = self.prepended.iter().cloned().chain(prepended).collect();
        made.appended = appended
            .into_iter()
            .chain(self.appended.iter().cloned())
            .collect();
        Some(made)
    }
}

/// `SdfListOp`'s reorder: each ordered item present starts a run that takes
/// the items after it up to the next ordered one; runs follow the order, and
/// items before any ordered item stay first.
fn reorder<T: Clone + PartialEq>(items: &mut Vec<T>, order: &[T]) {
    if order.is_empty() || items.is_empty() {
        return;
    }
    let mut unique: Vec<&T> = Vec::new();
    for item in order {
        if !unique.contains(&item) {
            unique.push(item);
        }
    }
    let scratch = std::mem::take(items);
    let mut taken = vec![false; scratch.len()];
    let mut tail = Vec::new();
    for item in unique.iter().copied() {
        let Some(start) = scratch.iter().position(|x| x == item) else {
            continue;
        };
        let mut end = start;
        loop {
            tail.push(scratch[end].clone());
            taken[end] = true;
            end += 1;
            if end >= scratch.len() || unique.contains(&&scratch[end]) {
                break;
            }
        }
    }
    for (index, item) in scratch.into_iter().enumerate() {
        if !taken[index] {
            items.push(item);
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
