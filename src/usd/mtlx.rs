//! MaterialX documents as layers, after UsdMtlx's file format
//! (`pxr/usd/usdMtlx/fileFormat.cpp`, `reader.cpp`): a `.mtlx` file a layer
//! references (`@look.mtlx@</MaterialX/Materials>`) composes as a layer
//! holding each of the document's materials as a Material at
//! `/MaterialX/Materials/<name>`.
//!
//! `kinogaki-mtlx` reads the document and its includes and flattens its node
//! graphs into each material ([`Document::materials`]): every node a
//! material reads is a Shader prim beneath it, its `info:id` the node's
//! MaterialX definition, its inputs of UsdMtlx's types ([`usd_type`])
//! holding their values or connected to the outputs they read, outputs named
//! as MaterialX names them. The shaders the material node reads feed its
//! terminals in the MaterialX render context (`outputs:mtlx:surface`,
//! `mtlx:displacement`, `mtlx:volume`). A color, and the file of an image
//! of colors, says the color space the document gives it (`colorSpace`, as
//! UsdMtlx writes it, the document's own included, which a layer's metadata
//! wouldn't carry across a reference); a file is an asset path relative to
//! the document, which composition anchors at the document.
//!
//! Where UsdMtlx keeps a document's node graphs as NodeGraph prims and a
//! surface's inputs as its material's interface, the graphs here are
//! flattened into their materials, which hold their values on their shaders.
//! Looks and their assignments are left out, said.

use super::error::{Error, Result};
use super::scene::Author;
use super::sdf::{LayerData, Path, Token, Value};
use super::shade::{self, Material, Port, ShadeKind, ShadeNode, Source};
use kinogaki_mtlx::{Bound, Document, Included, MaterialNode, Type, Value as Mtlx};
use std::collections::HashSet;

/// Whether `identifier` names a MaterialX document: a `.mtlx` file, a
/// package's entry too.
pub(crate) fn is_document(identifier: &str) -> bool {
    let file = identifier.trim_end_matches(']').to_ascii_lowercase();
    file.ends_with(".mtlx")
}

/// The layer the MaterialX document `text` (of identity `identity`) is, its
/// includes read through `load` ([`Document::include`]); with what was left
/// out, said.
///
/// # Errors
/// Text that isn't a MaterialX document.
pub(crate) fn layer(
    text: &str,
    identity: &str,
    load: &mut dyn FnMut(&str, Option<&str>) -> Option<Included>,
) -> Result<(LayerData, Vec<String>)> {
    let mut document = (text.parse::<Document>()).map_err(|why| Error::invalid(why.to_string()))?;
    let mut said = document.include(Some(identity), load);
    let (materials, left) = document.materials();
    said.extend(left);
    let root = Path::absolute_root().child("MaterialX").child("Materials");
    let mut names = HashSet::new();
    let mut author = Author::new();
    for m in &materials {
        let at = root.child(unique(prim_name(&m.name), &mut names));
        let material = material(m, at, &mut said);
        shade::write(&mut author, &material, &|asset| asset.clone())?;
    }
    Ok((author.finish(), said))
}

/// The USD type a MaterialX port of type `ty` is (UsdMtlx's): a closure,
/// shader or other type that holds no value a token.
pub(crate) fn usd_type(ty: &Type) -> &'static str {
    match ty {
        Type::Boolean => "bool",
        Type::Integer => "int",
        Type::Float => "float",
        Type::Color3 => "color3f",
        Type::Color4 => "color4f",
        Type::Vector2 => "float2",
        Type::Vector3 => "float3",
        Type::Vector4 => "float4",
        Type::Matrix33 => "matrix3d",
        Type::Matrix44 => "matrix4d",
        Type::String => "string",
        Type::Filename => "asset",
        Type::Named(_) => "token",
    }
}

/// The terminal each input of a material node feeds.
const TERMINALS: [(&str, &str); 3] = [
    ("surfaceshader", "mtlx:surface"),
    ("displacementshader", "mtlx:displacement"),
    ("volumeshader", "mtlx:volume"),
];

