// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Metadata blocks: `( item; item ... )` after a layer header, a prim, a
//! variant, an attribute or a relationship. Items with their own syntax are
//! read here; `key = value` looks the key up in the schema and reads a
//! known field as its type, or keeps an unknown field's text as an
//! unregistered value (`textParserHelpers.cpp`, `_KeyValueMetadataEnd`).

use std::sync::Arc;

use super::Reader;
use crate::usd::error::Result;
use crate::usd::sdf::schema::{FieldForm, applies, find_field};
use crate::usd::sdf::{
    ListOp, ListPart, Path, Payload, Permission, Reference, Token, Value, ValueKind, field,
};

/// The items of one list op part, by the list op's item type.
pub(super) enum Items {
    Tokens(Vec<Token>),
    Strings(Vec<Token>),
    Int64(Vec<i64>),
    Unregistered(Vec<Token>),
    Paths(Vec<Path>),
    References(Vec<Reference>),
    Payloads(Vec<Payload>),
}

fn edit<T: Clone + PartialEq>(
    existing: Option<&Arc<ListOp<T>>>,
    part: ListPart,
    items: Vec<T>,
) -> Arc<ListOp<T>> {
    let mut list = existing.map(|list| (**list).clone()).unwrap_or_default();
    list.set(part, items);
    Arc::new(list)
}

