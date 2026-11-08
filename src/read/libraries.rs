//! Where a stage's Materials go: a Material Library for each prim holding
//! Materials, at that prim's place, as a USD Scope of Materials is one
//! (UsdShade puts no rule on where a Material is; `Looks`, `Materials` and
//! `/mtl` are habits).
//!
//! - A Scope holding Materials alone is the library itself, keeping its
//!   name and place.
//! - Materials beside other prims (under an Xform, a Scope holding more,
//!   Scopes of Materials among them, the root) go into a library named
//!   `Materials` in that place, said: a library holds Materials, never
//!   objects.
//! - Materials inside what Studio holds as one value (an instance's
//!   prototype, a point instancer's prototypes, a mesh) go into a library
//!   named `Materials` beside it, since a value holds no objects.
use crate::usd::scene::{Node, NodeKind, Scene};
use crate::usd::sdf::Path;
use std::collections::HashMap;

/// The name of a library made for Materials beside other prims.
pub(super) const MATERIALS: &str = "Materials";

/// Where a library is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Place {
    /// The node (a Scope of Materials) is the library.
    Node(usize),
    /// A library named [`MATERIALS`] under the node, or at the top.
    Under(Option<usize>),
}

/// The scene's libraries, each with its materials (indices into the
/// scene's), in the order their first material comes; what was gathered
/// into a library named [`MATERIALS`] beside other prims, said.
pub(super) fn libraries(scene: &Scene, warnings: &mut Vec<String>) -> Vec<(Place, Vec<usize>)> {
    let at: HashMap<Path, usize> = (0..scene.nodes.len()).map(|k| (scene.path(k), k)).collect();
    let scopes = material_scopes(&scene.nodes);
    let holder = |k: usize| (scene.nodes[k].parent, &scene.nodes[k].kind);
    let mut libraries: Vec<(Place, Vec<usize>)> = Vec::new();
    let mut index: HashMap<Place, usize> = HashMap::new();
    for (m, material) in scene.materials.iter().enumerate() {
        let parent = material.path.parent();
        let nearest = parent.ancestors().find_map(|a| at.get(&a).map(|&k| (a, k)));
        let (place, said) = match nearest {
            Some((a, k)) if a == parent && scopes[k] => (Place::Node(k), None),
            Some((_, k)) if scene.nodes[k].instance.is_some() => (
                Place::Under(scene.nodes[k].parent),
                Some(("inside an instance's prototype", "beside the instance")),
            ),
            Some((_, k)) => match holder(k) {
                (_, NodeKind::Group) => {
                    (Place::Under(Some(k)), Some(("beside other prims", "there")))
                }
                (parent, NodeKind::Instancer(_)) => (
                    Place::Under(parent),
                    Some(("inside a point instancer's prototypes", "beside it")),
                ),
                (parent, _) => (
                    Place::Under(parent),
                    Some(("inside a prim's value", "beside it")),
                ),
            },
            None => (
                Place::Under(None),
                Some(("beside other prims", "at the top")),
            ),
        };
        let k = *index.entry(place).or_insert_with(|| {
            if let Some((what, there)) = said {
                warnings.push(format!(
                    "{parent}: Materials {what} go into a library named {MATERIALS} {there}"
                ));
            }
            libraries.push((place, Vec::new()));
            libraries.len() - 1
        });
        libraries[k].1.push(m);
    }
    libraries
}

/// Whether each node holds Materials alone: a Scope grouping, not an
/// instance, holding no prim the scene reads (it is a node for its
/// Materials).
fn material_scopes(nodes: &[Node]) -> Vec<bool> {
    let mut only = (nodes.iter())
        .map(|n| {
            matches!(n.kind, NodeKind::Group)
                && n.instance.is_none()
                && n.type_name.as_str() == "Scope"
        })
        .collect::<Vec<_>>();
    for node in nodes {
        if let Some(p) = node.parent {
            only[p] = false;
        }
    }
    only
}
