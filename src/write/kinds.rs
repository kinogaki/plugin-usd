//! The model kinds an export writes, so other applications see the scene's
//! models (OpenUSD's model hierarchy, glossary.rst:2079-2110: only group
//! models hold models, and a model's parent is a group model, so the
//! hierarchy is contiguous from the default prim):
//!
//! - the default prim is an `assembly`;
//! - a group (an Xform: a Part, an object holding its value beside its
//!   children) is a `group`;
//! - a prim showing geometry or instances is a `component`;
//! - a Scope (a Material Library), a light and a camera are no models;
//! - each prototype instances show is a `component` (a `group` when it holds
//!   instances of its own), which the instances' prims take through their
//!   reference, as the Open Chess Set's instanced bishops are components.
//!
//! A prim takes its kind only beneath a group model; beneath anything
//! else (a component, a Scope) it is no model.
use crate::usd::scene::{Node, NodeKind, Scene};
use crate::usd::sdf::Token;

/// `scene` with the model kinds its nodes and prototypes are written of.
pub(super) fn assign(scene: &mut Scene) {
    let mut groups: Vec<bool> = Vec::with_capacity(scene.nodes.len());
    for node in &mut scene.nodes {
        let model = match node.parent {
            None => "assembly",
            Some(p) if groups[p] => kind(node),
            Some(_) => "",
        };
        groups.push(matches!(model, "assembly" | "group"));
        node.model = Token::new(model);
    }
    for prototype in &mut scene.prototypes {
        let nested = prototype
            .nodes
            .iter()
            .any(|n| n.instance.is_some() || matches!(n.kind, NodeKind::Instancer(_)));
        prototype.model = Token::new(if nested { "group" } else { "component" });
    }
}

/// The kind `node` takes beneath a group model.
fn kind(node: &Node) -> &'static str {
    match (&node.kind, node.instance) {
        (_, Some(_)) => "component",
        (NodeKind::Group, None) if node.type_name.as_str() == "Scope" => "",
        (NodeKind::Group, None) => "group",
        (NodeKind::Camera(_) | NodeKind::Light(_), None) => "",
        _ => "component",
    }
}
