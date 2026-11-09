//! Items written as one USD layer: a usda, a usdc, or a usdz carrying the
//! files its materials and domes name under `textures/` (a usda or usdc
//! names them as every written document does: [`assets`]).
//!
//! The stage is in the project's units (`metersPerUnit`, `upAxis`). Of a
//! document read from a layer (`Writing::metadata`) whose `defaultPrim`
//! names a top item, the top items are the root prims as they were, that
//! one the default prim, and the layer's metadata comes back
//! ([`crate::metadata`]); else the default prim is the one group the items
//! are under, or one named after the file holding them. Each materials item (a Material Library) is a Scope
//! at its place, named as the library, its Materials beneath by their
//! names; a group holding only libraries, unplaced, is a Scope too. Each
//! other item is its prim, placed relative to its parent's: polygons and curves
//! ([`geometry`]), what the host makes polygons of (a B-rep), lights and
//! cameras ([`crate::lights`]), and instances as USD's instancing
//! ([`instances`]). A value several items show (copies sharing one) is
//! written once, as a prototype each of their prims is an instance of.
//! Prims carry the model kinds of [`kinds`].
mod assets;
mod curves;
mod geometry;
mod instances;
mod kinds;
mod materials;
mod named;
mod preview;

use crate::lights;
use crate::names;
use crate::space::matrix;
use crate::usd::scene::{Instancer, Matrix4d, Node, NodeKind, Scene, UpAxis as StageUp};
use crate::usd::sdf::{Path, Token};
use assets::Assets;
use geometry::Shaping;
use instances::Shows;
use kinogaki_render_api::standard::LightShape;
use kinogaki_studio_api::format::Unique;
use kinogaki_studio_api::geometry::{UpAxis, Xform};
use kinogaki_studio_api::standard;
use kinogaki_studio_api::value::ImageRef;
use kinogaki_studio_api::{
    Data, DataTypeId, Diagnostic, Error, FileItem, FormatOptions, FormatWriter, Writing,
};
use std::collections::{HashMap, HashSet};
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
            stored: HashMap::new(),
            shared: shared(items),
            writing,
            warnings: Vec::new(),
        };
        let top = Top::of(items, path, writing.metadata);
        let root_item = match &top {
            Top::One { item, .. } => *item,
            Top::Prims(_) => None,
        };
        let root = match &top {
            Top::One { name, .. } => Path::absolute_root().child(Token::new(name)),
            Top::Prims(_) => Path::absolute_root(),
        };
        // Each item's prim name, unique beside its siblings (the root's
        // children for the top ones), libraries among them; the prims an
        // item's value makes beneath it take names after its children's.
        let mut children: HashMap<Option<usize>, Unique> = HashMap::new();
        let mut paths: Vec<Path> = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let parent = item.parent.filter(|&p| Some(p) != root_item);
            let path = match (Some(i) == root_item, parent) {
                (true, _) => root.clone(),
                (false, parent) => {
                    let above = parent.map_or(&root, |p| &paths[p]);
                    let name = names::identifier(&item.name);
                    above.child(Token::new(&children.entry(parent).or_default().take(&name)))
                }
            };
            paths.push(path);
        }
        for (item, path) in items.iter().zip(&paths) {
            out.materials_item(item, path)?;
        }
        // The root prim holding the others, when one does.
        let held = match &top {
            Top::One { item, name } => {
                out.scene.nodes.push(Node {
                    visible: item.is_none_or(|i| items[i].visible),
                    transform: item.map_or(Matrix4d::IDENTITY, |i| matrix(&items[i].placement)),
                    ..Node::group(name, None)
                });
                Some(0)
            }
            Top::Prims(default) => {
                out.scene.default_prim = Some(default.clone());
                None
            }
        };
        // Each item's node, under its parent's (the root's for the top ones).
        let mut parents = vec![false; items.len()];
        for parent in items.iter().filter_map(|i| i.parent) {
            parents[parent] = true;
        }
        let scopes = scopes(items);
        let named = named::Named::of(items, writing.colors, &mut out.warnings)?;
        let mut node_of: Vec<Option<usize>> = vec![None; items.len()];
        for (i, item) in items.iter().enumerate() {
            writing.check()?;
            if Some(i) == root_item {
                node_of[i] = Some(0);
                continue;
            }
            // A geometry light makes no prim; one holding others, a group.
            let group;
            let item = match named.unwritten(i) {
                true if !parents[i] => continue,
                true => {
                    group = FileItem {
                        data: None,
                        ..item.clone()
                    };
                    &group
                }
                false => item,
            };
            let parent = item.parent.and_then(|p| node_of[p]).or(held);
            let name = paths[i].name();
            node_of[i] = Some(match scopes[i] {
                true => {
                    let scope = Node {
                        type_name: Token::new("Scope"),
                        ..Node::group(name, parent)
                    };
                    out.scene.nodes.push(scope);
                    out.scene.nodes.len() - 1
                }
                false => {
                    let names = children.entry(Some(i)).or_default();
                    let at = out.item(item, (name, parent), (parents[i], names))?;
                    named.emit(i, item, &mut out.scene, at, &mut out.warnings);
                    at
                }
            });
        }
        named.list_portals(&mut out.scene, &node_of, &paths);
        if let Some(metadata) = writing.metadata {
            out.layer_metadata(metadata);
        }
        kinds::assign(&mut out.scene);
        let (assets, unpacked) = std::mem::replace(&mut out.assets, Assets::of(path)).assets();
        out.scene.assets = assets;
        out.warnings.extend(unpacked);
        writing.check()?;
        crate::usd::scene::write_file(&out.scene, path)
            .map_err(|e| Error::invalid(format!("{}: {e}", path.display())))?;
        Ok(out.warnings.into_iter().map(Diagnostic::warning).collect())
    }
}

