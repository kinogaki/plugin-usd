// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Prim definitions, after `UsdPrimDefinition` and `UsdSchemaRegistry`
//! (`pxr/usd/usd/primDefinition.cpp`, `schemaRegistry.cpp`): a prim's
//! built-in properties come from its typed schema, then the API schemas that
//! schema builds in, then its applied API schemas in order, each with the
//! API schemas it builds in; the first definition of a name wins. A
//! multiple-apply schema applied as `Name:instance` defines its properties
//! with the instance in place of `__INSTANCE_NAME__`. Only what flattening
//! needs is kept (`schema_table.rs`): each property's type, variability and
//! kind.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::schema_table::SCHEMA_TABLE;
use crate::usd::sdf::Token;

/// A built-in property.
#[derive(Clone, Debug)]
pub struct PropertyDefinition {
    /// The type name (`point3f[]`); empty for a relationship.
    pub type_name: Token,
    /// Whether the property is uniform.
    pub uniform: bool,
    /// Whether the property is a relationship.
    pub relationship: bool,
}

struct SchemaEntry {
    multiple: bool,
    /// Built-in API schemas, as applied: `Name` or `Name:instance`.
    builtins: Vec<&'static str>,
    /// Properties by name (a single-apply schema's).
    named: HashMap<&'static str, PropertyDefinition>,
    /// A multiple-apply schema's properties: the text before and after
    /// `__INSTANCE_NAME__`, and the definition.
    templates: Vec<(&'static str, &'static str, PropertyDefinition)>,
}

/// The schema table, parsed once.
fn schemas() -> &'static HashMap<&'static str, SchemaEntry> {
    static SCHEMAS: OnceLock<HashMap<&'static str, SchemaEntry>> = OnceLock::new();
    SCHEMAS.get_or_init(|| {
        let mut entries = HashMap::new();
        for row in SCHEMA_TABLE {
            let mut parts = row.splitn(3, '|');
            let name = parts.next().unwrap_or("");
            let builtins = parts.next().unwrap_or("");
            let properties = parts.next().unwrap_or("");
            let (name, multiple) = match name.strip_suffix('*') {
                Some(name) => (name, true),
                None => (name, false),
            };
            let builtins = builtins
                .split(',')
                .filter(|text| !text.is_empty())
                .collect();
            let properties: Vec<(&str, PropertyDefinition)> = properties
                .split(';')
                .filter(|text| !text.is_empty())
                .filter_map(|property| {
                    let mut words = property.split(' ');
                    let (name, type_name, flag) = (words.next()?, words.next()?, words.next()?);
                    let relationship = flag == "r";
                    Some((
                        name,
                        PropertyDefinition {
                            type_name: if relationship {
                                Token::empty()
                            } else {
                                Token::new(type_name)
                            },
                            uniform: flag == "u",
                            relationship,
                        },
                    ))
                })
                .collect();
            // The first entry of a name wins, as the registry's first plugin does.
            let mut named = HashMap::new();
            let mut templates = Vec::new();
            for (property, definition) in properties {
                match property.split_once("__INSTANCE_NAME__") {
                    Some((before, after)) => templates.push((before, after, definition)),
                    None => {
                        named.entry(property).or_insert(definition);
                    }
                }
            }
            entries.entry(name).or_insert(SchemaEntry {
                multiple,
                builtins,
                named,
                templates,
            });
        }
        entries
    })
}

/// The definition of property `name` on a prim of type `type_name` (empty:
/// none) with the API schemas `applied` (`Name` or `Name:instance`), if it
/// is built in.
pub fn find(type_name: &str, applied: &[Token], name: &str) -> Option<PropertyDefinition> {
    if !type_name.is_empty()
        && let Some(found) = find_in(type_name, name, 0)
    {
        return Some(found);
    }
    applied.iter().find_map(|api| find_in(api, name, 0))
}

/// `name` in the schema applied as `application`, or its built-ins.
fn find_in(application: &str, name: &str, depth: usize) -> Option<PropertyDefinition> {
    if depth > 8 {
        return None;
    }
    let (schema, instance) = application.split_once(':').unwrap_or((application, ""));
    let entry = schemas().get(schema)?;
    if entry.multiple && instance.is_empty() {
        return None;
    }
    if let Some(found) = entry.named.get(name) {
        return Some(found.clone());
    }
    for (before, after, definition) in &entry.templates {
        if name.len() == before.len() + instance.len() + after.len()
            && name.starts_with(before)
            && name.ends_with(after)
            && &name[before.len()..before.len() + instance.len()] == instance
        {
            return Some(definition.clone());
        }
    }
    entry
        .builtins
        .iter()
        .find_map(|builtin| find_in(builtin, name, depth + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_builtin_properties() {
        let points = find("Mesh", &[], "points").unwrap();
        assert_eq!(points.type_name.as_str(), "point3f[]");
        assert!(!points.uniform);
        assert!(find("Mesh", &[], "subdivisionScheme").unwrap().uniform);
        assert!(find("Mesh", &[], "nothing").is_none());
        let applied = [Token::new("CollectionAPI:lights")];
        assert!(
            find("", &applied, "collection:lights:includes")
                .unwrap()
                .relationship
        );
    }
}
