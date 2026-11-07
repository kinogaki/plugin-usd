//! A scene authored as one layer, and written as a usda, usdc or usdz file
//! with the files it names.
//!
//! The layer holds the stage metadata (`defaultPrim` the first top prim,
//! `upAxis`, `metersPerUnit`), each node as its prim at the path its names
//! give, placed by its local transform (`!resetXformStack!` kept), hidden
//! with `visibility = invisible`, bound with `material:binding`; then the
//! materials. An instance holds a copy of its prototype's prims; a point
//! instancer holds its prototypes beneath it.
//!
//! A usdz carries under `textures/` every asset held, and every file on
//! disk that is a picture a usdz may pack ([`packable`]); a usda or usdc
//! names files on disk by their paths. A missing asset, an unpackable one
//! and, in a usda or usdc, a held one are written as they were authored:
//! writing files beside a layer is its format's, which names them as it
//! writes them.

mod geometry;

use std::collections::HashMap;
use std::path::Path as FilePath;
use std::sync::Arc;

use super::{Asset, AssetSource, Author, Matrix4d, Node, NodeKind, Scene, UpAxis, camera, light};
use crate::usd::error::{Error, Result};
use crate::usd::layer_file::{Format, write_layer};
use crate::usd::package::{Bytes, packable, write_usdz};
use crate::usd::sdf::identifier::is_identifier;
use crate::usd::sdf::{Array, LayerData, Path, Token, Value};
use crate::usd::shade;

/// How deep instances may hold instances when written as copies.
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
    let writer = Writer { scene, asset };
    writer.nodes(&mut author, &Path::absolute_root(), &scene.nodes, 0)?;
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
            std::fs::write(path, bytes).map_err(|e| located(e.into()))
        }
        Format::Usda | Format::Usdc => {
            let layer = author_with(scene, &|p| written(scene, &HashMap::new(), p));
            write_layer(&layer.map_err(located)?, path)
        }
    }
}

/// Each asset that has bytes (held, or a picture on disk) named
/// `<folder>/<its file name>`, names kept unique (`<stem>_1.<extension>`…,
/// counted per name): the names by asset path, and the files.
#[allow(clippy::type_complexity)]
fn packaged(
    assets: &[Asset],
    folder: &str,
) -> Result<(HashMap<Token, Token>, Vec<(String, Bytes)>)> {
    let mut names = HashMap::new();
    let mut files: Vec<(String, Bytes)> = Vec::new();
    let mut taken: HashMap<String, usize> = HashMap::new();
    for asset in assets {
        let bytes = match &asset.source {
            AssetSource::Bytes(bytes) => bytes.clone(),
            AssetSource::File(file) if packable(file) => Bytes::from(std::fs::read(file)?),
            AssetSource::File(_) | AssetSource::Missing => continue,
        };
        let file = file_name(&asset.path);
        let (stem, extension) = match file.rfind('.') {
            Some(dot) if dot > 0 => (&file[..dot], &file[dot..]),
            _ => (file, ""),
        };
        let mut name = format!("{folder}/{file}");
        let mut copy = taken.get(&name).copied().unwrap_or(0);
        while taken.contains_key(&name) {
            copy += 1;
            name = format!("{folder}/{stem}_{copy}{extension}");
        }
        taken.insert(format!("{folder}/{file}"), copy);
        taken.insert(name.clone(), 0);
        names.insert(asset.path.clone(), Token::new(&name));
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

/// Authoring a scene's nodes.
struct Writer<'a> {
    scene: &'a Scene,
    asset: &'a dyn Fn(&Token) -> Token,
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

    /// Author one node at `path`, with its instance's copy.
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
        placement(author, path, &node.transform, node.resets_transform);
        if !node.visible {
            let hidden = Value::Token(Token::new("invisible"));
            author.attribute(path, "visibility", "token", hidden, false);
        }
        self.bind(author, path, node.material)?;
        if let Some(index) = node.instance {
            let prototype = self.scene.prototypes.get(index).ok_or_else(|| {
                Error::invalid(format!("{path}: prototype {index} does not exist"))
            })?;
            self.nodes(author, path, &prototype.nodes, depth + 1)?;
        }
        Ok(())
    }

    /// Bind material `material` (an index into the scene's) to `prim`.
    fn bind(&self, author: &mut Author, prim: &Path, material: Option<usize>) -> Result<()> {
        let Some(index) = material else {
            return Ok(());
        };
        let material =
            self.scene.materials.get(index).ok_or_else(|| {
                Error::invalid(format!("{prim}: material {index} does not exist"))
            })?;
        author.api_schema(prim, "MaterialBindingAPI");
        author.relationship(prim, "material:binding", vec![material.path.clone()]);
        Ok(())
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