/// Where the items' top prims go.
enum Top {
    /// Each top item is a root prim, the default prim the one of this name:
    /// a document read from a layer whose `defaultPrim` named it.
    Prims(Token),
    /// One root prim of this name holds every other prim: the one group at
    /// the top (`item`), else a prim made for them, named after the file.
    One { item: Option<usize>, name: String },
}

impl Top {
    /// Where `items`, written to `path`, go; `metadata` the layer's they
    /// were read from.
    fn of(items: &[FileItem], path: &FilePath, metadata: Option<&FormatOptions>) -> Top {
        let tops = || (0..items.len()).filter(|&i| items[i].parent.is_none());
        let default =
            (metadata.and_then(|m| m.get(crate::metadata::DEFAULT_PRIM))).and_then(|v| v.text());
        if let Some(default) = default
            && tops().any(|i| names::identifier(&items[i].name) == default)
        {
            return Top::Prims(Token::new(default));
        }
        let shown = tops().filter(|&i| !is_materials(&items[i]));
        match shown.collect::<Vec<_>>()[..] {
            [one] if items[one].data.is_none() => Top::One {
                item: Some(one),
                name: names::identifier(&items[one].name).into_owned(),
            },
            _ => {
                let stem = path
                    .file_stem()
                    .map_or("Scene".into(), |s| s.to_string_lossy());
                Top::One {
                    item: None,
                    name: names::identifier(&stem).into_owned(),
                }
            }
        }
    }
}

/// Whether `item` holds a library of materials.
fn is_materials(item: &FileItem) -> bool {
    (item.data.as_ref()).is_some_and(|d| standard::materials().get(d).is_some())
}

/// Which items are Scopes: libraries, and groups holding only libraries
/// and such groups, unplaced and shown (USD's grouping of what has no
/// transform; any other group is an Xform).
fn scopes(items: &[FileItem]) -> Vec<bool> {
    let mut holds = vec![false; items.len()];
    let mut scope: Vec<bool> = (items.iter())
        .map(|i| {
            is_materials(i) || (i.data.is_none() && i.visible && i.placement == Xform::IDENTITY)
        })
        .collect();
    for (i, item) in items.iter().enumerate().rev() {
        if let Some(p) = item.parent {
            holds[p] = true;
            scope[p] &= scope[i];
        }
        scope[i] &= holds[i] || is_materials(item);
    }
    scope
}

