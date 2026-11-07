// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The layer and its prims: the header and layer metadata, root prims and
//! the root prim order, prim specs with their specifier, type, metadata and
//! contents, and variant sets with their variants. Children and property
//! names are recorded in the order they appear, as `primChildren` and
//! `properties` fields, and a variant set's variants as `variantChildren`.

use super::Reader;
use crate::usd::error::Result;
use crate::usd::sdf::identifier::identifier_len;
use crate::usd::sdf::schema::applies;
use crate::usd::sdf::{Path, SpecType, Specifier, Token, Value, field};

impl Reader<'_> {
    /// The whole layer.
    pub(super) fn read_layer(&mut self) -> Result<()> {
        if !self.data.starts_with(b"#usda ") {
            return Err(self.error("not a usda layer: it must start with '#usda '"));
        }
        while !self.at_end() && !matches!(self.peek(), b'\n' | b'\r') {
            self.at += 1;
        }
        let root = Path::absolute_root();
        self.skip_all()?;
        if self.peek() == b'(' {
            self.metadata(&root, applies::LAYER, &root)?;
        }
        let mut children = Vec::new();
        loop {
            self.skip_all()?;
            if self.at_end() {
                break;
            }
            if self.keyword("reorder") {
                self.skip_inline()?;
                if !self.keyword("rootPrims") {
                    return Err(self.error("expected 'rootPrims' after 'reorder'"));
                }
                self.assignment()?;
                let names = self.name_list()?;
                self.set_field(&root, field::PRIM_ORDER, names);
                self.skip_line()?;
                continue;
            }
            self.prim_spec(&root, &mut children)?;
            self.skip_line()?;
            if !self.at_end() && !matches!(self.peek(), b'\n' | b'\r') {
                return Err(self.error("expected a line end after a prim"));
            }
        }
        self.set_field(&root, field::PRIM_CHILDREN, Value::token_vector(children));
        Ok(())
    }

    /// `String | [String, ...]` as a token vector.
    fn name_list(&mut self) -> Result<Value> {
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
            self.expect(b']', "expected ']' after names")?;
        } else {
            names.push(Token::new(&self.string()?));
        }
        Ok(Value::token_vector(names))
    }

    /// `def|over|class [Type] "name" [( metadata )] { contents }` under
    /// `parent`, its name appended to `siblings`.
    fn prim_spec(&mut self, parent: &Path, siblings: &mut Vec<Token>) -> Result<()> {
        self.enter()?;
        let specifier = if self.keyword("def") {
            Specifier::Def
        } else if self.keyword("over") {
            Specifier::Over
        } else if self.keyword("class") {
            Specifier::Class
        } else {
            return Err(self.error("expected def, over or class"));
        };
        self.skip_inline()?;
        let mut type_name = None;
        if !self.sees_string() {
            let start = self.at;
            self.identifier()?;
            loop {
                let before = self.at;
                self.skip_inline()?;
                if self.peek() != b'.' {
                    self.at = before;
                    break;
                }
                self.at += 1;
                self.skip_inline()?;
                self.identifier()?;
            }
            type_name = Some(Token::new(self.text(start, self.at)));
            self.skip_inline()?;
        }
        let name_text = self.string()?;
        if name_text.is_empty() || identifier_len(name_text.as_bytes(), 0) != name_text.len() {
            return Err(self.error(format!("'{name_text}' is not a valid prim name")));
        }
        let name = Token::new(&name_text);
        let path = parent.child(name.clone());
        if self.layer.has_spec(&path) {
            return Err(self.error(format!("a duplicate prim {path}")));
        }
        self.layer.create_spec(path.clone(), SpecType::Prim);
        siblings.push(name);
        self.set_field(&path, field::SPECIFIER, Value::Specifier(specifier));
        if let Some(type_name) = type_name {
            self.set_field(&path, field::TYPE_NAME, Value::Token(type_name));
        }
        self.skip_all()?;
        if self.peek() == b'(' {
            self.metadata(&path, applies::PRIM, &path)?;
            self.skip_all()?;
        }
        self.expect(b'{', "expected '{' to open a prim")?;
        self.contents(&path)?;
        self.expect(b'}', "expected '}' to close a prim")?;
        self.leave();
        Ok(())
    }

    /// The items of a prim or variant body up to its closing `}`; then its
    /// `primChildren` and `properties` fields.
    fn contents(&mut self, path: &Path) -> Result<()> {
        let mut children = Vec::new();
        let mut properties = Vec::new();
        loop {
            self.skip_all()?;
            if self.peek() == b'}' || self.at_end() {
                break;
            }
            if self.sees("def") || self.sees("over") || self.sees("class") {
                self.prim_spec(path, &mut children)?;
                self.item_line_end()?;
                continue;
            }
            if self.keyword("variantSet") {
                self.variant_set(path)?;
                self.item_line_end()?;
                continue;
            }
            let start = self.at;
            if self.keyword("reorder") {
                self.skip_inline()?;
                if self.keyword("nameChildren") {
                    self.assignment()?;
                    let names = self.name_list()?;
                    self.set_field(path, field::PRIM_ORDER, names);
                    self.body_statement_end()?;
                    continue;
                }
                if self.keyword("properties") {
                    self.assignment()?;
                    let names = self.name_list()?;
                    self.set_field(path, field::PROPERTY_ORDER, names);
                    self.body_statement_end()?;
                    continue;
                }
                self.at = start;
            }
            self.property_spec(path, &mut properties)?;
            self.body_statement_end()?;
        }
        if !children.is_empty() {
            self.set_field(path, field::PRIM_CHILDREN, Value::token_vector(children));
        }
        if !properties.is_empty() {
            self.set_field(path, field::PROPERTIES, Value::token_vector(properties));
        }
        Ok(())
    }

    /// A line end must follow a prim or variant set (or the body closes).
    fn item_line_end(&mut self) -> Result<()> {
        self.skip_line()?;
        match self.peek() {
            b'\n' | b'\r' => {
                self.at += 1;
                Ok(())
            }
            b'}' => Ok(()),
            _ if self.at_end() => Ok(()),
            _ => Err(self.error("expected a line end")),
        }
    }

    /// After a property or reorder statement in a prim body: a `;` or a
    /// line end (or the end of the text), never the closing `}` on the
    /// same line.
    fn body_statement_end(&mut self) -> Result<()> {
        self.skip_line()?;
        match self.peek() {
            b';' | b'\n' | b'\r' => {
                self.at += 1;
                Ok(())
            }
            _ if self.at_end() => Ok(()),
            _ => Err(self.error("expected a line end or ';' after a statement")),
        }
    }

    /// `variantSet "name" = { "variant" [( metadata )] { contents } ... }`.
    fn variant_set(&mut self, prim: &Path) -> Result<()> {
        self.skip_inline()?;
        let set_name = Token::new(&self.string()?);
        self.skip_all()?;
        self.expect(b'=', "expected '=' after the variant set name")?;
        self.skip_all()?;
        self.expect(b'{', "expected '{' to open a variant set")?;
        let set_path = prim.variant(set_name.clone(), "");
        if !self.layer.has_spec(&set_path) {
            self.layer
                .create_spec(set_path.clone(), SpecType::VariantSet);
            let mut names = self
                .layer
                .spec(prim)
                .map(|spec| spec.tokens(field::VARIANT_SET_CHILDREN).to_vec())
                .unwrap_or_default();
            names.push(set_name.clone());
            self.set_field(
                prim,
                field::VARIANT_SET_CHILDREN,
                Value::token_vector(names),
            );
        }
        let mut variants = Vec::new();
        loop {
            self.skip_all()?;
            if self.peek() == b'}' {
                break;
            }
            if self.at_end() {
                return Err(self.error("expected '}' to close a variant set"));
            }
            let variant_name = self.string()?;
            self.check_variant_name(&variant_name)?;
            let variant = Token::new(&variant_name);
            let variant_path = prim.variant(set_name.clone(), variant.clone());
            if self.layer.has_spec(&variant_path) {
                return Err(self.error(format!("a duplicate variant {variant_path}")));
            }
            self.layer
                .create_spec(variant_path.clone(), SpecType::Variant);
            variants.push(variant);
            self.skip_all()?;
            if self.peek() == b'(' {
                self.metadata(&variant_path, applies::PRIM, &variant_path)?;
                self.skip_all()?;
            }
            self.expect(b'{', "expected '{' to open a variant")?;
            self.enter()?;
            self.contents(&variant_path)?;
            self.leave();
            self.expect(b'}', "expected '}' to close a variant")?;
        }
        self.at += 1;
        self.set_field(
            &set_path,
            field::VARIANT_CHILDREN,
            Value::token_vector(variants),
        );
        Ok(())
    }
}