impl Reader<'_> {
    /// Set field `name` of the spec at `spec`.
    pub(super) fn set_field(&mut self, spec: &Path, name: &str, value: Value) {
        self.layer.set(spec, name, value);
    }

    /// Set one part of the list op in field `name` (made when absent).
    pub(super) fn set_list_items(&mut self, spec: &Path, name: &str, part: ListPart, items: Items) {
        let existing = self.layer.get(spec, name);
        let value = match items {
            Items::Tokens(items) => {
                let old = if let Some(Value::TokenListOp(list)) = existing {
                    Some(list)
                } else {
                    None
                };
                Value::TokenListOp(edit(old, part, items))
            }
            Items::Strings(items) => {
                let old = if let Some(Value::StringListOp(list)) = existing {
                    Some(list)
                } else {
                    None
                };
                Value::StringListOp(edit(old, part, items))
            }
            Items::Int64(items) => {
                let old = if let Some(Value::Int64ListOp(list)) = existing {
                    Some(list)
                } else {
                    None
                };
                Value::Int64ListOp(edit(old, part, items))
            }
            Items::Unregistered(items) => {
                let old = if let Some(Value::UnregisteredListOp(list)) = existing {
                    Some(list)
                } else {
                    None
                };
                Value::UnregisteredListOp(edit(old, part, items))
            }
            Items::Paths(items) => {
                let old = if let Some(Value::PathListOp(list)) = existing {
                    Some(list)
                } else {
                    None
                };
                Value::PathListOp(edit(old, part, items))
            }
            Items::References(items) => {
                let old = if let Some(Value::ReferenceListOp(list)) = existing {
                    Some(list)
                } else {
                    None
                };
                Value::ReferenceListOp(edit(old, part, items))
            }
            Items::Payloads(items) => {
                let old = if let Some(Value::PayloadListOp(list)) = existing {
                    Some(list)
                } else {
                    None
                };
                Value::PayloadListOp(edit(old, part, items))
            }
        };
        self.set_field(spec, name, value);
    }

    /// The metadata block at the cursor for the spec at `spec`, of kind
    /// `applies`; `anchor` is the prim path relative paths expand from.
    pub(super) fn metadata(&mut self, spec: &Path, applies: u8, anchor: &Path) -> Result<()> {
        self.expect(b'(', "expected '(' to start metadata")?;
        self.skip_all()?;
        while self.peek() != b')' {
            if self.at_end() {
                return Err(self.error("expected ')' to end metadata"));
            }
            self.metadata_item(spec, applies, anchor)?;
            self.statement_end(b')')?;
            self.skip_all()?;
        }
        self.at += 1;
        Ok(())
    }

    fn metadata_item(&mut self, spec: &Path, applies: u8, anchor: &Path) -> Result<()> {
        if self.sees_string() {
            let text = Token::new(&self.string()?);
            self.set_field(spec, field::COMMENT, Value::String(text));
            return Ok(());
        }
        if self.keyword("doc") {
            self.assignment()?;
            let text = Token::new(&self.string()?);
            self.set_field(spec, field::DOCUMENTATION, Value::String(text));
            return Ok(());
        }
        if applies != applies::LAYER && self.keyword("permission") {
            self.assignment()?;
            let permission = match self.identifier()? {
                "public" => Permission::Public,
                "private" => Permission::Private,
                _ => return Err(self.error("expected 'public' or 'private'")),
            };
            self.set_field(spec, field::PERMISSION, Value::Permission(permission));
            return Ok(());
        }
        if applies != applies::LAYER && self.keyword("symmetryFunction") {
            self.assignment()?;
            let token = if self.sees_identifier() {
                Token::new(self.identifier()?)
            } else {
                Token::empty()
            };
            self.set_field(spec, field::SYMMETRY_FUNCTION, Value::Token(token));
            return Ok(());
        }
        if applies == applies::ATTRIBUTE && self.keyword("displayUnit") {
            self.assignment()?;
            let word = self.identifier()?;
            self.check_display_unit(word)?;
            self.set_field(spec, field::DISPLAY_UNIT, Value::Token(Token::new(word)));
            return Ok(());
        }
        if applies == applies::PRIM && self.prim_metadata_item(spec, anchor)? {
            return Ok(());
        }
        if applies == applies::LAYER {
            if self.keyword("subLayers") {
                self.assignment()?;
                return self.sublayers(spec);
            }
            if self.keyword("relocates") {
                self.assignment()?;
                return self.relocates(spec, None);
            }
        }
        // A list op edit: `add|delete|append|prepend|reorder key = list`.
        if let Some(part) = self.list_op_keyword() {
            self.skip_inline()?;
            let key = self.identifier()?;
            self.assignment()?;
            return self.keyed_list_op(spec, applies, key, part);
        }
        let key = if self.keyword("customData") {
            "customData"
        } else if self.keyword("symmetryArguments") {
            "symmetryArguments"
        } else {
            self.identifier()?
        };
        self.assignment()?;
        self.keyed_value(spec, applies, key)
    }

    /// A list op keyword at the cursor, consumed.
    pub(super) fn list_op_keyword(&mut self) -> Option<ListPart> {
        for (word, part) in [
            ("add", ListPart::Added),
            ("delete", ListPart::Deleted),
            ("append", ListPart::Appended),
            ("prepend", ListPart::Prepended),
            ("reorder", ListPart::Ordered),
        ] {
            if self.keyword(word) {
                return Some(part);
            }
        }
        None
    }

    /// Prim-only items; whether one was read.
    fn prim_metadata_item(&mut self, spec: &Path, anchor: &Path) -> Result<bool> {
        if self.keyword("kind") {
            self.assignment()?;
            let text = Token::new(&self.string()?);
            self.set_field(spec, field::KIND, Value::Token(text));
            return Ok(true);
        }
        if self.keyword("variants") {
            self.assignment()?;
            self.variant_selections(spec)?;
            return Ok(true);
        }
        if self.keyword("relocates") {
            self.assignment()?;
            self.relocates(spec, Some(anchor))?;
            return Ok(true);
        }
        for (word, name) in [
            ("prefixSubstitutions", field::PREFIX_SUBSTITUTIONS),
            ("suffixSubstitutions", field::SUFFIX_SUBSTITUTIONS),
        ] {
            if self.keyword(word) {
                self.assignment()?;
                let dictionary = self.string_dictionary()?;
                self.set_field(spec, name, Value::Dictionary(Arc::new(dictionary)));
                return Ok(true);
            }
        }
        let start = self.at;
        let part = self.list_op_keyword();
        if part.is_some() {
            self.skip_inline()?;
        }
        let part_or_explicit = part.unwrap_or(ListPart::Explicit);
        if self.arc_item(spec, anchor, part_or_explicit)? {
            return Ok(true);
        }
        self.at = start;
        Ok(false)
    }

    /// `references`, `payload`, `inherits`, `specializes` or `variantSets`
    /// with `part`; whether one was read.
    fn arc_item(&mut self, spec: &Path, anchor: &Path, part: ListPart) -> Result<bool> {
        if self.keyword("references") {
            self.assignment()?;
            self.reference_list(spec, part, false)?;
        } else if self.keyword("payload") {
            self.assignment()?;
            self.reference_list(spec, part, true)?;
        } else if self.keyword("inherits") {
            self.assignment()?;
            self.class_paths(spec, field::INHERIT_PATHS, part, anchor)?;
        } else if self.keyword("specializes") {
            self.assignment()?;
            self.class_paths(spec, field::SPECIALIZES, part, anchor)?;
        } else if self.keyword("variantSets") {
            self.assignment()?;
            self.variant_set_names(spec, part)?;
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// `key = None | dictionary | value` for a schema or unregistered key.
    fn keyed_value(&mut self, spec: &Path, applies: u8, key: &str) -> Result<()> {
        if let Some(found) = find_field(key) {
            if found.applies & applies != 0 {
                match found.form {
                    FieldForm::Dictionary => {
                        if self.peek() != b'{' {
                            return Err(self.error("a dictionary field needs a dictionary value"));
                        }
                        let dictionary = self.dictionary_value()?;
                        self.set_field(spec, key, Value::Dictionary(Arc::new(dictionary)));
                    }
                    FieldForm::Plain => {
                        if self.sees("None") {
                            return Err(self.error("an invalid value for a metadata field"));
                        }
                        let kind = found.kind.unwrap_or(ValueKind::Value);
                        let value = self.typed_value(kind, found.array)?;
                        self.set_field(spec, key, value);
                    }
                    _ => return self.keyed_list_op(spec, applies, key, ListPart::Explicit),
                }
                return Ok(());
            }
            if found.applies == 0 {
                return Err(self.error(format!("'{key}' is a field that is not metadata")));
            }
        }
        // Unregistered: a dictionary keeps its value, anything else its text.
        if self.peek() == b'{' {
            let dictionary = self.dictionary_value()?;
            self.set_field(
                spec,
                key,
                Value::UnregisteredDictionary(Arc::new(dictionary)),
            );
            return Ok(());
        }
        let text = self.recorded_value()?;
        self.set_field(spec, key, Value::Unregistered(text));
        Ok(())
    }

    /// `op key = None | [items]` for a list-op field (or unregistered key).
    fn keyed_list_op(&mut self, spec: &Path, applies: u8, key: &str, part: ListPart) -> Result<()> {
        let mut form = None;
        if let Some(found) = find_field(key) {
            if found.applies & applies != 0 {
                match found.form {
                    FieldForm::TokenListOp | FieldForm::StringListOp | FieldForm::Int64ListOp => {
                        form = Some(found.form)
                    }
                    _ => return Err(self.error("list editing a field that is not a list op")),
                }
            } else if found.applies == 0 {
                return Err(self.error(format!("'{key}' is a field that is not metadata")));
            }
        }
        let Some(form) = form else {
            // Unregistered: the list's text without its brackets, one item.
            if part == ListPart::Explicit {
                let text = self.recorded_value()?;
                self.set_field(spec, key, Value::Unregistered(text));
                return Ok(());
            }
            let mut items = Vec::new();
            if !self.keyword("None") {
                let text = self.recorded_value()?;
                let trimmed = text.strip_prefix('[').unwrap_or(&text);
                let trimmed = trimmed.strip_suffix(']').unwrap_or(trimmed);
                items.push(Token::new(trimmed));
            }
            self.set_list_items(spec, key, part, Items::Unregistered(items));
            return Ok(());
        };
        let none = self.keyword("None");
        let items = match form {
            FieldForm::Int64ListOp => {
                let values = if none {
                    Vec::new()
                } else {
                    self.int64_items()?
                };
                Items::Int64(values)
            }
            _ => {
                let mut values = Vec::new();
                if !none {
                    let array = self.typed_value(ValueKind::Token, true)?;
                    if let Value::Array(crate::usd::sdf::Array::Token(tokens)) = array {
                        values = tokens.to_vec();
                    }
                }
                if form == FieldForm::StringListOp {
                    Items::Strings(values)
                } else {
                    Items::Tokens(values)
                }
            }
        };
        self.set_list_items(spec, key, part, items);
        Ok(())
    }

    fn int64_items(&mut self) -> Result<Vec<i64>> {
        match self.typed_value(ValueKind::Int64, true)? {
            Value::Array(crate::usd::sdf::Array::Int64(values)) => Ok(values.to_vec()),
            _ => Err(self.error("expected a list of integers")),
        }
    }
}