/// The values (by identity) several of `items` show that a prototype can
/// hold (not instances, lights, cameras or materials): each written once.
fn shared(items: &[FileItem]) -> HashSet<usize> {
    let mut seen = HashSet::new();
    let mut shared = HashSet::new();
    for data in items.iter().filter_map(|i| i.data.as_ref()) {
        let other = [
            standard::instances().id(),
            standard::light().id(),
            standard::camera().id(),
            standard::materials().id(),
        ];
        if !other.contains(&data.ty()) && !seen.insert(data.identity()) {
            shared.insert(data.identity());
        }
    }
    shared
}

/// What a value is written as.
enum Kinds {
    /// Prims of its own: the item's (an empty suffix), then children, each
    /// with its material.
    Prims(Vec<(&'static str, NodeKind, Option<usize>)>),
    /// One prim's kind.
    One(NodeKind),
    /// An instance of a prototype.
    Instance(Shows),
    /// Instances placed by a point instancer.
    Instancer(Instancer),
    /// Instances each a prim of its own.
    Instances(Vec<Shows>),
}

/// The scene being written.
struct Out<'w> {
    scene: Scene,
    assets: Assets,
    /// Each material's index in the scene, by its key.
    materials: HashMap<Arc<str>, usize>,
    /// Whether each of the scene's materials is double-sided.
    double_sided: Vec<bool>,
    /// Each prototype written (an index into the scene's; none for a
    /// value nothing here writes), by what it is written of.
    stored: HashMap<Of, Option<usize>>,
    /// The values several items show ([`shared`]).
    shared: HashSet<usize>,
    writing: &'w Writing<'w>,
    warnings: Vec<String>,
}

/// What a prototype is written of: a value (by its identity), and the
/// material map its instances render it with (pairs of a material its
/// primitives name and the one they render with; empty: its own
/// materials).
type Of = (usize, Vec<(String, String)>);

impl Out<'_> {
    /// The layer metadata `metadata` (the document's file's) holds that
    /// still applies: its fields as read, the render settings prim's path
    /// while it names a prim written; what doesn't, said.
    fn layer_metadata(&mut self, metadata: &FormatOptions) {
        self.scene.metadata = crate::metadata::fields(metadata, &mut self.warnings);
        let named = metadata.get(crate::metadata::RENDER_SETTINGS);
        let Some(path) = named.and_then(|v| v.text()) else {
            return;
        };
        let written = (0..self.scene.nodes.len()).any(|n| self.scene.path(n).to_string() == path);
        match written {
            true => self.scene.metadata.push((
                Token::new(crate::metadata::RENDER_SETTINGS),
                crate::usd::sdf::Value::String(Token::new(path)),
            )),
            false => self.warnings.push(format!(
                "{}: names {path}, which isn't written; left out",
                crate::metadata::RENDER_SETTINGS
            )),
        }
    }

