//! A material authored as its Material prim and network: every node at its
//! path, every port with its type, value, color space and connections.

use std::sync::Arc;

use super::{Material, Port, ShadeKind, Source};
use crate::usd::error::{Error, Result};
use crate::usd::scene::Author;
use crate::usd::sdf::{Path, Token, Value, Variability, field};

/// Author `material` (its ancestors made as Scopes where missing). Asset
/// values pass through `asset`, which gives the path written for each.
pub(crate) fn write(
    author: &mut Author,
    material: &Material,
    asset: &dyn Fn(&Token) -> Token,
) -> Result<()> {
    let path = &material.path;
    if !path.is_absolute() || !path.is_prim() || path.is_absolute_root() {
        return Err(Error::invalid(format!(
            "{path} is not a prim path for a material"
        )));
    }
    if author.has(path) {
        return Err(Error::invalid(format!("{path} is written twice")));
    }
    scopes(author, &path.parent());
    author.prim(path, "Material");
    if !material.custom_data.is_empty() {
        let data = Value::Dictionary(Arc::new(material.custom_data.clone()));
        author.prim_metadata(path, field::CUSTOM_DATA, data);
    }
    let writer = Writer { material, asset };
    writer.ports(author, path, "inputs:", &material.inputs);
    writer.ports(author, path, "outputs:", &material.outputs);
    for node in &material.nodes {
        if !node.path.has_prefix(path) || node.path == *path {
            return Err(Error::invalid(format!(
                "the node {} is not beneath its material {path}",
                node.path
            )));
        }
        scopes(author, &node.path.parent());
        let type_name = match node.kind {
            ShadeKind::Shader => "Shader",
            ShadeKind::NodeGraph => "NodeGraph",
        };
        author.prim(&node.path, type_name);
        writer.ports(author, &node.path, "info:", &node.info);
        writer.ports(author, &node.path, "inputs:", &node.inputs);
        writer.ports(author, &node.path, "outputs:", &node.outputs);
    }
    Ok(())
}

/// `path` and its missing ancestors made as Scopes.
fn scopes(author: &mut Author, path: &Path) {
    if path.is_absolute_root() || author.has(path) {
        return;
    }
    scopes(author, &path.parent());
    author.prim(path, "Scope");
}

struct Writer<'a> {
    material: &'a Material,
    asset: &'a dyn Fn(&Token) -> Token,
}

impl Writer<'_> {
    /// Author `ports` on `prim`, each named `prefix` and its name.
    fn ports(&self, author: &mut Author, prim: &Path, prefix: &str, ports: &[Port]) {
        for port in ports {
            let name = format!("{prefix}{}", port.name);
            let attribute = match &port.value {
                Some(value) => {
                    let value = self.assets_in(value);
                    author.attribute(prim, &name, &port.type_name, value, port.uniform)
                }
                None => {
                    let attribute = author.declare(prim, &name, &port.type_name);
                    let variability = if port.uniform {
                        Variability::Uniform
                    } else {
                        Variability::Varying
                    };
                    author.property_metadata(
                        &attribute,
                        field::VARIABILITY,
                        Value::Variability(variability),
                    );
                    attribute
                }
            };
            if let Some(space) = &port.color_space {
                author.property_metadata(
                    &attribute,
                    field::COLOR_SPACE,
                    Value::Token(space.clone()),
                );
            }
            if !port.sources.is_empty() {
                let sources = port
                    .sources
                    .iter()
                    .map(|source| self.target(source))
                    .collect();
                author.connect(&attribute, sources);
            }
        }
    }

    /// The attribute path a connection names.
    fn target(&self, source: &Source) -> Path {
        let prim = match source.node.and_then(|node| self.material.nodes.get(node)) {
            Some(node) => &node.path,
            None => &self.material.path,
        };
        let prefix = if source.output { "outputs:" } else { "inputs:" };
        prim.property(format!("{prefix}{}", source.port))
    }

    /// `value` with its asset paths as written.
    fn assets_in(&self, value: &Value) -> Value {
        match value {
            Value::Asset(path) if !path.is_empty() => Value::Asset((self.asset)(path)),
            other => other.clone(),
        }
    }
}
