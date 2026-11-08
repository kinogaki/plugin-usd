//! A scene authored as one layer, and written as a usda, usdc or usdz file
//! with the files it names.
//!
//! The layer holds the stage metadata (`defaultPrim` the first top prim,
//! `upAxis`, `metersPerUnit`), each node as its prim at the path its names
//! give, of its model kind, placed by its local transform
//! (`!resetXformStack!` kept), hidden with `visibility = invisible`, bound
//! with `material:binding`
//! (`bindMaterialAs = "strongerThanDescendants"` where it is stronger);
//! then the prototypes instances show, then the materials.
//!
//! Instances are USD's own ([`Stored`]): each prototype an instance shows
//! is written once, beneath the default prim's `Prototypes`, a `class`, so
//! no traversal draws it in place (as `UsdGeomPointInstancer`'s schema
//! advises, schema.usda:2104-2112), and every instance is an instanceable
//! prim referencing it. A point instancer holds its prototypes beneath it,
//! in a `Prototypes` scope.
//!
//! A usdz carries under `textures/` every asset held, and every file on
//! disk that is a picture a usdz may pack ([`packable`]); a usda or usdc
//! names files on disk by their paths. A missing asset, an unpackable one
//! and, in a usda or usdc, a held one are written as they were authored:
//! writing files beside a layer is its format's, which names them as it
//! writes them.

mod geometry;

use kinogaki_studio_api::format::{Unique, files};
use std::collections::{HashMap, HashSet};
use std::path::Path as FilePath;
use std::sync::Arc;

use super::{Asset, AssetSource, Author, Matrix4d, Node, NodeKind, Scene, UpAxis, camera, light};
use crate::usd::error::{Error, Result};
use crate::usd::layer_file::{Format, write_layer};
use crate::usd::package::{Bytes, packable, write_usdz};
use crate::usd::sdf::identifier::is_identifier;
use crate::usd::sdf::{Array, LayerData, LayerOffset, ListOp, Path, Reference, Token, Value};
use crate::usd::shade;

/// How deep point instancers may hold point instancers among their
/// prototypes.
const MOST_NESTING: usize = 64;

/// The scene as a layer, its asset paths as they are.
///
/// # Errors
/// A node or subset name that is not an identifier, two siblings of one
/// name, a material or prototype index that does not exist, or a primvar
/// that holds no attribute value.
pub fn author(scene: &Scene) -> Result<LayerData> {
    let names = HashMap::new();
    author_with(scene, &|path| written(scene, &names, path))
}

/// The path written for the asset at `path`: its new name when it was
/// packaged, as authored when it is missing or held, else as it is.
fn written(scene: &Scene, names: &HashMap<Token, Token>, path: &Token) -> Token {
    if let Some(name) = names.get(path) {
        return name.clone();
    }
    match scene.asset(path) {
        Some(asset) if !matches!(asset.source, AssetSource::File(_)) => asset.authored.clone(),
        _ => path.clone(),
    }
}

/// The scene as a layer, each asset path written as `asset` gives it.
fn author_with(scene: &Scene, asset: &dyn Fn(&Token) -> Token) -> Result<LayerData> {
    let mut author = Author::new();
    if let Some(top) = scene.nodes.iter().find(|node| node.parent.is_none()) {
        author.layer_metadata("defaultPrim", Value::Token(top.name.clone()));
    }
    author.layer_metadata("metersPerUnit", Value::Double(scene.meters_per_unit));
    let up = match scene.up_axis {
        UpAxis::Y => "Y",
        UpAxis::Z => "Z",
    };
    author.layer_metadata("upAxis", Value::Token(Token::new(up)));
    let stored = Stored::of(scene)?;
    let writer = Writer {
        scene,
        asset,
        stored: &stored,
    };
    writer.nodes(&mut author, &Path::absolute_root(), &scene.nodes, 0)?;
    for (index, path) in &stored.prims {
        author.class(&stored.holder);
        author.prim(path, "Xform");
        let prototype = &scene.prototypes[*index];
        model(&mut author, path, &prototype.model);
        writer.nodes(&mut author, path, &prototype.nodes, 0)?;
    }
    for material in &scene.materials {
        shade::write(&mut author, material, asset)?;
    }
    Ok(author.finish())
}

/// Write `scene` to `path` in the format its extension names, with the
/// files it names (see the module).
pub fn write_file(scene: &Scene, path: impl AsRef<FilePath>) -> Result<()> {
    let path = path.as_ref();
    let file = path.display().to_string();
    let located = |error: Error| error.in_file(file.clone());
    let format = Format::for_path(path).ok_or_else(|| {
        located(Error::invalid(
            "a USD file must end in .usda, .usdc, .usd or .usdz",
        ))
    })?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("scene");
    match format {
        Format::Usdz => {
            let (names, files) = packaged(&scene.assets, "textures").map_err(located)?;
            let layer = author_with(scene, &|p| written(scene, &names, p)).map_err(located)?;
            let entries: Vec<(&str, &[u8])> = files
                .iter()
                .map(|(name, bytes)| (name.as_str(), bytes.as_ref()))
                .collect();
            let bytes = write_usdz(&layer, &format!("{stem}.usdc"), &entries).map_err(located)?;
            let all = |file: &mut std::fs::File| std::io::Write::write_all(file, &bytes);
            files::replace(path, all).map_err(|e| located(e.into()))
        }
        Format::Usda | Format::Usdc => {
            let layer = author_with(scene, &|p| written(scene, &HashMap::new(), p));
            write_layer(&layer.map_err(located)?, path)
        }
    }
}

