//! The prims lights name, as the items made of them: each portal a dome
//! names takes the dome as its Dome (a portal two domes name keeps the
//! first's), and a GeometryLight's geometry is the mesh it emits from. An
//! item names another by its index ([`FileItem`]). A name reaching no item
//! of the kind it needs (a prim not read, a prototype's, another kind) is
//! said and names nothing.
use super::At;
use crate::usd::scene::{LightKind, NodeKind, Scene};
use crate::usd::sdf::Path;
use kinogaki_render_api::standard::{Light, LightShape};
use kinogaki_studio_api::{FileItem, ObjectRef, standard};
use std::collections::HashMap;

/// The light `item` holds, when it holds one.
fn light_of(item: &FileItem) -> Option<Light> {
    standard::light().get(item.data.as_ref()?).cloned()
}

/// `item` holding `light`, when it is a light value.
fn hold(item: &mut FileItem, light: Light) {
    if let Ok(data) = standard::light().wrap(light) {
        item.data = Some(data);
    }
}

/// The portals and geometry `scene`'s lights name, set on `items` (`at`:
/// each node's item).
pub(super) fn resolve(
    scene: &Scene,
    at: &[At],
    items: &mut [FileItem],
    warnings: &mut Vec<String>,
) {
    let naming: Vec<usize> = (scene.nodes.iter().enumerate())
        .filter(|(_, node)| match &node.kind {
            NodeKind::Light(light) => !light.portals.is_empty() || light.geometry.is_some(),
            _ => false,
        })
        .map(|(k, _)| k)
        .collect();
    if naming.is_empty() {
        return;
    }
    let item_at: HashMap<Path, usize> = (0..scene.nodes.len())
        .map(|k| (scene.path(k), at[k].item))
        .collect();
    for k in naming {
        let NodeKind::Light(light) = &scene.nodes[k].kind else {
            continue;
        };
        let (path, item) = (scene.path(k), at[k].item);
        let named = ObjectRef(item as u64);
        if light.kind == LightKind::Dome {
            for portal in &light.portals {
                let target = item_at.get(portal).copied();
                let held = target.and_then(|i| Some((i, light_of(&items[i])?)));
                match held {
                    Some((i, mut held)) => match &mut held.shape {
                        LightShape::Portal { dome: Some(_), .. } => warnings.push(format!(
                            "{portal}: another dome named it first; {path} lights through it there"
                        )),
                        LightShape::Portal { dome, .. } => {
                            *dome = Some(named);
                            hold(&mut items[i], held);
                        }
                        _ => warnings.push(format!(
                            "{path}: portal {portal} isn't a PortalLight; left out"
                        )),
                    },
                    None => warnings.push(format!("{path}: portal {portal} isn't read; left out")),
                }
            }
        }
        if let Some(geometry) = &light.geometry {
            let target = item_at.get(geometry).copied();
            let mesh = target.filter(|&i| {
                (items[i].data.as_ref()).is_some_and(|d| standard::geometry().get(d).is_some())
            });
            match (mesh, light_of(&items[item])) {
                (Some(mesh), Some(mut held)) => {
                    if let LightShape::Geometry { geometry, .. } = &mut held.shape {
                        *geometry = Some(ObjectRef(mesh as u64));
                    }
                    hold(&mut items[item], held);
                }
                _ => warnings.push(format!(
                    "{path}: its geometry {geometry} isn't a mesh read here; it emits from nothing"
                )),
            }
        }
    }
}