    /// The materials `item` holds (a library), each beneath `library`, the
    /// library's prim, by its name.
    fn materials_item(&mut self, item: &FileItem, library: &Path) -> Result<(), Error> {
        let held = item
            .data
            .as_ref()
            .and_then(|d| standard::materials().get(d));
        let Some(held) = held else { return Ok(()) };
        let mut names = Unique::default();
        for material in &held.materials {
            let name = names.take(&names::identifier(&material.name));
            let path = library.child(Token::new(&name));
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

    /// `item` as the node `name` under node `parent`; its index. Its value
    /// is its prim's when it is one prim and the item holds no children
    /// (`parent_of`); else the item's prim is a group and the value's
    /// prims are beneath it, named by `names` beside the item's children:
    /// a gprim holds no prims (UsdGeom's encapsulation rule), and an
    /// instance's prim holds nothing of its own.
    fn item(
        &mut self,
        item: &FileItem,
        (name, parent): (&str, Option<usize>),
        (parent_of, names): (bool, &mut Unique),
    ) -> Result<usize, Error> {
        let mut node = Node {
            transform: matrix(&item.placement),
            visible: item.visible,
            ..Node::group(name, parent)
        };
        let at = self.scene.nodes.len();
        let mut children: Vec<Node> = Vec::new();
        let kinds = match &item.data {
            Some(data) if self.shared.contains(&data.identity()) => (self.prototype(data, &[])?)
                .map(|prototype| {
                    Kinds::Instance(Shows {
                        prototype,
                        xform: Xform::IDENTITY,
                        material: None,
                        stronger: false,
                        name: name.to_owned(),
                    })
                }),
            Some(data) => self.kinds(data, &[], &item.name)?,
            None => None,
        };
        match kinds {
            Some(Kinds::Prims(mut prims)) if prims.len() == 1 && !parent_of => {
                let (_, kind, material) = prims.remove(0);
                (node.kind, node.material) = (kind, material);
            }
            Some(Kinds::Prims(prims)) => {
                for (suffix, kind, material) in prims {
                    let child = names.take(if suffix.is_empty() {
                        "Geometry"
                    } else {
                        suffix
                    });
                    children.push(Node {
                        kind,
                        material,
                        ..Node::group(&child, Some(at))
                    });
                }
            }
            Some(Kinds::One(kind)) => node.kind = kind,
            Some(Kinds::Instance(shows)) if !parent_of => {
                let placement = item.placement.compose(&shows.xform);
                node = Node {
                    transform: matrix(&placement),
                    ..shows.node(name, parent)
                };
                node.visible = item.visible;
            }
            Some(Kinds::Instancer(instancer)) if !parent_of => {
                node.kind = NodeKind::Instancer(Box::new(instancer));
            }
            Some(Kinds::Instance(shows)) => {
                children.push(shows.node(&names.take("Instance"), Some(at)));
            }
            Some(Kinds::Instancer(instancer)) => children.push(Node {
                kind: NodeKind::Instancer(Box::new(instancer)),
                ..Node::group(&names.take("Instances"), Some(at))
            }),
            Some(Kinds::Instances(each)) => {
                for shows in each {
                    let name = names.take(&shows.name);
                    children.push(shows.node(&name, Some(at)));
                }
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
            return self.instances(instances, name);
        }
        if let Some(light) = standard::light().get(data) {
            let texture = match &light.shape {
                LightShape::Dome { image, .. } => image.as_ref(),
                LightShape::Rect { texture, .. } => texture.as_ref(),
                _ => None,
            };
            let ies = light.shaping.as_ref().and_then(|s| s.ies.as_ref());
            let files = lights::Files {
                texture: texture.and_then(|image| self.light_file(image, "image")),
                ies: ies.and_then(|profile| self.light_file(profile, "IES profile")),
            };
            let usd = lights::usd_light(light, 1.0, files, self.writing.colors)?;
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
            colors: self.writing.colors,
        };
        let value = rendered.as_ref().unwrap_or(value);
        let prims = geometry::prims(value, &shaping, &mut self.warnings);
        Ok(Some(Kinds::Prims(prims.prims)))
    }

    /// The asset a light's image or IES profile (`what`) names: its file,
    /// packed or written beside the layer when held embedded. A node's image
    /// is Export's to resolve before writing; one here is left out, said.
    fn light_file(&mut self, image: &ImageRef, what: &str) -> Option<Token> {
        let file = match image {
            ImageRef::File(source) => source.file(),
            ImageRef::Node(path) => {
                let said = format!(
                    "a light's {what} names the Image node {path}, which isn't written; left out"
                );
                self.warnings.push(said);
                return None;
            }
        };
        match self.assets.file(file) {
            Ok(asset) => Some(asset),
            Err(e) => {
                self.warnings
                    .push(format!("a light's {what}: {}; left out", e.message()));
                None
            }
        }
    }
}
