// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Values as text (`Sdf_FileIOUtility::StringFromVtValue`, `WriteDictionary`,
//! `WriteListOp`, `WriteRelocates`, `WriteLayerOffset`): numbers as
//! `TfStringify` gives them, bools as 1 and 0, tuples `(a, b)`, matrices
//! `( (a, b), (c, d) )`, arrays `[a, b]` on one line, strings and tokens
//! quoted, assets `@...@`.

use super::Writer;
use super::arrays::{format_array, format_element};
use crate::usd::error::{Error, Result};
use crate::usd::sdf::identifier::is_ascii_identifier;
use crate::usd::sdf::{
    Array, Dictionary, ListOp, ListPart, Path, Payload, Reference, Token, Value,
};
use crate::usd::text::number::{write_f64, write_integer, write_unsigned};

/// How a list op's items print.
enum ListItems<'a> {
    Quoted(&'a [Token]),
    Bare(&'a [Token]),
    Paths(&'a [Path]),
    Int(Vec<i64>),
    Unsigned(Vec<u64>),
    References(&'a [Reference]),
    Payloads(&'a [Payload]),
}

impl ListItems<'_> {
    fn len(&self) -> usize {
        match self {
            ListItems::Quoted(items) | ListItems::Bare(items) => items.len(),
            ListItems::Paths(items) => items.len(),
            ListItems::Int(items) => items.len(),
            ListItems::Unsigned(items) => items.len(),
            ListItems::References(items) => items.len(),
            ListItems::Payloads(items) => items.len(),
        }
    }

    /// Paths, references and payloads print one per line.
    fn per_line(&self) -> bool {
        matches!(
            self,
            ListItems::Paths(_) | ListItems::References(_) | ListItems::Payloads(_)
        )
    }
}

fn parts<T>(list: &ListOp<T>) -> Vec<(ListPart, &[T])>
where
    T: Clone + PartialEq,
{
    if list.explicit {
        return vec![(ListPart::Explicit, list.explicit_items.as_slice())];
    }
    ListPart::EDITS
        .iter()
        .map(|&part| (part, list.items(part)))
        .filter(|(_, items)| !items.is_empty())
        .collect()
}

impl Writer<'_> {
    /// A field in a metadata section: `name = value`, list ops per part.
    pub(super) fn simple_field(&mut self, indent: usize, name: &str, value: &Value) -> Result<()> {
        if matches!(
            value,
            Value::IntListOp(_)
                | Value::Int64ListOp(_)
                | Value::UIntListOp(_)
                | Value::UInt64ListOp(_)
                | Value::StringListOp(_)
                | Value::TokenListOp(_)
                | Value::UnregisteredListOp(_)
        ) {
            return self.list_op(indent, name, value);
        }
        self.indent(indent);
        self.put(name);
        self.put(" = ");
        match value {
            Value::Unregistered(text) => {
                self.put(text);
                self.put("\n");
            }
            Value::UnregisteredDictionary(dictionary) | Value::Dictionary(dictionary) => {
                self.dictionary(indent, dictionary, false)?;
            }
            Value::Bool(flag) => self.put(if *flag { "true\n" } else { "false\n" }),
            _ => {
                self.value(value)?;
                self.put("\n");
            }
        }
        Ok(())
    }

    /// `StringFromVtValue`: an attribute value, a block, an array.
    pub(super) fn value(&mut self, value: &Value) -> Result<()> {
        match value {
            Value::ValueBlock => self.put("None"),
            Value::AnimationBlock => self.put("AnimationBlock"),
            Value::Unregistered(text) => self.put(text),
            Value::TokenVector(items) | Value::StringVector(items) => {
                self.byte(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        self.put(", ");
                    }
                    self.quoted(item);
                }
                self.byte(b']');
            }
            Value::DoubleVector(items) => {
                self.byte(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        self.put(", ");
                    }
                    write_f64(&mut self.out, *item);
                }
                self.byte(b']');
            }
            Value::Path(path) => self.path_ref(path),
            Value::Array(array) => self.array(array),
            _ => {
                if value.attribute_kind().is_none() {
                    return Err(Error::unsupported(format!(
                        "a {:?} value cannot be written as usda",
                        value.kind()
                    )));
                }
                self.scalar(value);
            }
        }
        Ok(())
    }

    /// A scalar attribute value.
    fn scalar(&mut self, value: &Value) {
        match value {
            Value::String(text) | Value::Token(text) | Value::PathExpression(text) => {
                self.quoted(text)
            }
            Value::Asset(text) => self.asset(text),
            _ => format_element(&mut self.out, value),
        }
    }

    /// An array value: numbers through the array formatter, strings quoted.
    fn array(&mut self, array: &Array) {
        match array {
            Array::String(items) | Array::Token(items) | Array::PathExpression(items) => {
                self.byte(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        self.put(", ");
                    }
                    self.quoted(item);
                }
                self.byte(b']');
            }
            Array::Asset(items) => {
                self.byte(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        self.put(", ");
                    }
                    self.asset(item);
                }
                self.byte(b']');
            }
            _ => {
                if let Some(text) = self.preformatted.find(array) {
                    self.out.push(b'[');
                    self.out.extend_from_slice(text);
                    self.out.push(b']');
                } else {
                    format_array(&mut self.out, array);
                }
            }
        }
    }

    /// `{\n ... }\n`: keys in byte order, `dictionary k = {...}` nested,
    /// `type k = value` otherwise; `"k": "v"` pairs when `strings_only`.
    pub(super) fn dictionary(
        &mut self,
        indent: usize,
        dictionary: &Dictionary,
        strings_only: bool,
    ) -> Result<()> {
        self.put("{\n");
        let mut remaining = dictionary.len();
        for (key, entry) in dictionary {
            remaining -= 1;
            if strings_only {
                let Value::String(text) = entry else { continue };
                self.indent(indent + 1);
                self.quoted(key);
                self.put(": ");
                self.quoted(text);
                if remaining > 0 {
                    self.put(", ");
                }
                self.put("\n");
                continue;
            }
            if let Value::Dictionary(inner) = entry {
                self.indent(indent + 1);
                self.put("dictionary ");
                self.key(key);
                self.put(" = ");
                self.dictionary(indent + 1, inner, false)?;
                continue;
            }
            self.indent(indent + 1);
            self.value_type(entry)?;
            self.byte(b' ');
            self.key(key);
            self.put(" = ");
            match entry {
                // TfStringify prints path expressions in dictionaries unquoted.
                Value::PathExpression(text) => self.put(text),
                Value::Array(Array::PathExpression(items)) => {
                    self.byte(b'[');
                    for (index, item) in items.iter().enumerate() {
                        if index > 0 {
                            self.put(", ");
                        }
                        self.put(item);
                    }
                    self.byte(b']');
                }
                _ => self.value(entry)?,
            }
            self.put("\n");
        }
        self.indent(indent);
        self.put("}\n");
        Ok(())
    }

    /// A dictionary key: bare when it is an identifier, quoted otherwise.
    pub(super) fn key(&mut self, text: &str) {
        if is_ascii_identifier(text) {
            self.put(text);
        } else {
            self.quoted(text);
        }
    }

    /// The serialization type name of a dictionary value.
    fn value_type(&mut self, value: &Value) -> Result<()> {
        let (name, array) = match value {
            Value::StringVector(_) => ("string", true),
            Value::TokenVector(_) => ("token", true),
            Value::DoubleVector(_) => ("double", true),
            Value::Array(array) => (array.kind().type_name().unwrap_or(""), true),
            other => (
                other
                    .attribute_kind()
                    .and_then(|kind| kind.type_name())
                    .unwrap_or(""),
                false,
            ),
        };
        if name.is_empty() {
            return Err(Error::unsupported(format!(
                "a dictionary cannot hold a {:?} value",
                value.kind()
            )));
        }
        self.put(name);
        if array {
            self.put("[]");
        }
        Ok(())
    }

    /// A list op field, explicit or one line per part: `op name = items`.
    pub(super) fn list_op(&mut self, indent: usize, name: &str, value: &Value) -> Result<()> {
        macro_rules! each_part {
            ($list:expr, $items:expr) => {
                for (part, items) in parts($list) {
                    self.list_part(indent, name, part.keyword(), $items(items));
                }
            };
        }
        match value {
            Value::TokenListOp(list) | Value::StringListOp(list) => {
                each_part!(list, ListItems::Quoted)
            }
            Value::UnregisteredListOp(list) => each_part!(list, ListItems::Bare),
            Value::PathListOp(list) => each_part!(list, ListItems::Paths),
            Value::ReferenceListOp(list) => each_part!(list, ListItems::References),
            Value::PayloadListOp(list) => each_part!(list, ListItems::Payloads),
            Value::IntListOp(list) => {
                each_part!(list, |items: &[i32]| ListItems::Int(
                    items.iter().map(|&x| i64::from(x)).collect()
                ))
            }
            Value::Int64ListOp(list) => {
                each_part!(list, |items: &[i64]| ListItems::Int(items.to_vec()))
            }
            Value::UIntListOp(list) => {
                each_part!(list, |items: &[u32]| ListItems::Unsigned(
                    items.iter().map(|&x| u64::from(x)).collect()
                ))
            }
            Value::UInt64ListOp(list) => {
                each_part!(list, |items: &[u64]| ListItems::Unsigned(items.to_vec()))
            }
            _ => return Err(Error::unsupported("a list op this writer cannot print")),
        }
        Ok(())
    }

    /// One list op part (`_WriteListOpList`): paths, references and
    /// payloads one per line, a single path or payload (or a reference
    /// without custom data) without brackets; tokens, strings and numbers
    /// on one line in brackets.
    fn list_part(&mut self, indent: usize, name: &str, op: &str, items: ListItems<'_>) {
        self.indent(indent);
        if !op.is_empty() {
            self.put(op);
            self.byte(b' ');
        }
        self.put(name);
        self.put(" = ");
        if items.len() == 0 {
            self.put("None\n");
            return;
        }
        let per_line = items.per_line();
        let single = items.len() == 1
            && match &items {
                ListItems::Paths(_) | ListItems::Payloads(_) => true,
                ListItems::References(references) => references[0].custom_data.is_empty(),
                _ => false,
            };
        if single {
            self.list_item(0, &items, 0);
            self.put("\n");
            return;
        }
        self.put(if per_line { "[\n" } else { "[" });
        for index in 0..items.len() {
            self.list_item(if per_line { indent + 1 } else { 0 }, &items, index);
            if index + 1 < items.len() {
                self.put(if per_line { ",\n" } else { ", " });
            } else if per_line {
                self.put("\n");
            }
        }
        if per_line {
            self.indent(indent);
        }
        self.put("]\n");
    }

    fn list_item(&mut self, indent: usize, items: &ListItems<'_>, index: usize) {
        self.indent(indent);
        match items {
            ListItems::Quoted(items) => self.quoted(&items[index]),
            ListItems::Bare(items) => self.put(&items[index]),
            ListItems::Paths(items) => self.path_ref(&items[index]),
            ListItems::Int(items) => write_integer(&mut self.out, items[index]),
            ListItems::Unsigned(items) => write_unsigned(&mut self.out, items[index]),
            ListItems::References(items) => {
                let item = &items[index];
                self.reference(
                    indent,
                    &item.asset,
                    &item.prim,
                    item.offset,
                    Some(&item.custom_data),
                );
            }
            ListItems::Payloads(items) => {
                let item = &items[index];
                self.reference(indent, &item.asset, &item.prim, item.offset, None);
            }
        }
    }

    /// A reference or payload: `@asset@<path>` or `<path>`, its layer
    /// offset, and a reference's custom data in a multi-line block.
    fn reference(
        &mut self,
        indent: usize,
        asset: &str,
        prim: &Path,
        offset: crate::usd::sdf::LayerOffset,
        custom: Option<&Dictionary>,
    ) {
        let custom = custom.filter(|data| !data.is_empty());
        let multiline = custom.is_some();
        if !asset.is_empty() {
            self.asset(asset);
            if !prim.is_empty() {
                self.path_ref(prim);
            }
        } else {
            self.path_ref(prim);
        }
        if multiline {
            self.put(" (\n");
        }
        self.layer_offset(indent + 1, multiline, offset.offset, offset.scale);
        if let Some(data) = custom {
            self.indent(indent + 1);
            self.put("customData = ");
            // Custom data holds only dictionary-printable values (parsed so).
            let _ = self.dictionary(indent + 1, data, false);
        }
        if multiline {
            self.indent(indent);
            self.put(")");
        }
    }

    /// ` (offset = o; scale = s)` or offset/scale lines, when not identity.
    pub(super) fn layer_offset(&mut self, indent: usize, multiline: bool, offset: f64, scale: f64) {
        if offset == 0.0 && scale == 1.0 {
            return;
        }
        if !multiline {
            self.put(" (");
        }
        if offset != 0.0 {
            if multiline {
                self.indent(indent);
            }
            self.put("offset = ");
            write_f64(&mut self.out, offset);
            if multiline {
                self.put("\n");
            }
        }
        if scale != 1.0 {
            if !multiline && offset != 0.0 {
                self.put("; ");
            }
            if multiline {
                self.indent(indent);
            }
            self.put("scale = ");
            write_f64(&mut self.out, scale);
            if multiline {
                self.put("\n");
            }
        }
        if !multiline {
            self.byte(b')');
        }
    }

    /// `relocates = { <a>: <b>, ... }`.
    pub(super) fn relocates(&mut self, indent: usize, multiline: bool, pairs: &[(Path, Path)]) {
        self.indent(indent);
        self.put(if multiline {
            "relocates = {\n"
        } else {
            "relocates = { "
        });
        for (index, (source, target)) in pairs.iter().enumerate() {
            if multiline {
                self.indent(indent + 1);
            }
            self.path_ref(source);
            self.put(": ");
            self.path_ref(target);
            if index + 1 < pairs.len() {
                self.put(", ");
            }
            if multiline {
                self.put("\n");
            }
        }
        if multiline {
            self.indent(indent);
            self.put("}\n");
        } else {
            self.put(" }");
        }
    }
}