/// `m` as a Material at `at`: its material node the Material, every other
/// node a Shader beneath it.
fn material(m: &kinogaki_mtlx::Material, at: Path, said: &mut Vec<String>) -> Material {
    let mut out = Material::new(at.clone());
    let mut names = HashSet::new();
    let mut shader = vec![None; m.nodes.len()];
    for (k, node) in m.nodes.iter().enumerate() {
        if k == m.material {
            continue;
        }
        shader[k] = Some(out.nodes.len());
        let mut id = Port::new(
            "id",
            "token",
            Some(Value::Token(Token::new(&node.def.name))),
        );
        id.uniform = true;
        out.nodes.push(ShadeNode {
            path: at.child(unique(prim_name(&node.name), &mut names)),
            kind: ShadeKind::Shader,
            parent: None,
            info: vec![id],
            inputs: Vec::new(),
            outputs: Vec::new(),
        });
    }
    for (k, node) in m.nodes.iter().enumerate() {
        for (input, bound) in &node.inputs {
            match (shader[k], bound) {
                // The material node's inputs are its terminals.
                (None, Bound::Output { node: from, output }) => {
                    let Some((_, terminal)) = TERMINALS.iter().find(|(i, _)| i == input) else {
                        said.push(format!(
                            "{}: its {input} has no USD terminal; left out",
                            m.name
                        ));
                        continue;
                    };
                    if let Some(s) = shader[*from] {
                        let from = source(&mut out, s, output, "token");
                        out.outputs.push(connected(terminal, "token", from));
                    }
                }
                (None, Bound::Value { .. }) => {}
                (Some(at), Bound::Output { node: from, output }) => {
                    if let (Some(port), Some(s)) = (node.def.input(input), shader[*from]) {
                        let ty = usd_type(&port.ty);
                        let from = source(&mut out, s, output, ty);
                        out.nodes[at].inputs.push(connected(input, ty, from));
                    }
                }
                (Some(at), Bound::Value { value, colorspace }) => {
                    match valued(node, input, value, colorspace.as_deref()) {
                        Some(port) => out.nodes[at].inputs.push(port),
                        None => said.push(format!(
                            "{}: {}'s {input} holds a value USD can't; left out",
                            m.name, node.name
                        )),
                    }
                }
            }
        }
    }
    out
}

/// A connection to output `output` (of USD type `ty`) of `material`'s node
/// `node`, the output declared on it.
fn source(material: &mut Material, node: usize, output: &str, ty: &str) -> Source {
    let outputs = &mut material.nodes[node].outputs;
    if !outputs.iter().any(|o| o.name.as_str() == output) {
        outputs.push(Port::new(output, ty, None));
    }
    Source {
        node: Some(node),
        port: Token::new(output),
        output: true,
    }
}

/// A port `name` of USD type `ty` connected to `source`.
fn connected(name: &str, ty: &str, source: Source) -> Port {
    let mut port = Port::new(name, ty, None);
    port.sources.push(source);
    port
}

/// Input `input` of `node` holding `value` in color space `colorspace`; none
/// for a value USD can't hold, or an input its definition lacks.
fn valued(
    node: &MaterialNode,
    input: &str,
    value: &Mtlx,
    colorspace: Option<&str>,
) -> Option<Port> {
    let port = node.def.input(input)?;
    let mut out = Port::new(input, usd_type(&port.ty), Some(usd_value(value, &port.ty)?));
    out.uniform = port.uniform;
    // UsdMtlx's: colors, and the files of images of colors.
    let colors = match port.ty {
        Type::Filename => (node.def.outputs.iter()).any(|o| o.ty.is_color()),
        ref ty => ty.is_color(),
    };
    out.color_space = colorspace.filter(|_| colors).map(Token::new);
    Some(out)
}

/// A MaterialX value of type `ty` as USD holds it ([`usd_type`]); none for
/// one it can't (an empty file name, an integer past 32 bits).
fn usd_value(value: &Mtlx, ty: &Type) -> Option<Value> {
    let floats = |n: &[f64]| -> Vec<f32> { n.iter().map(|&v| v as f32).collect() };
    Some(match (ty, value) {
        (Type::Boolean, Mtlx::Boolean(on)) => Value::Bool(*on),
        (Type::Integer, Mtlx::Integer(n)) => Value::Int(i32::try_from(*n).ok()?),
        (Type::String, Mtlx::Text(text)) => Value::String(Token::new(text)),
        (Type::Filename, Mtlx::Text(file)) if !file.is_empty() => Value::Asset(Token::new(file)),
        (Type::Matrix33, Mtlx::Numbers(n)) => Value::Matrix3d(n.as_slice().try_into().ok()?),
        (Type::Matrix44, Mtlx::Numbers(n)) => Value::Matrix4d(n.as_slice().try_into().ok()?),
        (_, Mtlx::Numbers(n)) => match floats(n)[..] {
            [a] => Value::Float(a),
            [a, b] => Value::Vec2f([a, b]),
            [a, b, c] => Value::Vec3f([a, b, c]),
            [a, b, c, d] => Value::Vec4f([a, b, c, d]),
            _ => return None,
        },
        _ => return None,
    })
}

/// `name` as a prim name: letters, digits and `_`, a digit not first (a
/// namespaced MaterialX name's `:` too).
fn prim_name(name: &str) -> String {
    let mut valid: String = (name.chars())
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if !valid.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        valid.insert(0, '_');
    }
    valid
}

/// `name`, else `name_2`, `name_3`…: the first `taken` lacks, taken.
fn unique(name: String, taken: &mut HashSet<String>) -> String {
    let mut chosen = name.clone();
    let mut n = 1;
    while !taken.insert(chosen.clone()) {
        n += 1;
        chosen = format!("{name}_{n}");
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_prim_names_unique_in_their_scope() {
        let mut taken = HashSet::new();
        assert_eq!(unique(prim_name("ns:steel"), &mut taken), "ns_steel");
        assert_eq!(unique(prim_name("ns_steel"), &mut taken), "ns_steel_2");
        assert_eq!(prim_name("2x"), "_2x");
        assert!(is_document("/a/look.MTLX") && is_document("/a/b.usdz[look.mtlx]"));
        assert!(!is_document("/a/look.usda"));
    }
}
