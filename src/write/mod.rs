//! Items written as one USD layer: a usda, a usdc, or a usdz carrying the
//! files its materials and domes name under `textures/` (a usda or usdc
//! names them as every written document does: [`assets`]).
//!
//! The stage is in the project's units (`metersPerUnit`, `upAxis`), its
//! default prim the one group the items are under (else one named after the
//! file holding them), its materials in that prim's `Looks`. Each item is
//! its prim, placed relative to its parent's: polygons and curves
//! ([`geometry`]), what the host makes polygons of (a B-rep), lights and
//! cameras ([`crate::lights`]), and instances as copies of their
//! prototypes, each instance's material bound on its copy; an instance
//! whose `material_map` renders the prototype's materials as others places
//! a prototype written in those, once a map.
mod assets;
mod geometry;
mod materials;
mod preview;

use crate::lights;
use crate::names;
use crate::space::matrix;
use crate::usd::scene::{Matrix4d, Node, NodeKind, Prototype, Scene, UpAxis as StageUp};
use crate::usd::sdf::{Path, Token};
use assets::Assets;
use geometry::Shaping;
use kinogaki_render_api::standard::LightShape;
use kinogaki_studio_api::format::Unique;
use kinogaki_studio_api::geometry::{Domain, UpAxis, names as standard_names};
use kinogaki_studio_api::standard::{self, Instances};
use kinogaki_studio_api::value::ImageRef;
use kinogaki_studio_api::{
    Data, DataTypeId, Diagnostic, Error, FileItem, FormatOptions, FormatWriter, Writing,
};
use std::collections::HashMap;
use std::path::Path as FilePath;
use std::sync::{Arc, LazyLock};

pub(crate) struct Writer;

/// What it writes: what a read gives, and B-reps through the host's mesher.
static WRITES: LazyLock<Vec<DataTypeId>> = LazyLock::new(|| {
    let mut types = super::TYPES.to_vec();
    types.push(standard::brep().id());
    types
});

impl FormatWriter for Writer {
    fn accepts(&self) -> &[DataTypeId] {
        &WRITES
    }

    fn write(
        &self,
        path: &FilePath,
        items: &[FileItem],
        writing: &Writing<'_>,
        _: &FormatOptions,
    ) -> Result<Vec<Diagnostic>, Error> {
        let units = writing.units;
        let mut out = Out {
            scene: Scene {
                up_axis: match units.up() {
                    UpAxis::Y => StageUp::Y,
                    UpAxis::Z => StageUp::Z,
                },
                meters_per_unit: units.meters_per_unit(),
                ..Scene::default()
            },
            assets: Assets::of(path),
            materials: HashMap::new(),
            double_sided: Vec::new(),
            prototypes: Vec::new(),
            writing,
            warnings: Vec::new(),
        };
        // The default prim: the one group at the top, else one named after
        // the file.
        let shown =
            (0..items.len()).filter(|&i| items[i].parent.is_none() && !is_materials(&items[i]));
        let root_item = match shown.collect::<Vec<_>>()[..] {
            [one] if items[one].data.is_none() => Some(one),
            _ => None,
        };
        let root_name = match root_item {
            Some(item) => names::identifier(&items[item].name).into_owned(),
            None => {
                let stem = path
                    .file_stem()
                    .map_or("Scene".into(), |s| s.to_string_lossy());
                names::identifier(&stem).into_owned()
            }
        };
        let root = Path::absolute_root().child(Token::new(&root_name));
        let mut root_children = Unique::default();
        let looks = root.child(Token::new(&root_children.take("Looks")));
        for item in items.iter().filter(|i| is_materials(i)) {
            out.materials_item(item, &looks)?;
        }
        out.scene.nodes.push(Node {
            visible: root_item.is_none_or(|i| items[i].visible),
            transform: root_item.map_or(Matrix4d::IDENTITY, |i| matrix(&items[i].placement)),
            ..Node::group(&root_name, None)
        });
        // Each item's node, under its parent's (the root's for the top ones).
        let mut node_of: Vec<Option<usize>> = vec![None; items.len()];
        let mut children: HashMap<usize, Unique> = HashMap::from([(0, root_children)]);
        for (i, item) in items.iter().enumerate() {
            writing.check()?;
            if is_materials(item) {
                continue;
            }
            if Some(i) == root_item {
                node_of[i] = Some(0);
                continue;
            }
            let parent = item.parent.and_then(|p| node_of[p]).unwrap_or(0);
            let name = (children.entry(parent).or_default()).take(&names::identifier(&item.name));
            node_of[i] = Some(out.item(item, &name, parent)?);
        }
        out.scene.prototypes = std::mem::take(&mut out.prototypes)
            .into_iter()
            .map(|(_, p)| p)
            .collect();
        let (assets, unpacked) = std::mem::replace(&mut out.assets, Assets::of(path)).assets();
        out.scene.assets = assets;
        out.warnings.extend(unpacked);
        writing.check()?;
        crate::usd::scene::write_file(&out.scene, path)
            .map_err(|e| Error::invalid(format!("{}: {e}", path.display())))?;
        Ok(out.warnings.into_iter().map(Diagnostic::warning).collect())
    }
}