/// Each asset that has bytes (held, or a picture on disk) named
/// `<folder>/<its file name>`, names kept unique (`<stem>_1.<extension>`…,
/// counted per name, case ignored): the paths written for them by asset
/// path (`./<folder>/…`), and the files.
#[allow(clippy::type_complexity)]
fn packaged(
    assets: &[Asset],
    folder: &str,
) -> Result<(HashMap<Token, Token>, Vec<(String, Bytes)>)> {
    let mut names = HashMap::new();
    let mut files: Vec<(String, Bytes)> = Vec::new();
    let mut taken = Unique::files();
    for asset in assets {
        let bytes = match &asset.source {
            AssetSource::Bytes(bytes) => bytes.clone(),
            AssetSource::File(file) if packable(file) => Bytes::from(std::fs::read(file)?),
            AssetSource::File(_) | AssetSource::Missing => continue,
        };
        let name = taken.take(&format!("{folder}/{}", file_name(&asset.path)));
        // File-relative, as the usdz specification encourages.
        names.insert(asset.path.clone(), Token::new(&format!("./{name}")));
        files.push((name, bytes));
    }
    Ok((names, files))
}

/// The file name an asset path ends in (inside a package path too).
fn file_name(path: &str) -> &str {
    let path = path.trim_end_matches(']');
    let start = path.rfind(['/', '[', '\\']).map_or(0, |at| at + 1);
    match &path[start..] {
        "" => "asset",
        name => name,
    }
}

/// Where the prototypes instances show are written: each once, beneath
/// `holder` (the default prim's child `Prototypes`, else its first free
/// name), named after the first instance showing it, in the order the
/// scene's nodes, then the prototypes' own, meet them.
struct Stored {
    holder: Path,
    /// Each prototype and its prim, in the order met.
    prims: Vec<(usize, Path)>,
    /// Each prototype's place in `prims`.
    at: HashMap<usize, usize>,
}

impl Stored {
    /// Where `scene`'s prototypes are written.
    ///
    /// # Errors
    /// An instance or point instancer naming a prototype that does not
    /// exist.
    fn of(scene: &Scene) -> Result<Stored> {
        let top = scene.nodes.iter().position(|node| node.parent.is_none());
        let default = top.map_or_else(Path::absolute_root, |t| {
            Path::absolute_root().child(scene.nodes[t].name.clone())
        });
        // The names beneath the default prim: its children's, and the
        // materials' scopes.
        let mut taken = Unique::names();
        let children = scene
            .nodes
            .iter()
            .filter(|n| n.parent.is_some() && n.parent == top);
        for node in children {
            taken.take(&node.name);
        }
        for material in &scene.materials {
            if let Some(child) = material.path.ancestors().find(|a| a.parent() == default) {
                taken.take(child.name());
            }
        }
        let mut stored = Stored {
            holder: default.child(taken.take("Prototypes")),
            prims: Vec::new(),
            at: HashMap::new(),
        };
        let mut names = Unique::names();
        let mut walked = HashSet::new();
        let mut queue: Vec<&[Node]> = vec![&scene.nodes];
        let mut next = 0;
        while let Some(&nodes) = queue.get(next) {
            next += 1;
            for node in nodes {
                let mut shown = Vec::new();
                if let Some(index) = node.instance {
                    if !stored.at.contains_key(&index) {
                        let path = stored.holder.child(names.take(&node.name));
                        stored.at.insert(index, stored.prims.len());
                        stored.prims.push((index, path));
                    }
                    shown.push(index);
                }
                if let NodeKind::Instancer(instancer) = &node.kind {
                    shown.extend(&instancer.prototypes);
                }
                for index in shown {
                    let prototype = scene.prototypes.get(index).ok_or_else(|| {
                        Error::invalid(format!("{}: prototype {index} does not exist", node.name))
                    })?;
                    if walked.insert(index) {
                        queue.push(&prototype.nodes);
                    }
                }
            }
        }
        Ok(stored)
    }
}

/// Authoring a scene's nodes.
struct Writer<'a> {
    scene: &'a Scene,
    asset: &'a dyn Fn(&Token) -> Token,
    stored: &'a Stored,
}

