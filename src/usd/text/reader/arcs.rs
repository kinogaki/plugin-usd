// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Composition arcs and layer structure in metadata: references and
//! payloads (with layer offsets and, for references, custom data), inherits
//! and specializes paths, variant set names and selections, relocates, and
//! the layer's subLayers.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::Reader;
use super::metadata::Items;
use crate::usd::error::Result;
use crate::usd::sdf::{
    Dictionary, LayerOffset, ListPart, Path, Payload, Reference, SpecType, Token, Value, field,
};

impl Reader<'_> {
    /// `None | item | [items]` of references (or payloads) into `part`.
    pub(super) fn reference_list(
        &mut self,
        spec: &Path,
        part: ListPart,
        payload: bool,
    ) -> Result<()> {
        let mut items = Vec::new();
        if !self.keyword("None") {
            if self.peek() == b'[' {
                self.at += 1;
                self.skip_all()?;
                if self.peek() != b']' {
                    loop {
                        items.push(self.reference_item(payload)?);
                        if !self.list_end(b']')? {
                            break;
                        }
                    }
                }
                self.expect(b']', "expected ']' after references")?;
            } else {
                items.push(self.reference_item(payload)?);
            }
        }
        if items.is_empty() && part != ListPart::Explicit {
            return Err(self.error(
                "None or an empty list is only allowed for explicit references and payloads",
            ));
        }
        self.check_unique(&items)?;
        if payload {
            let payloads = items
                .into_iter()
                .map(|item| Payload {
                    asset: item.asset,
                    prim: item.prim,
                    offset: item.offset,
                })
                .collect();
            self.set_list_items(spec, field::PAYLOAD, part, Items::Payloads(payloads));
        } else {
            self.set_list_items(spec, field::REFERENCES, part, Items::References(items));
        }
        Ok(())
    }

    /// `@asset@[<prim>] [( params )]` or `<prim> [( params )]`.
    fn reference_item(&mut self, payload: bool) -> Result<Reference> {
        let mut item = Reference {
            asset: Token::empty(),
            prim: Path::empty(),
            offset: LayerOffset::IDENTITY,
            custom_data: Dictionary::new(),
        };
        if self.peek() == b'<' {
            item.prim = self.path_ref()?;
        } else {
            let asset = self.asset_path()?;
            if asset.is_empty() {
                return Err(self.error("a reference or payload asset path must not be empty"));
            }
            item.asset = Token::new(&asset);
            self.skip_inline()?;
            if self.peek() == b'<' {
                item.prim = self.path_ref()?;
            }
        }
        self.check_arc_path(&item.prim)?;
        self.skip_inline()?;
        if self.peek() != b'(' {
            return Ok(item);
        }
        self.at += 1;
        self.skip_all()?;
        while self.peek() != b')' {
            if !payload && self.keyword("customData") {
                self.assignment()?;
                item.custom_data = self.dictionary_value()?;
            } else if self.keyword("offset") {
                self.assignment()?;
                item.offset.offset = self.number()?.as_f64();
            } else if self.keyword("scale") {
                self.assignment()?;
                item.offset.scale = self.number()?.as_f64();
            } else {
                return Err(self.error("expected offset, scale or customData"));
            }
            self.statement_end(b')')?;
            self.skip_all()?;
        }
        self.at += 1;
        Ok(item)
    }

    /// `None | <path> | [<path>, ...]` of inherits or specializes, anchored
    /// at the prim, into `part` of `name`.
    pub(super) fn class_paths(
        &mut self,
        spec: &Path,
        name: &str,
        part: ListPart,
        anchor: &Path,
    ) -> Result<()> {
        let items = self.path_list(anchor)?;
        if items.is_empty() && part != ListPart::Explicit {
            return Err(self.error(
                "None or an empty list is only allowed for explicit inherits and specializes",
            ));
        }
        for path in &items {
            self.check_class_path(path)?;
        }
        self.check_unique(&items)?;
        self.set_list_items(spec, name, part, Items::Paths(items));
        Ok(())
    }

    /// `None | <path> | [<path>, ...]`, relative paths made absolute at
    /// `anchor` with its variant selections stripped.
    pub(super) fn path_list(&mut self, anchor: &Path) -> Result<Vec<Path>> {
        let mut items = Vec::new();
        if self.keyword("None") {
            return Ok(items);
        }
        let base = anchor.strip_variants();
        if self.peek() == b'[' {
            self.at += 1;
            self.skip_all()?;
            if self.peek() != b']' {
                loop {
                    let path = self.path_ref()?;
                    items.push(
                        path.make_absolute(&base)
                            .map_err(|error| self.error(error.message))?,
                    );
                    if !self.list_end(b']')? {
                        break;
                    }
                }
            }
            self.expect(b']', "expected ']' after paths")?;
            return Ok(items);
        }
        let path = self.path_ref()?;
        items.push(
            path.make_absolute(&base)
                .map_err(|error| self.error(error.message))?,
        );
        Ok(items)
    }

    /// `variantSets = "name" | ["a", "b"]`: the string list op, and for
    /// explicit and added names the variant set specs and children.
    pub(super) fn variant_set_names(&mut self, spec: &Path, part: ListPart) -> Result<()> {
        let mut names = Vec::new();
        if self.peek() == b'[' {
            self.at += 1;
            self.skip_all()?;
            loop {
                names.push(Token::new(&self.string()?));
                if !self.list_end(b']')? {
                    break;
                }
            }
            self.expect(b']', "expected ']' after variant set names")?;
        } else {
            names.push(Token::new(&self.string()?));
        }
        self.set_list_items(
            spec,
            field::VARIANT_SET_NAMES,
            part,
            Items::Strings(names.clone()),
        );
        if part == ListPart::Explicit || part == ListPart::Added {
            for name in &names {
                let set_path = spec.variant(name.clone(), "");
                self.layer.create_spec(set_path, SpecType::VariantSet);
            }
            self.set_field(
                spec,
                field::VARIANT_SET_CHILDREN,
                Value::token_vector(names),
            );
        }
        Ok(())
    }

    /// `variants = { string set = "selection" ... }` into the selection map.
    pub(super) fn variant_selections(&mut self, spec: &Path) -> Result<()> {
        let dictionary = self.dictionary_value()?;
        let mut selections = BTreeMap::new();
        for (set, value) in dictionary {
            let Value::String(selection) = value else {
                return Err(self.error("variant selections must be strings"));
            };
            // A selection may be a variable expression, `...`.
            let expression =
                selection.len() >= 2 && selection.starts_with('`') && selection.ends_with('`');
            if !expression {
                self.check_variant_name(&selection)?;
            }
            selections.insert(set, selection);
        }
        self.set_field(
            spec,
            field::VARIANT_SELECTION,
            Value::VariantSelections(Arc::new(selections)),
        );
        Ok(())
    }

    /// `relocates = { <source>: <target>, ... }`. On a prim (`anchor` set)
    /// the paths are made absolute there; on the layer they are kept.
    pub(super) fn relocates(&mut self, spec: &Path, anchor: Option<&Path>) -> Result<()> {
        self.expect(b'{', "expected '{' after relocates")?;
        self.skip_all()?;
        let mut pairs = Vec::new();
        let base = anchor.map_or_else(Path::absolute_root, Path::strip_variants);
        if self.peek() != b'}' {
            loop {
                let mut source = self.path_ref()?;
                self.skip_inline()?;
                self.expect(b':', "expected ':' between relocates paths")?;
                self.skip_inline()?;
                let mut target = self.path_ref()?;
                self.check_relocates_source(&source)?;
                self.check_relocates_target(&target)?;
                source = source
                    .make_absolute(&base)
                    .map_err(|error| self.error(error.message))?;
                if !target.is_empty() {
                    target = target
                        .make_absolute(&base)
                        .map_err(|error| self.error(error.message))?;
                }
                pairs.push((source, target));
                if !self.list_end(b'}')? {
                    break;
                }
            }
        }
        self.expect(b'}', "expected '}' after relocates")?;
        let name = if anchor.is_some() {
            field::RELOCATES
        } else {
            field::LAYER_RELOCATES
        };
        self.set_field(spec, name, Value::Relocates(Arc::new(pairs)));
        Ok(())
    }

    /// `subLayers = [ @path@ (offset = o; scale = s), ... ]`.
    pub(super) fn sublayers(&mut self, spec: &Path) -> Result<()> {
        self.expect(b'[', "expected '[' after subLayers")?;
        self.skip_all()?;
        let mut assets = Vec::new();
        let mut offsets = Vec::new();
        if self.peek() != b']' {
            loop {
                assets.push(Token::new(&self.asset_path()?));
                let mut offset = LayerOffset::IDENTITY;
                self.skip_inline()?;
                if self.peek() == b'(' {
                    self.at += 1;
                    self.skip_all()?;
                    while self.peek() != b')' {
                        if self.keyword("offset") {
                            self.assignment()?;
                            offset.offset = self.number()?.as_f64();
                        } else if self.keyword("scale") {
                            self.assignment()?;
                            offset.scale = self.number()?.as_f64();
                        } else {
                            return Err(self.error("expected offset or scale"));
                        }
                        self.statement_end(b')')?;
                        self.skip_all()?;
                    }
                    self.at += 1;
                }
                offsets.push(offset);
                if !self.list_end(b']')? {
                    break;
                }
            }
        }
        self.expect(b']', "expected ']' after subLayers")?;
        if assets.is_empty() {
            return Ok(());
        }
        self.set_field(
            spec,
            field::SUB_LAYERS,
            Value::StringVector(Arc::new(assets)),
        );
        self.set_field(
            spec,
            field::SUB_LAYER_OFFSETS,
            Value::LayerOffsetVector(Arc::new(offsets)),
        );
        Ok(())
    }
}
