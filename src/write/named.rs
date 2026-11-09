//! The objects lights name, as their prims: a geometry light is no prim of
//! its own but MeshLightAPI on the mesh it names (a mesh two lights name
//! takes the first's), and a portal names its dome, which lists it in its
//! `portals`. An item names another by its index ([`FileItem`]); a light
//! naming nothing written, or a mesh written as instances, is said and
//! left out.
use crate::lights;
use crate::usd::scene::{Light as UsdLight, NodeKind, Scene};
use crate::usd::sdf::Path;
use kinogaki_render_api::standard::LightShape;
use kinogaki_studio_api::color::ColorService;
use kinogaki_studio_api::{Error, FileItem, standard};
use std::collections::HashMap;

/// What the lights among a file's items name.
#[derive(Default)]
pub(super) struct Named {
    /// Each mesh's item, and the light it is.
    meshes: HashMap<usize, UsdLight>,
    /// Each dome's item, and its portals' items.
    portals: HashMap<usize, Vec<usize>>,
    /// The geometry lights' items, which make no prim.
    unwritten: Vec<usize>,
}

impl Named {
    /// What `items`' lights name, their colors through `colors`; what can't
    /// be written said into `warnings`.
    pub(super) fn of(
        items: &[FileItem],
        colors: &dyn ColorService,
        warnings: &mut Vec<String>,
    ) -> Result<Named, Error> {
        let mut named = Named::default();
        let holds = |i: usize, f: &dyn Fn(&kinogaki_studio_api::Data) -> bool| {
            (items.get(i).and_then(|item| item.data.as_ref())).is_some_and(f)
        };
        for (i, item) in items.iter().enumerate() {
            let Some(light) = item.data.as_ref().and_then(|d| standard::light().get(d)) else {
                continue;
            };
            match &light.shape {
                LightShape::Geometry { geometry, .. } => {
                    named.unwritten.push(i);
                    let mesh = geometry.map(|g| g.0 as usize);
                    let mesh =
                        mesh.filter(|&m| holds(m, &|d| standard::geometry().get(d).is_some()));
                    match mesh {
                        Some(m) if named.meshes.contains_key(&m) => warnings.push(format!(
                            "{}: another light emits from {} first; left out",
                            item.name, items[m].name
                        )),
                        Some(m) => {
                            let usd =
                                lights::usd_light(light, 1.0, lights::Files::default(), colors)?;
                            named.meshes.insert(m, usd);
                        }
                        None => warnings.push(format!(
                            "{}: emits from no mesh written; left out",
                            item.name
                        )),
                    }
                }
                LightShape::Portal {
                    dome: Some(dome), ..
                } => {
                    let d = dome.0 as usize;
                    let is_dome = |data: &kinogaki_studio_api::Data| {
                        standard::light()
                            .get(data)
                            .is_some_and(|l| matches!(l.shape, LightShape::Dome { .. }))
                    };
                    match holds(d, &is_dome) {
                        true => named.portals.entry(d).or_default().push(i),
                        false => warnings.push(format!(
                            "{}: its dome isn't a dome light written; it guides nothing",
                            item.name
                        )),
                    }
                }
                _ => {}
            }
        }
        Ok(named)
    }

    /// Whether item `i` makes no prim (a geometry light).
    pub(super) fn unwritten(&self, i: usize) -> bool {
        self.unwritten.contains(&i)
    }

    /// The light the nodes made of item `i` (`scene.nodes[at..]`) are: each
    /// Mesh among them takes it; said when none is a Mesh.
    pub(super) fn emit(
        &self,
        i: usize,
        item: &FileItem,
        scene: &mut Scene,
        at: usize,
        warnings: &mut Vec<String>,
    ) {
        let Some(light) = self.meshes.get(&i) else {
            return;
        };
        let mut meshes = 0;
        for node in &mut scene.nodes[at..] {
            if let NodeKind::Mesh(mesh) = &mut node.kind {
                mesh.light = Some(Box::new(light.clone()));
                meshes += 1;
            }
        }
        if meshes == 0 {
            warnings.push(format!(
                "{}: written as instances, its light is left out",
                item.name
            ));
        }
    }

    /// Each dome's `portals`: its portals' prims (`paths`, each item's),
    /// on its node (`node_of`, each item's).
    pub(super) fn list_portals(
        &self,
        scene: &mut Scene,
        node_of: &[Option<usize>],
        paths: &[Path],
    ) {
        for (&dome, portals) in &self.portals {
            let Some(node) = node_of[dome] else { continue };
            if let NodeKind::Light(light) = &mut scene.nodes[node].kind {
                light.portals = portals.iter().map(|&p| paths[p].clone()).collect();
            }
        }
    }
}