impl Writer<'_> {
    /// Author `nodes` (parents first) with their top ones beneath `root`.
    fn nodes(&self, author: &mut Author, root: &Path, nodes: &[Node], depth: usize) -> Result<()> {
        if depth > MOST_NESTING {
            return Err(Error::limit("instances nest more than 64 deep"));
        }
        let mut paths: Vec<Path> = Vec::with_capacity(nodes.len());
        for (index, node) in nodes.iter().enumerate() {
            let parent = match node.parent {
                None => root,
                Some(parent) if parent < index => &paths[parent],
                Some(parent) => {
                    return Err(Error::invalid(format!(
                        "node {index} names a parent {parent} that does not come before it"
                    )));
                }
            };
            let path = self.child(parent, &node.name)?;
            if author.has(&path) {
                return Err(Error::invalid(format!("{path} is written twice")));
            }
            self.node(author, &path, node, depth)?;
            paths.push(path);
        }
        Ok(())
    }

    /// `parent`'s child `name`, when `name` is an identifier.
    fn child(&self, parent: &Path, name: &Token) -> Result<Path> {
        if !is_identifier(name) {
            return Err(Error::invalid(format!(
                "{name:?} under {parent} is not a prim name"
            )));
        }
        Ok(parent.child(name.clone()))
    }

    /// Author one node at `path`, an instance referencing its prototype.
    fn node(&self, author: &mut Author, path: &Path, node: &Node, depth: usize) -> Result<()> {
        match &node.kind {
            NodeKind::Group => {
                author.prim(
                    path,
                    if node.type_name.is_empty() {
                        "Xform"
                    } else {
                        &node.type_name
                    },
                );
            }
            NodeKind::Mesh(mesh) => self.mesh(author, path, mesh)?,
            NodeKind::Shape(shape) => self.shape(author, path, shape)?,
            NodeKind::Curves(curves) => self.curves(author, path, curves)?,
            NodeKind::Points(points) => self.points(author, path, points)?,
            NodeKind::Camera(camera) => camera::write(author, path, camera),
            NodeKind::Light(light) => {
                let mut light = (**light).clone();
                light.texture = light.texture.map(|file| (self.asset)(&file));
                light::write(author, path, &light);
            }
            NodeKind::Instancer(instancer) => self.instancer(author, path, instancer, depth)?,
        }
        model(author, path, &node.model);
        placement(author, path, &node.transform, node.resets_transform);
        if !node.visible {
            let hidden = Value::Token(Token::new("invisible"));
            author.attribute(path, "visibility", "token", hidden, false);
        }
        self.bind(author, path, node.material, node.stronger)?;
        if let Some(index) = node.instance {
            let at = self.stored.at[&index];
            let reference = Reference {
                asset: Token::empty(),
                prim: self.stored.prims[at].1.clone(),
                offset: LayerOffset::IDENTITY,
                custom_data: Default::default(),
            };
            let references = ListOp {
                prepended: vec![reference],
                ..ListOp::default()
            };
            author.prim_metadata(path, "instanceable", Value::Bool(true));
            author.prim_metadata(
                path,
                "references",
                Value::ReferenceListOp(Arc::new(references)),
            );
        }
        Ok(())
    }

    /// Bind material `material` (an index into the scene's) to `prim`,
    /// `stronger` than its descendants' bindings when so.
    fn bind(
        &self,
        author: &mut Author,
        prim: &Path,
        material: Option<usize>,
        stronger: bool,
    ) -> Result<()> {
        let Some(index) = material else {
            return Ok(());
        };
        let material =
            self.scene.materials.get(index).ok_or_else(|| {
                Error::invalid(format!("{prim}: material {index} does not exist"))
            })?;
        author.api_schema(prim, "MaterialBindingAPI");
        author.relationship(prim, "material:binding", vec![material.path.clone()]);
        if stronger {
            let strength = Value::Token(Token::new("strongerThanDescendants"));
            author.property_metadata(
                &prim.property("material:binding"),
                "bindMaterialAs",
                strength,
            );
        }
        Ok(())
    }
}

/// A prim's model kind, when it has one.
fn model(author: &mut Author, path: &Path, kind: &Token) {
    if !kind.is_empty() {
        author.prim_metadata(path, "kind", Value::Token(kind.clone()));
    }
}

/// A prim's placement as one `xformOp:transform` (nothing for identity),
/// after `!resetXformStack!` when it resets its parents' transforms.
fn placement(author: &mut Author, path: &Path, local: &Matrix4d, resets: bool) {
    let mut order = Vec::new();
    if resets {
        order.push(Token::new("!resetXformStack!"));
    }
    if !local.is_identity() {
        author.attribute(
            path,
            "xformOp:transform",
            "matrix4d",
            Value::Matrix4d(local.0),
            false,
        );
        order.push(Token::new("xformOp:transform"));
    }
    if !order.is_empty() {
        let order = Value::Array(Array::Token(Arc::new(order)));
        author.attribute(path, "xformOpOrder", "token[]", order, true);
    }
}
