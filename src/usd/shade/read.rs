//! A Material prim's network read from a (flattened) layer: the Shader and
//! NodeGraph prims beneath it (inside Scopes too), each one's `info:*`,
//! inputs and outputs, and connections resolved to the nodes they name.

use super::{Material, Port, ShadeKind, ShadeNode, Source};
use crate::usd::scene::values::{Time, color_space, value_at};
use crate::usd::sdf::identifier::dictionary_order;
use crate::usd::sdf::{
    Array, LayerData, Path, PathMap, SpecType, Specifier, Token, Value, Variability, field,
};

/// The deepest nesting of prims beneath a material walked.
const MOST_DEPTH: usize = 64;

/// Read the Material at `path`. Asset values pass through `asset`, which
/// gives the path the scene names each by; connections to prims outside
/// the material are left out, each said in `warnings`.
pub(crate) fn read(
    layer: &LayerData,
    path: &Path,
    time: Time,
    asset: &mut dyn FnMut(&Token) -> Token,
    warnings: &mut Vec<String>,
) -> Material {
    let mut material = Material::new(path.clone());
    if let Some(Value::Dictionary(data)) = layer.get(path, field::CUSTOM_DATA) {
        material.custom_data = (**data).clone();
    }
    let mut found = Vec::new();
    collect(layer, path, None, 0, &mut found);
    let index: PathMap<usize> = (found.iter().enumerate())
        .map(|(at, (node, _, _))| (node.clone(), at))
        .collect();
    let mut reader = Reader {
        layer,
        material: path,
        index: &index,
        time,
        asset,
        warnings,
    };
    material.inputs = reader.ports(path, "inputs:");
    material.outputs = reader.ports(path, "outputs:");
    for (node, kind, parent) in &found {
        let shade = ShadeNode {
            path: node.clone(),
            kind: *kind,
            parent: *parent,
            info: reader.ports(node, "info:"),
            inputs: reader.ports(node, "inputs:"),
            outputs: reader.ports(node, "outputs:"),
        };
        material.nodes.push(shade);
    }
    material
}

/// The Shader and NodeGraph prims beneath `prim`, depth first, each with
/// its kind and the index of the NodeGraph holding it.
fn collect(
    layer: &LayerData,
    prim: &Path,
    parent: Option<usize>,
    depth: usize,
    found: &mut Vec<(Path, ShadeKind, Option<usize>)>,
) {
    if depth > MOST_DEPTH {
        return;
    }
    for name in layer
        .spec(prim)
        .map_or(&[][..], |spec| spec.tokens(field::PRIM_CHILDREN))
    {
        let child = prim.child(name.clone());
        let Some(spec) = layer.spec(&child) else {
            continue;
        };
        let defined =
            spec.get(field::SPECIFIER).and_then(Value::as_specifier) == Some(Specifier::Def);
        let active = spec.get(field::ACTIVE).and_then(Value::as_bool) != Some(false);
        if !defined || !active {
            continue;
        }
        let type_name = spec.get(field::TYPE_NAME).and_then(Value::as_token);
        match type_name.map(|t| t.as_str()) {
            Some("Shader") => found.push((child, ShadeKind::Shader, parent)),
            Some("NodeGraph") => {
                found.push((child.clone(), ShadeKind::NodeGraph, parent));
                let graph = found.len() - 1;
                collect(layer, &child, Some(graph), depth + 1, found);
            }
            // Another material's network is its own.
            Some("Material") => {}
            _ => collect(layer, &child, parent, depth + 1, found),
        }
    }
}

struct Reader<'a> {
    layer: &'a LayerData,
    material: &'a Path,
    index: &'a PathMap<usize>,
    time: Time,
    asset: &'a mut dyn FnMut(&Token) -> Token,
    warnings: &'a mut Vec<String>,
}

impl Reader<'_> {
    /// The attributes of `prim` named `prefix…`, as ports in name order
    /// (as a composed prim lists its properties, and usda prints them).
    fn ports(&mut self, prim: &Path, prefix: &str) -> Vec<Port> {
        let properties = self
            .layer
            .spec(prim)
            .map_or(&[][..], |spec| spec.tokens(field::PROPERTIES));
        let mut ports = Vec::new();
        for property in properties {
            let Some(name) = property.strip_prefix(prefix) else {
                continue;
            };
            let attribute = prim.property(property.clone());
            let Some(spec) = self.layer.spec(&attribute) else {
                continue;
            };
            if spec.kind != SpecType::Attribute {
                continue;
            }
            let type_name = (spec.get(field::TYPE_NAME))
                .and_then(Value::as_token)
                .cloned()
                .unwrap_or_default();
            // An attribute's value (a path some files author as one is not).
            let value = value_at(self.layer, prim, property, self.time)
                .filter(|value| value.attribute_kind().is_some())
                .map(|value| self.assets_in(value));
            let sources = self.sources(&attribute);
            let uniform =
                spec.get(field::VARIABILITY) == Some(&Value::Variability(Variability::Uniform));
            ports.push(Port {
                name: Token::new(name),
                type_name,
                uniform,
                value,
                sources,
                color_space: color_space(self.layer, prim, property),
            });
        }
        ports.sort_by(|a, b| dictionary_order(&a.name, &b.name));
        ports
    }

    /// The connections of `attribute`, resolved to the material's nodes.
    fn sources(&mut self, attribute: &Path) -> Vec<Source> {
        let Some(Value::PathListOp(list)) = self.layer.get(attribute, field::CONNECTION_PATHS)
        else {
            return Vec::new();
        };
        let mut sources = Vec::new();
        for target in list.applied() {
            let prim = target.prim_path();
            let node = if prim == *self.material {
                None
            } else if let Some(&node) = self.index.get(&prim) {
                Some(node)
            } else {
                self.warnings.push(format!(
                    "{attribute}: its connection to {target} leaves the material {}; left out",
                    self.material
                ));
                continue;
            };
            let name = target.name();
            let (port, output) = match (name.strip_prefix("outputs:"), name.strip_prefix("inputs:"))
            {
                (Some(port), _) => (port, true),
                (None, Some(port)) => (port, false),
                (None, None) => continue,
            };
            sources.push(Source {
                node,
                port: Token::new(port),
                output,
            });
        }
        sources
    }

    /// `value` with its asset paths named as the scene names them.
    fn assets_in(&mut self, value: Value) -> Value {
        match value {
            Value::Asset(path) if !path.is_empty() => Value::Asset((self.asset)(&path)),
            Value::Array(Array::Asset(paths)) => Value::Array(Array::Asset(std::sync::Arc::new(
                (paths.iter())
                    .map(|path| {
                        if path.is_empty() {
                            path.clone()
                        } else {
                            (self.asset)(path)
                        }
                    })
                    .collect(),
            ))),
            other => other,
        }
    }
}