/// Whether `item` holds the file's materials.
fn is_materials(item: &FileItem) -> bool {
    (item.data.as_ref()).is_some_and(|d| standard::materials().get(d).is_some())
}

/// What a value is written as.
enum Kinds {
    /// Prims of its own: the item's (an empty suffix), then children, each
    /// with its material.
    Prims(Vec<(&'static str, NodeKind, Option<usize>)>),
    /// One prim's kind.
    One(NodeKind),
    /// Copies of prototypes, children of the item's prim.
    Instances(Vec<Node>),
}

/// The scene being written.
struct Out<'w> {
    scene: Scene,
    assets: Assets,
    /// Each material's index in the scene, by its key.
    materials: HashMap<Arc<str>, usize>,
    /// Whether each of the scene's materials is double-sided.
    double_sided: Vec<bool>,
    /// Each prototype written, by what it is written of.
    prototypes: Vec<(Of, Prototype)>,
    writing: &'w Writing<'w>,
    warnings: Vec<String>,
}

/// What a prototype is written of: a value, and the material map its
/// instances render it with (pairs of a material its primitives name and
/// the one they render with; empty: its own materials).
type Of = (Data, Vec<(String, String)>);

impl Out<'_> {
    /// The materials and images `item` holds, the materials under `looks`.
    fn materials_item(&mut self, item: &FileItem, looks: &Path) -> Result<(), Error> {
        let held = item
            .data
            .as_ref()
            .and_then(|d| standard::materials().get(d));
        let Some(held) = held else { return Ok(()) };
        let mut names = Unique::default();
        for material in &held.materials {
            let name = names.take(&names::identifier(&material.name));
            let path = looks.child(Token::new(&name));
            let colors = self.writing.colors;
            let made =
                materials::material(material, path, &mut self.assets, colors, &mut self.warnings)?;
            self.materials
                .insert(Arc::from(material.key.as_str()), self.scene.materials.len());
            self.double_sided.push(material.double_sided);
            self.scene.materials.push(made);
        }
        Ok(())
    }

    /// `item` as the node `name` under node `parent`; its index.
    fn item(&mut self, item: &FileItem, name: &str, parent: usize) -> Result<usize, Error> {
        let mut node = Node {
            transform: matrix(&item.placement),
            visible: item.visible,
            ..Node::group(name, Some(parent))
        };
        let at = self.scene.nodes.len();
        let mut children: Vec<Node> = Vec::new();
        let kinds = match &item.data {
            Some(data) => self.kinds(data, &[], &item.name)?,
            None => None,
        };
        match kinds {
            Some(Kinds::Prims(prims)) => {
                let mut names = Unique::default();
                for (k, (suffix, kind, material)) in prims.into_iter().enumerate() {
                    if k == 0 && suffix.is_empty() {
                        (node.kind, node.material) = (kind, material);
                        continue;
                    }
                    let child = names.take(if suffix.is_empty() { "Shape" } else { suffix });
                    children.push(Node {
                        kind,
                        material,
                        ..Node::group(&child, Some(at))
                    });
                }
            }
            Some(Kinds::One(kind)) => node.kind = kind,
            Some(Kinds::Instances(copies)) => {
                children.extend(copies.into_iter().map(|copy| Node {
                    parent: Some(at),
                    ..copy
                }));
            }
            None => {}
        }
        self.scene.nodes.push(node);
        self.scene.nodes.extend(children);
        Ok(at)
    }

