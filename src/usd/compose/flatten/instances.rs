// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Instances in a flattened stage, after `Usd_InstanceCache` and
//! `UsdStage::Flatten` (`pxr/usd/usd/instanceCache.cpp`, `stage.cpp`).
//! Instances with the same key share a prototype, whose source is one of
//! them; the source's descendants are searched too, for nested instances.
//! Prototypes are numbered in their sources' path order (as with
//! `USD_ASSIGN_PROTOTYPES_DETERMINISTICALLY`). Referenced, they are written
//! first, as `/Flattened_Prototype_N` in the text order of their
//! `/__Prototype_M` paths, N passing over the names the stage's root prims
//! take (`_GenerateFlattenedPrototypePath`): an over holding the source's
//! children, which instances reference. At their sources, a prototype's
//! prims are its source's own, written where they are.

use std::collections::HashSet;
use std::sync::Arc;

use super::{Flattener, Prototype};
use crate::usd::error::Result;
use crate::usd::sdf::{
    LayerOffset, ListOp, Path, Reference, SpecType, Specifier, Token, Value, field,
};

impl Flattener<'_> {
    /// Find the stage's instances: the stage's prims down to instances,
    /// then each new prototype's source's descendants, where nested
    /// instances are. Prototypes are numbered (`__Prototype_N`) in their
    /// sources' path order (as with `USD_ASSIGN_PROTOTYPES_DETERMINISTICALLY`;
    /// OpenUSD's own choice among instances of one depth is a race in its
    /// parallel population).
    pub(super) fn populate(&mut self, roots: &[Path]) -> Result<()> {
        for root in roots {
            self.walk(root)?;
        }
        let mut at = 0;
        while at < self.prototypes.len() {
            let source = self.prototype_source(at);
            self.prototypes[at].source = source.clone();
            let index = self.composer.index(&source)?;
            for name in self.composer.child_names(&index) {
                self.walk(&source.child(name))?;
            }
            at += 1;
        }
        let mut by_source: Vec<usize> = (0..self.prototypes.len()).collect();
        by_source.sort_by(|&a, &b| self.prototypes[a].source.cmp(&self.prototypes[b].source));
        for (rank, prototype) in by_source.into_iter().enumerate() {
            self.prototypes[prototype].number = rank + 1;
        }
        Ok(())
    }

    /// The source of a prototype: its shallowest instance, then the first
    /// in path order.
    fn prototype_source(&self, prototype: usize) -> Path {
        let mut source = self.prototypes[prototype].source.clone();
        for (instance, &of) in &self.instance_of {
            if of != prototype {
                continue;
            }
            let (depth, known) = (instance.depth(), source.depth());
            if depth < known || (depth == known && *instance < source) {
                source = instance.clone();
            }
        }
        source
    }

    /// Register the instances at and under `path`, not looking inside them.
    fn walk(&mut self, path: &Path) -> Result<()> {
        let mut work = vec![path.clone()];
        while let Some(path) = work.pop() {
            self.composer.check()?;
            let index = self.composer.index(&path)?;
            let opinions = self.gather(&index, None);
            if !self.is_active(&opinions) {
                continue;
            }
            if index.instanceable {
                let key = self.composer.instance_key(&index);
                let prototype = match self.prototypes.iter().position(|known| known.key == key) {
                    Some(at) => at,
                    None => {
                        self.prototypes.push(Prototype {
                            key,
                            source: path.clone(),
                            flattened: Path::empty(),
                            number: 0,
                        });
                        self.prototypes.len() - 1
                    }
                };
                self.instance_of.insert(path, prototype);
                continue;
            }
            for name in self.composer.child_names(&index).into_iter().rev() {
                work.push(path.child(name));
            }
        }
        Ok(())
    }

    /// Name the prototypes, past the names of the stage's root prims
    /// (`roots`), and write each: an over with its source's children
    /// (`_CopyPrototypePrim`).
    pub(super) fn write_prototypes(&mut self, roots: &[Path]) -> Result<()> {
        let count = self.prototypes.len();
        if count == 0 {
            return Ok(());
        }
        // Flattened names follow the prototype paths' text order
        // ("__Prototype_10" sorts before "__Prototype_2").
        let mut by_text: Vec<usize> = (0..count).collect();
        by_text.sort_by(|&a, &b| {
            self.prototypes[a]
                .number
                .to_string()
                .cmp(&self.prototypes[b].number.to_string())
        });
        let taken: HashSet<&Path> = roots.iter().collect();
        let mut next = 1;
        for &prototype in &by_text {
            let flattened = loop {
                let name = Path::absolute_root().child(format!("Flattened_Prototype_{next}"));
                next += 1;
                if !taken.contains(&name) {
                    break name;
                }
            };
            self.prototypes[prototype].flattened = flattened;
        }
        for &prototype in &by_text {
            let (source, flattened) = (
                self.prototypes[prototype].source.clone(),
                self.prototypes[prototype].flattened.clone(),
            );
            self.out
                .create_spec(flattened.clone(), SpecType::Prim)
                .set(field::SPECIFIER, Value::Specifier(Specifier::Over));
            self.children
                .entry(Path::absolute_root())
                .or_default()
                .push(flattened.name_token());
            let index = self.composer.index(&source)?;
            let names: Vec<Token> = self.composer.child_names(&index);
            self.writing = Some((source.clone(), flattened.clone()));
            let result = names.into_iter().try_for_each(|name| {
                self.prims(&source.child(name.clone()), &flattened.child(name))
            });
            self.writing = None;
            result?;
        }
        Ok(())
    }

    /// A target path of a prim in the prototype being written: paths in its
    /// source move into the prototype.
    pub(super) fn in_prototype(&self, path: &Path) -> Path {
        match &self.writing {
            Some((source, flattened)) if path.has_prefix(source) => {
                path.replace_prefix(source, flattened)
            }
            _ => path.clone(),
        }
    }

    /// `add references = </Flattened_Prototype_N>` on the flattened spec of
    /// instance `source`.
    pub(super) fn reference_prototype(&mut self, source: &Path, path: &Path) {
        let Some(&prototype) = self.instance_of.get(source) else {
            return;
        };
        let target = self.prototypes[prototype].flattened.clone();
        let reference = Reference {
            asset: Token::empty(),
            prim: target,
            offset: LayerOffset::IDENTITY,
            custom_data: Default::default(),
        };
        let list = ListOp {
            added: vec![reference],
            ..ListOp::default()
        };
        self.out.set(
            path,
            field::REFERENCES,
            Value::ReferenceListOp(Arc::new(list)),
        );
    }
}
