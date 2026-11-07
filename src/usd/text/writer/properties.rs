// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Attributes and relationships (`Sdf_WriteAttribute`,
//! `Sdf_WriteRelationship`): the declaration line with the default value
//! and the metadata section, then `.timeSamples` and `.connect` statements;
//! a relationship's explicit targets on its line, list-edited targets on
//! their own lines.

use super::{Writer, order};
use crate::usd::error::Result;
use crate::usd::sdf::{ListOp, ListPart, Path, Value, Variability, field, serialization_name};

fn is_uniform(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Variability(Variability::Uniform)))
}

fn is_true(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Bool(true)))
}

impl Writer<'_> {
    pub(super) fn write_attribute(&mut self, path: &Path, indent: usize) -> Result<()> {
        let attribute = self.layer.spec(path).expect("an attribute spec");
        let name = path.name();
        let uniform = is_uniform(attribute.get(field::VARIABILITY));
        let type_text = attribute
            .get(field::TYPE_NAME)
            .and_then(Value::as_token)
            .map(|text| serialization_name(text).into_owned())
            .unwrap_or_default();
        let custom = is_true(attribute.get(field::CUSTOM));
        let comment = attribute
            .get(field::COMMENT)
            .and_then(Value::as_token)
            .filter(|text| !text.is_empty());
        let default = attribute.get(field::DEFAULT);
        let samples = attribute.get(field::TIME_SAMPLES);
        let connections = attribute.get(field::CONNECTION_PATHS);
        let fields = order::metadata_fields(attribute, false);
        let info = comment.is_some() || !fields.is_empty();
        if info || default.is_some() || custom || (connections.is_none() && samples.is_none()) {
            self.indent(indent);
            if custom {
                self.put("custom ");
            }
            if uniform {
                self.put("uniform ");
            }
            self.put(&type_text);
            self.byte(b' ');
            self.put(name);
            match default {
                // Sdf writes a path default after the indentation, not " = ".
                Some(Value::Path(target)) => {
                    self.indent(indent);
                    self.path_ref(target);
                }
                Some(value) => {
                    self.put(" = ");
                    self.value(value)?;
                }
                None => {}
            }
            if info {
                self.put(" (\n");
                if let Some(comment) = comment {
                    self.indent(indent + 1);
                    self.quoted(comment);
                    self.put("\n");
                }
                for (field_name, value) in fields {
                    match field_name.as_str() {
                        field::DOCUMENTATION => self.documentation(indent + 1, value),
                        field::PERMISSION => self.permission(indent + 1, value),
                        field::SYMMETRY_FUNCTION => self.symmetry_function(indent + 1, value),
                        field::DISPLAY_UNIT => {
                            self.indent(indent + 1);
                            self.put("displayUnit = ");
                            self.put(value.as_token().map_or("", |text| text.as_str()));
                            self.put("\n");
                        }
                        _ => self.simple_field(indent + 1, field_name, value)?,
                    }
                }
                self.indent(indent);
                self.put(")");
            }
            self.put("\n");
        }
        if let Some(Value::TimeSamples(samples)) = samples {
            self.indent(indent);
            if uniform {
                self.put("uniform ");
            }
            self.put(&type_text);
            self.byte(b' ');
            self.put(name);
            self.put(".timeSamples = {\n");
            for (time, value) in samples.iter() {
                self.indent(indent + 1);
                crate::usd::text::number::write_f64(&mut self.out, *time);
                self.put(": ");
                self.value(value)?;
                self.put(",\n");
            }
            self.indent(indent);
            self.put("}\n");
        }
        if let Some(Value::PathListOp(list)) = connections {
            self.connection_list(indent, list, uniform, &type_text, name);
        }
        Ok(())
    }

    fn documentation(&mut self, indent: usize, value: &Value) {
        self.indent(indent);
        self.put("doc = ");
        self.quoted(value.as_token().map_or("", |text| text.as_str()));
        self.put("\n");
    }

    /// `.connect` statements: explicit, or one per list op part.
    fn connection_list(
        &mut self,
        indent: usize,
        list: &ListOp<Path>,
        uniform: bool,
        type_text: &str,
        name: &str,
    ) {
        if list.explicit {
            self.connection_statement(indent, &list.explicit_items, "", uniform, type_text, name);
            return;
        }
        for part in ListPart::EDITS {
            let items = list.items(part);
            if !items.is_empty() {
                let lead = format!("{} ", part.keyword());
                self.connection_statement(indent, items, &lead, uniform, type_text, name);
            }
        }
    }

    fn connection_statement(
        &mut self,
        indent: usize,
        items: &[Path],
        lead: &str,
        uniform: bool,
        type_text: &str,
        name: &str,
    ) {
        self.indent(indent);
        self.put(lead);
        if uniform {
            self.put("uniform ");
        }
        self.put(type_text);
        self.byte(b' ');
        self.put(name);
        self.put(".connect = ");
        match items {
            [] => self.put("None\n"),
            [only] => {
                self.path_ref(only);
                self.put("\n");
            }
            _ => {
                self.put("[\n");
                for item in items {
                    self.indent(indent + 1);
                    self.path_ref(item);
                    self.put(",\n");
                }
                self.indent(indent);
                self.put("]\n");
            }
        }
    }

    pub(super) fn write_relationship(&mut self, path: &Path, indent: usize) -> Result<()> {
        let relationship = self.layer.spec(path).expect("a relationship spec");
        let name = path.name();
        let comment = relationship
            .get(field::COMMENT)
            .and_then(Value::as_token)
            .filter(|text| !text.is_empty());
        let custom = is_true(relationship.get(field::CUSTOM));
        let varying = !is_uniform(relationship.get(field::VARIABILITY));
        let fields = order::metadata_fields(relationship, false);
        let info = comment.is_some() || !fields.is_empty();
        let list = match relationship.get(field::TARGET_PATHS) {
            Some(Value::PathListOp(list)) => Some(list.clone()),
            _ => None,
        };
        let explicit_targets = list.as_ref().is_some_and(|list| list.explicit);
        let target_edits = list
            .as_ref()
            .is_some_and(|list| !list.explicit && list.has_items());
        if info || explicit_targets || (!target_edits && !custom) {
            self.indent(indent);
            if custom {
                self.put("custom ");
            }
            if varying {
                self.put("varying ");
            }
            self.put("rel ");
            self.put(name);
            if explicit_targets {
                let items = &list.as_ref().expect("targets").explicit_items;
                if items.is_empty() {
                    self.put(" = None");
                } else {
                    self.target_list(indent, items, false);
                }
            }
            if info {
                self.put(" (\n");
                if let Some(comment) = comment {
                    self.indent(indent + 1);
                    self.quoted(comment);
                    self.put("\n");
                }
                for (field_name, value) in fields {
                    match field_name.as_str() {
                        field::DOCUMENTATION => self.documentation(indent + 1, value),
                        field::PERMISSION => self.permission(indent + 1, value),
                        field::SYMMETRY_FUNCTION => self.symmetry_function(indent + 1, value),
                        _ => self.simple_field(indent + 1, field_name, value)?,
                    }
                }
                self.indent(indent);
                self.put(")");
            }
            self.put("\n");
        } else if custom {
            self.indent(indent);
            self.put("custom ");
            if varying {
                self.put("varying ");
            }
            self.put("rel ");
            self.put(name);
            self.put("\n");
        }
        if target_edits {
            let list = list.expect("targets");
            for part in ListPart::EDITS {
                let items = list.items(part);
                if items.is_empty() {
                    continue;
                }
                self.indent(indent);
                self.put(part.keyword());
                self.byte(b' ');
                if varying {
                    self.put("varying ");
                }
                self.put("rel ");
                self.put(name);
                self.target_list(indent, items, true);
            }
        }
        if let Some(Value::Path(target)) = relationship.get(field::DEFAULT) {
            self.indent(indent);
            if varying {
                self.put("varying ");
            }
            self.put("rel ");
            self.put(name);
            self.put(".default = ");
            self.path_ref(target);
            // Sdf writes the line end after the indentation.
            self.indent(indent);
            self.put("\n");
        }
        Ok(())
    }

    /// ` = <path>` or ` = [ ... ]`, with a line end when `newline`.
    fn target_list(&mut self, indent: usize, items: &[Path], newline: bool) {
        if items.len() > 1 {
            self.put(" = [\n");
            for item in items {
                self.indent(indent + 1);
                self.path_ref(item);
                self.put(",\n");
            }
            self.indent(indent);
            self.put("]");
        } else {
            self.put(" = ");
            self.path_ref(&items[0]);
        }
        if newline {
            self.put("\n");
        }
    }
}