    /// What `data` (of the item named `name`) is written as, its polygons'
    /// materials through `map` (pairs of a material they name and the one
    /// they render with instead).
    fn kinds(
        &mut self,
        data: &Data,
        map: &[(&str, &str)],
        name: &str,
    ) -> Result<Option<Kinds>, Error> {
        if let Some(instances) = standard::instances().get(data) {
            return Ok(Some(Kinds::Instances(self.copies(instances)?)));
        }
        if let Some(light) = standard::light().get(data) {
            let texture = match &light.shape {
                LightShape::Dome {
                    image: Some(image), ..
                } => self.dome(image),
                _ => None,
            };
            let usd = lights::usd_light(light, 1.0, texture, self.writing.colors)?;
            return Ok(Some(Kinds::One(NodeKind::Light(Box::new(usd)))));
        }
        if let Some(camera) = standard::camera().get(data) {
            let usd = lights::usd_camera(camera, self.scene.meters_per_unit, 1.0);
            return Ok(Some(Kinds::One(NodeKind::Camera(Box::new(usd)))));
        }
        let meshed;
        let value = match standard::geometry().get(data) {
            Some(value) => value,
            None => match (self.writing.meshed)(data) {
                Some(value) => {
                    meshed = value;
                    &meshed
                }
                None => {
                    let said = format!("{name}: nothing here makes polygons of it; left out");
                    self.warnings.push(said);
                    return Ok(None);
                }
            },
        };
        let rendered = standard::materials_mapped(value, map)?;
        let shaping = Shaping {
            materials: &self.materials,
            double_sided: &self.double_sided,
            working: self.writing.colors.working(),
        };
        let value = rendered.as_ref().unwrap_or(value);
        let prims = geometry::prims(value, &shaping, &mut self.warnings);
        Ok(Some(Kinds::Prims(prims.prims)))
    }

    /// The asset a dome's image names: its file, packed or written beside
    /// the layer when held embedded. A node's image is Export's to resolve
    /// to its file; one left is said.
    fn dome(&mut self, image: &ImageRef) -> Option<Token> {
        let file = match image {
            ImageRef::File(source) => source.file(),
            ImageRef::Node(path) => {
                let said =
                    format!("a dome names the Image node {path}, which isn't written; left out");
                self.warnings.push(said);
                return None;
            }
        };
        match self.assets.file(file) {
            Ok(asset) => Some(asset),
            Err(e) => {
                self.warnings
                    .push(format!("a dome's image: {}; left out", e.message()));
                None
            }
        }
    }

    /// `instances` as copies: a node a placement, showing its prototype,
    /// binding the instance's material.
    fn copies(&mut self, instances: &Instances) -> Result<Vec<Node>, Error> {
        let attributes = instances.attributes();
        let named = attributes.text(Domain::Instance, standard_names::NAME);
        let bound = attributes.text(Domain::Instance, standard_names::MATERIAL);
        let mut prototypes = Vec::with_capacity(instances.prototypes().len());
        for prototype in instances.prototypes().iter() {
            prototypes.push(self.prototype(prototype, &[])?);
        }
        let mut names = Unique::default();
        let mut copies = Vec::with_capacity(instances.len());
        for i in 0..instances.len() {
            let key = bound.and_then(|b| b.get(i));
            let map = standard::material_map(attributes, i);
            let at = instances.prototype_index(i);
            let prototype = match key.is_none() && !map.is_empty() {
                // A map: the prototype written in the materials it maps to.
                true => self.prototype(&instances.prototypes()[at], &map)?,
                false => prototypes[at],
            };
            let Some(prototype) = prototype else {
                continue;
            };
            let name = named.and_then(|n| n.get(i)).filter(|n| !n.is_empty());
            let name = names.take(&names::identifier(name.unwrap_or("instance")));
            copies.push(Node {
                transform: matrix(&instances.xforms()[i]),
                instance: Some(prototype),
                material: key.and_then(|key| self.materials.get(key).copied()),
                ..Node::group(&name, None)
            });
        }
        Ok(copies)
    }

    /// The prototype `value` is, rendered through `map` (pairs of a
    /// material its primitives name and the one they render with instead),
    /// written once; none for a value nothing here writes.
    fn prototype(&mut self, value: &Data, map: &[(&str, &str)]) -> Result<Option<usize>, Error> {
        let mapped: Vec<(String, String)> = (map.iter())
            .map(|&(from, to)| (from.to_owned(), to.to_owned()))
            .collect();
        if let Some(at) =
            (self.prototypes.iter()).position(|((held, by), _)| held.ptr_eq(value) && *by == mapped)
        {
            return Ok(Some(at));
        }
        let nodes = match self.kinds(value, map, "a prototype")? {
            Some(Kinds::Prims(prims)) => {
                let mut names = Unique::default();
                (prims.into_iter())
                    .map(|(suffix, kind, material)| Node {
                        kind,
                        material,
                        ..Node::group(
                            &names.take(if suffix.is_empty() {
                                "Geometry"
                            } else {
                                suffix
                            }),
                            None,
                        )
                    })
                    .collect()
            }
            Some(Kinds::One(kind)) => vec![Node {
                kind,
                ..Node::group("Prototype", None)
            }],
            Some(Kinds::Instances(copies)) => copies,
            None => return Ok(None),
        };
        let prototype = Prototype {
            root: Path::absolute_root(),
            nodes,
        };
        self.prototypes.push(((value.clone(), mapped), prototype));
        Ok(Some(self.prototypes.len() - 1))
    }
}
