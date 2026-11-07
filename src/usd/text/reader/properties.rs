// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Attributes and relationships, after the grammar's PropertySpec: an
//! attribute declaration with a default value (or None, AnimationBlock) and
//! metadata, `.timeSamples`, `.connect` and list-op connections; a
//! relationship with its targets, metadata and list-op targets. Target and
//! connection paths expand against the owning prim, and each target or
//! connection set explicitly or added gets its own spec, as Sdf's parser
//! makes them.

use std::sync::Arc;

use super::Reader;
use super::metadata::Items;
use crate::usd::error::Result;
use crate::usd::sdf::schema::applies;
use crate::usd::sdf::{
    ListPart, Path, SpecType, Token, Value, ValueKind, Variability, field, find_type,
};

/// What an attribute's declaration says its values are.
#[derive(Clone, Copy)]
struct Declared {
    /// The value kind; none for an unknown type (values keep their text);
    /// `Value` for opaque and group types (which hold no values).
    kind: Option<ValueKind>,
    array: bool,
}

impl Reader<'_> {
    /// One property statement in the body of the prim at `prim`.
    pub(super) fn property_spec(&mut self, prim: &Path, properties: &mut Vec<Token>) -> Result<()> {
        if let Some(part) = self.list_op_keyword() {
            self.skip_inline()?;
            if self.sees_relationship() {
                let path = self.relationship_head(prim, properties)?;
                self.skip_inline()?;
                if self.peek() == b'=' {
                    self.assignment()?;
                    self.targets(&path, part, prim)?;
                }
                return Ok(());
            }
            let (path, _) = self.attribute_head(prim, properties)?;
            self.skip_inline()?;
            self.expect(b'.', "expected '.connect' in a connection list edit")?;
            self.skip_inline()?;
            if !self.keyword("connect") {
                return Err(self.error("expected 'connect'"));
            }
            self.assignment()?;
            return self.connections(&path, part, prim);
        }
        if self.sees_relationship() {
            return self.relationship(prim, properties);
        }
        self.attribute(prim, properties)
    }

    /// Whether a relationship declaration starts here.
    fn sees_relationship(&self) -> bool {
        if self.sees("rel") || self.sees("varying") {
            return true;
        }
        if !self.sees("custom") {
            return false;
        }
        let mut index = self.at + "custom".len();
        while matches!(self.data.get(index), Some(b' ' | b'\t')) {
            index += 1;
        }
        let rest = &self.data[index.min(self.data.len())..];
        let word = |word: &str| {
            rest.starts_with(word.as_bytes())
                && !rest
                    .get(word.len())
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        };
        word("rel") || word("varying")
    }

    /// `[custom] [varying] rel name`: the property path (its spec made).
    fn relationship_head(&mut self, prim: &Path, properties: &mut Vec<Token>) -> Result<Path> {
        let custom = self.keyword("custom");
        if custom {
            self.skip_inline()?;
        }
        let mut variability = Variability::Uniform;
        if self.keyword("varying") {
            variability = Variability::Varying;
            self.skip_inline()?;
        }
        if !self.keyword("rel") {
            return Err(self.error("expected 'rel'"));
        }
        self.skip_inline()?;
        let name = Token::new(self.namespaced_name()?);
        let path = prim.property(name.clone());
        if !self.layer.has_spec(&path) {
            self.layer.create_spec(path.clone(), SpecType::Relationship);
            properties.push(name);
        }
        self.set_field(&path, field::VARIABILITY, Value::Variability(variability));
        if custom {
            self.set_field(&path, field::CUSTOM, Value::Bool(true));
        }
        Ok(path)
    }

    fn relationship(&mut self, prim: &Path, properties: &mut Vec<Token>) -> Result<()> {
        let path = self.relationship_head(prim, properties)?;
        let before = self.at;
        self.skip_inline()?;
        if self.peek() == b'.' {
            self.at += 1;
            self.skip_inline()?;
            if !self.keyword("default") {
                return Err(self.error("expected 'default'"));
            }
            self.assignment()?;
            let target = self.path_ref()?;
            self.set_field(&path, field::DEFAULT, Value::Path(target));
            return Ok(());
        }
        if self.peek() == b'[' {
            return Err(self.unsupported("the bracketed relationship target form is not supported"));
        }
        if self.peek() == b'=' {
            self.assignment()?;
            self.targets(&path, ListPart::Explicit, prim)?;
            self.skip_inline()?;
        } else {
            self.at = before;
            self.skip_inline()?;
        }
        if self.peek() == b'(' {
            self.metadata(&path, applies::RELATIONSHIP, prim)?;
        }
        Ok(())
    }

    /// `None | <path> | [<path>, ...]` as the targets of `path`.
    fn targets(&mut self, path: &Path, part: ListPart, prim: &Path) -> Result<()> {
        let items = self.path_list(prim)?;
        if items.is_empty() && part != ListPart::Explicit {
            return Err(self.error("None or an empty list is only allowed for explicit targets"));
        }
        for item in &items {
            self.check_target_path(item)?;
        }
        self.check_unique(&items)?;
        if part == ListPart::Explicit || part == ListPart::Added {
            let mut children = self
                .layer
                .get(path, field::TARGET_CHILDREN)
                .and_then(path_vector)
                .unwrap_or_default();
            for item in &items {
                let target_path = path.target(item.clone());
                if !self.layer.has_spec(&target_path) {
                    self.layer
                        .create_spec(target_path, SpecType::RelationshipTarget);
                    children.push(item.clone());
                }
            }
            if !children.is_empty() {
                self.set_field(
                    path,
                    field::TARGET_CHILDREN,
                    Value::PathVector(Arc::new(children)),
                );
            }
        }
        self.set_list_items(path, field::TARGET_PATHS, part, Items::Paths(items));
        Ok(())
    }

    /// `[custom] [uniform|config] type[[]] name`: the property path and what
    /// its values are.
    fn attribute_head(
        &mut self,
        prim: &Path,
        properties: &mut Vec<Token>,
    ) -> Result<(Path, Declared)> {
        let custom = self.keyword("custom");
        if custom {
            self.skip_inline()?;
        }
        let mut variability = Variability::Varying;
        if self.keyword("uniform") || self.keyword("config") {
            variability = Variability::Uniform;
            self.skip_inline()?;
        }
        let type_text = self.identifier()?;
        let array = self.array_brackets()?;
        // An unknown type is kept by name; its values keep their text.
        let kind = find_type(type_text).map(|(entry, _)| entry.kind.unwrap_or(ValueKind::Value));
        // The typeName field is the name as written, with its [].
        let type_name = if array {
            Token::from(format!("{type_text}[]"))
        } else {
            Token::new(type_text)
        };
        self.skip_inline()?;
        let name = Token::new(self.namespaced_name()?);
        let path = prim.property(name.clone());
        if let Some(spec) = self.layer.spec(&path) {
            if spec
                .get(field::TYPE_NAME)
                .and_then(Value::as_token)
                .is_some_and(|old| *old != type_name)
            {
                return Err(self.error("an attribute's type cannot change"));
            }
            if let Some(Value::Variability(old)) = spec.get(field::VARIABILITY)
                && *old != variability
            {
                return Err(self.error("an attribute's variability cannot change"));
            }
        } else {
            self.layer.create_spec(path.clone(), SpecType::Attribute);
            properties.push(name);
            self.set_field(&path, field::CUSTOM, Value::Bool(false));
        }
        if custom {
            self.set_field(&path, field::CUSTOM, Value::Bool(true));
        }
        if self.layer.get(&path, field::TYPE_NAME).is_none() {
            self.set_field(&path, field::TYPE_NAME, Value::Token(type_name));
        }
        if self.layer.get(&path, field::VARIABILITY).is_none() {
            self.set_field(&path, field::VARIABILITY, Value::Variability(variability));
        }
        Ok((path, Declared { kind, array }))
    }

    fn attribute(&mut self, prim: &Path, properties: &mut Vec<Token>) -> Result<()> {
        let (path, declared) = self.attribute_head(prim, properties)?;
        let before = self.at;
        self.skip_inline()?;
        if self.peek() == b'.' {
            self.at += 1;
            self.skip_inline()?;
            if self.keyword("timeSamples") {
                self.assignment()?;
                return self.time_samples(&path, declared);
            }
            if self.keyword("connect") {
                self.assignment()?;
                return self.connections(&path, ListPart::Explicit, prim);
            }
            if self.keyword("spline") {
                return Err(self.unsupported("attribute splines are not supported"));
            }
            return Err(self.error("expected timeSamples, connect or spline"));
        }
        if self.peek() == b'=' {
            self.assignment()?;
            let value = self.attribute_value(declared)?;
            self.set_field(&path, field::DEFAULT, value);
            self.skip_inline()?;
        } else {
            self.at = before;
            self.skip_inline()?;
        }
        if self.peek() == b'(' {
            self.metadata(&path, applies::ATTRIBUTE, prim)?;
        }
        Ok(())
    }

    /// An attribute value: None (a value block), AnimationBlock, a path, or
    /// a typed value.
    fn attribute_value(&mut self, declared: Declared) -> Result<Value> {
        if self.keyword("None") {
            return Ok(Value::ValueBlock);
        }
        if self.keyword("AnimationBlock") {
            return Ok(Value::AnimationBlock);
        }
        // A path reference is a path value, whatever the attribute's type.
        if self.peek() == b'<' {
            return Ok(Value::Path(self.path_ref()?));
        }
        if self.sees("edit") {
            return Err(self.unsupported("array edits are not supported"));
        }
        match declared.kind {
            Some(ValueKind::Value) => {
                Err(self.error("an authored opinion for an opaque attribute"))
            }
            None => {
                let mut out = Vec::new();
                self.recorded_item(&mut out)?;
                Ok(Value::Unregistered(Token::from(
                    String::from_utf8(out).map_err(|_| self.error("a value that is not UTF-8"))?,
                )))
            }
            Some(kind) => self.typed_value(kind, declared.array),
        }
    }

    /// `{ time: value, ... }` into the timeSamples field.
    fn time_samples(&mut self, path: &Path, declared: Declared) -> Result<()> {
        self.expect(b'{', "expected '{' to open time samples")?;
        let mut samples: Vec<(f64, Value)> = Vec::new();
        self.skip_all()?;
        if self.peek() != b'}' {
            loop {
                let time = self.number()?.as_f64();
                self.skip_inline()?;
                self.expect(b':', "expected ':' after a sample time")?;
                self.skip_inline()?;
                let value = if self.keyword("None") {
                    Value::ValueBlock
                } else {
                    self.attribute_value(declared)?
                };
                // Kept in time order; a repeated time replaces the value.
                match samples.binary_search_by(|(at, _)| at.total_cmp(&time)) {
                    Ok(found) => samples[found].1 = value,
                    Err(insert) => samples.insert(insert, (time, value)),
                }
                if !self.list_end(b'}')? {
                    break;
                }
            }
        }
        self.expect(b'}', "expected '}' to close time samples")?;
        self.set_field(
            path,
            field::TIME_SAMPLES,
            Value::TimeSamples(Arc::new(samples)),
        );
        Ok(())
    }

    /// `None | <path> | [<path>, ...]` as the connections of `path`.
    fn connections(&mut self, path: &Path, part: ListPart, prim: &Path) -> Result<()> {
        let items = self.path_list(prim)?;
        if items.is_empty() && part != ListPart::Explicit {
            return Err(
                self.error("None or an empty list is only allowed for explicit connections")
            );
        }
        for item in &items {
            self.check_target_path(item)?;
        }
        self.check_unique(&items)?;
        if part == ListPart::Explicit || part == ListPart::Added {
            for item in &items {
                let connection = path.target(item.clone());
                if !self.layer.has_spec(&connection) {
                    self.layer.create_spec(connection, SpecType::Connection);
                }
            }
            self.set_field(
                path,
                field::CONNECTION_CHILDREN,
                Value::PathVector(Arc::new(items.clone())),
            );
        }
        self.set_list_items(path, field::CONNECTION_PATHS, part, Items::Paths(items));
        Ok(())
    }
}

fn path_vector(value: &Value) -> Option<Vec<Path>> {
    match value {
        Value::PathVector(paths) => Some(paths.to_vec()),
        _ => None,
    }
}
