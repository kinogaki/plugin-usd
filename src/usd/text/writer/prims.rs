// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Prims (`Sdf_WritePrim` and its parts): the preamble `def Type "name"`,
//! the metadata section (comment, then fields in dictionary order, with the
//! arcs, variant sets and selections, relocates and substitutions in their
//! own syntax), the body (reorder statements, properties in dictionary
//! order, children, variant sets), and variant sets with their variants.

use super::{Writer, order};
use crate::usd::error::Result;
use crate::usd::sdf::identifier::dictionary_order;
use crate::usd::sdf::{ListPart, Path, Permission, SpecType, Specifier, Value, field};

impl Writer<'_> {
    pub(super) fn write_prim(&mut self, path: &Path, indent: usize) -> Result<()> {
        let prim = self.layer.spec(path).expect("a prim spec");
        let specifier = prim
            .get(field::SPECIFIER)
            .and_then(Value::as_specifier)
            .unwrap_or(Specifier::Over);
        self.indent(indent);
        self.put(match specifier {
            Specifier::Def => "def",
            Specifier::Class => "class",
            Specifier::Over => "over",
        });
        if let Some(type_name) = prim.get(field::TYPE_NAME).and_then(Value::as_token)
            && !type_name.is_empty()
            && type_name.as_str() != "__AnyType__"
        {
            self.byte(b' ');
            self.put(type_name);
        }
        self.byte(b' ');
        self.quoted(path.name());
        self.prim_metadata(path, indent)?;
        self.put("\n");
        self.indent(indent);
        self.put("{\n");
        self.prim_body(path, indent)?;
        self.indent(indent);
        self.put("}\n");
        Ok(())
    }

    /// ` (\n ... )` when the prim has a comment or metadata.
    fn prim_metadata(&mut self, path: &Path, indent: usize) -> Result<()> {
        let prim = self.layer.spec(path).expect("a prim spec");
        let fields = order::metadata_fields(prim, false);
        let comment = prim
            .get(field::COMMENT)
            .and_then(Value::as_token)
            .filter(|text| !text.is_empty());
        if comment.is_none() && fields.is_empty() {
            return Ok(());
        }
        self.put(" (\n");
        if let Some(comment) = comment {
            self.indent(indent + 1);
            self.quoted(comment);
            self.put("\n");
        }
        for (name, value) in fields {
            match name.as_str() {
                field::DOCUMENTATION => {
                    self.indent(indent + 1);
                    self.put("doc = ");
                    self.quoted(value.as_token().map_or("", |text| text.as_str()));
                    self.put("\n");
                }
                field::PERMISSION => self.permission(indent + 1, value),
                field::SYMMETRY_FUNCTION => self.symmetry_function(indent + 1, value),
                field::PAYLOAD => self.list_op(indent + 1, "payload", value)?,
                field::REFERENCES => self.list_op(indent + 1, "references", value)?,
                field::VARIANT_SET_NAMES => self.variant_set_names(indent + 1, value),
                field::INHERIT_PATHS => self.list_op(indent + 1, "inherits", value)?,
                field::SPECIALIZES => self.list_op(indent + 1, "specializes", value)?,
                field::RELOCATES => self.prim_relocates(indent + 1, path, value)?,
                field::PREFIX_SUBSTITUTIONS | field::SUFFIX_SUBSTITUTIONS => {
                    self.indent(indent + 1);
                    self.put(name);
                    self.put(" = ");
                    if let Some(dictionary) = value.as_dictionary() {
                        self.dictionary(indent + 1, dictionary, true)?;
                    }
                }
                field::VARIANT_SELECTION => {
                    if let Value::VariantSelections(selections) = value
                        && !selections.is_empty()
                    {
                        self.indent(indent + 1);
                        self.put("variants = {\n");
                        for (set, selection) in selections.iter() {
                            self.indent(indent + 2);
                            self.put("string ");
                            self.key(set);
                            self.put(" = ");
                            self.quoted(selection);
                            self.put("\n");
                        }
                        self.indent(indent + 1);
                        self.put("}\n");
                    }
                }
                _ => self.simple_field(indent + 1, name, value)?,
            }
        }
        self.indent(indent);
        self.put(")");
        Ok(())
    }

    pub(super) fn permission(&mut self, indent: usize, value: &Value) {
        self.indent(indent);
        self.put(if matches!(value, Value::Permission(Permission::Private)) {
            "permission = private\n"
        } else {
            "permission = public\n"
        });
    }

    pub(super) fn symmetry_function(&mut self, indent: usize, value: &Value) {
        self.indent(indent);
        self.put("symmetryFunction = ");
        self.put(value.as_token().map_or("", |text| text.as_str()));
        self.put("\n");
    }

    /// `variantSets = ...` lines, explicit or per list op part.
    fn variant_set_names(&mut self, indent: usize, value: &Value) {
        let Value::StringListOp(list) = value else {
            return;
        };
        if list.explicit {
            self.indent(indent);
            self.put("variantSets = ");
            self.name_vector(&list.explicit_items);
            self.put("\n");
            return;
        }
        for part in ListPart::EDITS {
            let items = list.items(part);
            if items.is_empty() {
                continue;
            }
            self.indent(indent);
            self.put(part.keyword());
            self.put(" variantSets = ");
            self.name_vector(items);
            self.put("\n");
        }
    }

    /// Reorders, properties, children and variant sets.
    fn prim_body(&mut self, path: &Path, indent: usize) -> Result<()> {
        let prim = self.layer.spec(path).expect("a prim spec");
        if let Some(order) = prim.get(field::PROPERTY_ORDER).and_then(Value::as_tokens)
            && order.len() > 1
        {
            self.indent(indent + 1);
            self.put("reorder properties = ");
            self.name_vector(order);
            self.put("\n");
        }
        if let Some(order) = prim.get(field::PRIM_ORDER).and_then(Value::as_tokens)
            && order.len() > 1
        {
            self.indent(indent + 1);
            self.put("reorder nameChildren = ");
            self.name_vector(order);
            self.put("\n");
        }
        // Properties by name in dictionary order, attributes before
        // relationships of the same name.
        let mut properties: Vec<(Path, SpecType)> = prim
            .tokens(field::PROPERTIES)
            .iter()
            .map(|name| path.property(name.clone()))
            .filter_map(|property| self.layer.spec(&property).map(|spec| (property, spec.kind)))
            .collect();
        properties.sort_by(|a, b| dictionary_order(a.0.name(), b.0.name()).then(a.1.cmp(&b.1)));
        for (property, kind) in &properties {
            if *kind == SpecType::Attribute {
                self.write_attribute(property, indent + 1)?;
            } else {
                self.write_relationship(property, indent + 1)?;
            }
        }
        let children: Vec<Path> = prim
            .tokens(field::PRIM_CHILDREN)
            .iter()
            .map(|name| path.child(name.clone()))
            .filter(|child| self.layer.has_spec(child))
            .collect();
        if !properties.is_empty() && !children.is_empty() {
            self.put("\n");
        }
        for (index, child) in children.iter().enumerate() {
            if index > 0 {
                self.put("\n");
            }
            self.write_prim(child, indent + 1)?;
        }
        for set in prim.tokens(field::VARIANT_SET_CHILDREN) {
            let set_path = path.variant(set.clone(), "");
            if self.layer.has_spec(&set_path) {
                self.write_variant_set(&set_path, path, indent + 1)?;
            }
        }
        Ok(())
    }

    /// `variantSet "name" = { ... }` with its variants in name order.
    fn write_variant_set(&mut self, set_path: &Path, prim: &Path, indent: usize) -> Result<()> {
        let set = self.layer.spec(set_path).expect("a variant set spec");
        let name = set_path.name_token();
        let mut variants: Vec<Path> = set
            .tokens(field::VARIANT_CHILDREN)
            .iter()
            .map(|variant| prim.variant(name.clone(), variant.clone()))
            .filter(|variant| self.layer.has_spec(variant))
            .collect();
        if variants.is_empty() {
            return Ok(());
        }
        variants.sort_by(|a, b| variant_name(a).as_bytes().cmp(variant_name(b).as_bytes()));
        self.indent(indent);
        self.put("variantSet ");
        self.quoted(&name);
        self.put(" = {\n");
        for variant in &variants {
            self.indent(indent + 1);
            self.quoted(variant_name(variant));
            self.prim_metadata(variant, indent + 1)?;
            self.put(" {\n");
            self.prim_body(variant, indent + 1)?;
            self.put("\n");
            self.indent(indent + 1);
            self.put("}\n");
        }
        self.indent(indent);
        self.put("}\n");
        Ok(())
    }

    /// A prim's relocates, each path relative to the prim, ordered by the
    /// relative source paths.
    fn prim_relocates(&mut self, indent: usize, prim: &Path, value: &Value) -> Result<()> {
        let Value::Relocates(pairs) = value else {
            return Ok(());
        };
        let mut relative = Vec::with_capacity(pairs.len());
        for (source, target) in pairs.iter() {
            let source = source.make_relative(prim)?;
            let target = if target.is_empty() {
                Path::empty()
            } else {
                target.make_relative(prim)?
            };
            relative.push((source, target));
        }
        relative.sort_by(|a, b| a.0.cmp(&b.0));
        self.relocates(indent, true, &relative);
        Ok(())
    }
}

fn variant_name(path: &Path) -> &str {
    path.variant_selection()
        .map_or("", |(_, variant)| variant.as_str())
}
