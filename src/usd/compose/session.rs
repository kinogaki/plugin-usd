// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Stage options authored before composing, as UsdStage does them: variant
//! selections go in the stage's session layer, the strongest opinion of the
//! root layer stack (`UsdVariantSet::SetVariantSelection` with the session
//! layer as edit target), and payloads can be limited by path
//! (`UsdStage::LoadAndUnload`'s rules).

use std::collections::BTreeMap;
use std::sync::Arc;

use super::composer::Composer;
use crate::usd::error::{Error, Result};
use crate::usd::sdf::{LayerData, Path, PathKind, SpecType, Specifier, Token, Value, field};

impl Composer {
    /// Select `selection` for variant set `set` of the prim at `prim`, in
    /// the session layer.
    pub fn select_variant(&mut self, prim: &Path, set: &str, selection: &str) -> Result<()> {
        if !prim.is_absolute() || !prim.is_prim() {
            return Err(Error::invalid(format!("{prim} is not a prim path")));
        }
        let Some(session) = self.stacks.stack(self.root_stack).session else {
            return Err(Error::invalid(
                "the stage has no session layer to author in",
            ));
        };
        let layer = self.layers.layer_mut(session);
        over_spec(layer, prim);
        let mut selections: BTreeMap<Token, Token> = match layer.get(prim, field::VARIANT_SELECTION)
        {
            Some(Value::VariantSelections(held)) => (**held).clone(),
            _ => BTreeMap::new(),
        };
        selections.insert(Token::new(set), Token::new(selection));
        layer.set(
            prim,
            field::VARIANT_SELECTION,
            Value::VariantSelections(Arc::new(selections)),
        );
        Ok(())
    }

    /// Author the variant selections a path with selections names
    /// (`/Kitchen{look=clean}`, `/Chair{size=big}{color=red}`).
    pub fn select_variants_at(&mut self, path: &Path) -> Result<()> {
        if path.kind() != PathKind::Variant {
            return Err(Error::invalid(format!(
                "{path} must end in {{set=variant}}"
            )));
        }
        let mut at = path.clone();
        while at.kind() == PathKind::Variant {
            let (set, selection) = at.variant_selection().expect("a variant selection");
            let (set, selection) = (set.clone(), selection.clone());
            self.select_variant(&at.strip_variants(), &set, &selection)?;
            at = at.parent();
        }
        Ok(())
    }

    /// Leave the payloads of prims whose path text starts with `prefix`
    /// unloaded.
    pub fn exclude_payloads(&mut self, prefix: &str) {
        self.unloaded.push(prefix.to_owned());
    }

    /// Load payloads only of prims whose path matches one of these globs
    /// (`*` any run, `?` any character; with several calls, any of them).
    pub fn include_payloads_matching(&mut self, pattern: &str) {
        self.loaded_matching.push(pattern.to_owned());
    }

    /// Whether every payload loads.
    pub(super) fn loads_all_payloads(&self) -> bool {
        self.unloaded.is_empty() && self.loaded_matching.is_empty()
    }

    /// Whether the payloads of the prim at `path` load.
    pub(super) fn payload_included(&self, path: &Path) -> bool {
        let text = path.text();
        if self
            .unloaded
            .iter()
            .any(|prefix| text.starts_with(prefix.as_str()))
        {
            return false;
        }
        self.loaded_matching.is_empty()
            || (self.loaded_matching)
                .iter()
                .any(|pattern| glob_matches(pattern, &text))
    }
}

/// The spec at prim `path` in `layer`, created as an over (with its
/// parents, each listed among its parent's children) when missing.
fn over_spec(layer: &mut LayerData, path: &Path) {
    if layer.has_spec(path) {
        return;
    }
    let parent = path.parent();
    if !parent.is_absolute_root() {
        over_spec(layer, &parent);
    }
    let spec = layer.create_spec(path.clone(), SpecType::Prim);
    spec.set(field::SPECIFIER, Value::Specifier(Specifier::Over));
    let parent_spec = layer.spec_mut(&parent).expect("the parent was just made");
    let mut children = parent_spec.tokens(field::PRIM_CHILDREN).to_vec();
    children.push(path.name_token());
    parent_spec.set(field::PRIM_CHILDREN, Value::token_vector(children));
}

/// Whether `text` matches glob `pattern` (`*` any run, `?` one character).
pub fn glob_matches(pattern: &str, text: &str) -> bool {
    let (pattern, text) = (pattern.as_bytes(), text.as_bytes());
    let (mut p, mut t) = (0, 0);
    let mut star = None;
    let mut resume = 0;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            resume = t;
            p += 1;
        } else if let Some(at) = star {
            p = at + 1;
            resume += 1;
            t = resume;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    #[test]
    fn globs_match() {
        assert!(super::glob_matches(
            "/Kitchen_set/Arch_grp*",
            "/Kitchen_set/Arch_grp/Wall"
        ));
        assert!(super::glob_matches("/A?C", "/ABC"));
        assert!(!super::glob_matches("/A*D", "/ABC"));
        assert!(!super::glob_matches("-", "/A"));
    }
}
