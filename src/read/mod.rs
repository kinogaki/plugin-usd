//! A USD file read into the standard values: the stage opened (from the
//! file read once, a package's entries ranges of it), composed and read as
//! a scene, then mapped into the project's space as items.
//!
//! - **Materials** come first, as one item: every Material prim keyed by its
//!   path ([`materials`]), with the images domes read.
//! - **Prims** follow, parents first, each placed where the stage places it
//!   (`!resetXformStack!` kept): a group, or a mesh, shape, curves or points
//!   ([`geometry`]), a light or a camera ([`crate::lights`]).
//! - **Instances.** A prototype is held once as one value (its geometry
//!   gathered, the prototypes it shows nested as instances, at most
//!   [`MAX_DEPTH`] deep); an instanceable prim and a point instancer place
//!   it. An instance binding a material the prototype's prims leave to it
//!   names it on the Instance domain.
mod files;
mod geometry;
mod materials;

use crate::lights;
use crate::names::file_name;
use crate::space;
use crate::usd::package::Bytes;
use crate::usd::scene::{Instancer, Node, NodeKind, ReadOptions, Scene};
use crate::usd::sdf::Token;
use crate::usd::{ErrorKind, Stage, StageOptions};
use files::{Files, Held};
use geometry::Shaping;
use kinogaki_studio_api::edit::editor::{Placed, gather};
use kinogaki_studio_api::format::files::Named;
use kinogaki_studio_api::geometry::image::ImageSource;
use kinogaki_studio_api::geometry::{Attribute, Attributes, Column, Domain, Text, Xform, names};
use kinogaki_studio_api::image_files::OP;
use kinogaki_studio_api::standard::{self, ImportedImage, Instances, MAX_DEPTH, Materials};
use kinogaki_studio_api::{
    Data, Diagnostic, Error, FileItem, FormatOptions, Imported, Input, Reading,
};
use std::collections::HashMap;
use std::path::Path as FilePath;
use std::sync::Arc;

/// Reads `input` as `chosen` says.
pub(crate) fn read(
    input: &Input<'_>,
    chosen: &FormatOptions,
    reading: &Reading<'_>,
) -> Result<Imported, Error> {
    reading.check()?;
    let named = Arc::new(Named::new(input, chosen, reading));
    let held = Held::new(input.held()?);
    let stage_options = StageOptions {
        budget: reading.budget.clone(),
        disk: Some(named.clone()),
        ..StageOptions::default()
    };
    let bytes = Bytes::shared(held.buffer.clone());
    let opened = match input.path() {
        Some(path) => Stage::open_held(path, bytes, &stage_options),
        None => Stage::open_memory(input.name(), bytes, &stage_options),
    };
    let mut stage = opened.map_err(|e| failed(input, e))?;
    let scene = crate::usd::scene::read(&mut stage, &ReadOptions::default())
        .map_err(|e| failed(input, e))?;
    drop(stage);
    reading.check()?;
    let space = space::conversion(scene.meters_per_unit, scene.up_axis, reading.units)
        .map_err(|e| Error::invalid(format!("{input}: {}", e.message())))?;
    let keys: Vec<Arc<str>> = (scene.materials.iter())
        .map(|m| Arc::from(m.path.to_string()))
        .collect();
    let mut files = Files::new(&scene, &held, &named);
    let mut warnings = scene.warnings.clone();
    let mut looks = Materials::default();
    let single = single_sided(&scene);
    for (k, material) in scene.materials.iter().enumerate() {
        reading.check()?;
        let sides = (space.scale(), single[k]);
        let read = materials::material(material, &mut files, reading.colors, &mut warnings, sides);
        looks.materials.push(read);
    }
    let mut walk = Walk {
        scene: &scene,
        shaping: Shaping {
            space: &space,
            colors: reading.colors,
            keys: &keys,
        },
        reading,
        items: Vec::new(),
        prototypes: HashMap::new(),
        images: Vec::new(),
        files: &mut files,
        warnings: &mut warnings,
    };
    let mut at = Vec::with_capacity(scene.nodes.len());
    for node in &scene.nodes {
        reading.check()?;
        let placed = walk.node(node, &at)?;
        at.push(placed);
    }
    let Walk { items, images, .. } = walk;
    looks.images = images;
    let mut all = Vec::with_capacity(items.len() + 1);
    if !looks.materials.is_empty() || !looks.images.is_empty() {
        all.push(FileItem::top(
            "Materials",
            standard::materials().wrap(looks),
        ));
    }
    let shift = all.len();
    all.extend(items.into_iter().map(|mut item| {
        item.parent = item.parent.map(|p| p + shift);
        item
    }));
    let mut warnings = said(warnings);
    warnings.extend(named.warnings());
    Ok(Imported {
        items: all,
        warnings,
    })
}

/// Each material's sidedness: a mesh's `doubleSided` is its material's
/// Double Sided, so a material a single-sided mesh shows is single-sided.
fn single_sided(scene: &Scene) -> Vec<bool> {
    let mut single = vec![false; scene.materials.len()];
    let nodes = scene
        .nodes
        .iter()
        .chain(scene.prototypes.iter().flat_map(|p| &p.nodes));
    for node in nodes {
        let (sides, subsets) = match &node.kind {
            NodeKind::Mesh(mesh) => (mesh.double_sided, &mesh.subsets[..]),
            NodeKind::Shape(shape) => (shape.double_sided, &[][..]),
            _ => continue,
        };
        if sides {
            continue;
        }
        let bound = node
            .material
            .into_iter()
            .chain(subsets.iter().filter_map(|s| s.material));
        bound.for_each(|m| single[m] = true);
    }
    single
}

/// A failed read as ours, naming the input.
fn failed(input: &Input<'_>, e: crate::usd::Error) -> Error {
    match e.kind {
        ErrorKind::Cancelled => Error::cancelled(),
        _ => Error::invalid(format!("{input}: {e}")),
    }
}

/// `lines` as warnings, each once with how many times it was said.
fn said(lines: Vec<String>) -> Vec<Diagnostic> {
    let mut counted: Vec<(String, usize)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for line in lines {
        match at.get(&line) {
            Some(&k) => counted[k].1 += 1,
            None => {
                at.insert(line.clone(), counted.len());
                counted.push((line, 1));
            }
        }
    }
    (counted.into_iter())
        .map(|(line, n)| match n {
            1 => Diagnostic::warning(line),
            n => Diagnostic::warning(format!("{line} ({n} times)")),
        })
        .collect()
}

/// A node placed: its item, and where it is in the project.
#[derive(Clone, Copy)]
struct At {
    item: usize,
    world: Xform,
}

/// The walk over a scene's nodes into items.
struct Walk<'s, 'w> {
    scene: &'s Scene,
    shaping: Shaping<'w>,
    reading: &'w Reading<'w>,
    items: Vec<FileItem>,
    /// Each prototype as one value, made once; none when it shows nothing.
    prototypes: HashMap<usize, Option<Data>>,
    /// The images domes read, keyed by their file.
    images: Vec<ImportedImage>,
    files: &'w mut Files<'s>,
    warnings: &'w mut Vec<String>,
}

impl Walk<'_, '_> {
    /// `node` as an item under its parent's (`at`, each earlier node's).
    fn node(&mut self, node: &Node, at: &[At]) -> Result<At, Error> {
        let space = self.shaping.space;
        // A camera or a light keeps the frame it looks down.
        let local = match node.kind {
            NodeKind::Camera(_) | NodeKind::Light(_) => space.frame(&space::xform(&node.transform)),
            _ => space.placement(&space::xform(&node.transform)),
        };
        let above = node.parent.map(|p| at[p]);
        let world_above = above.map_or(Xform::IDENTITY, |a| a.world);
        let (placement, world) = match node.resets_transform {
            true => {
                let inverse = world_above.inverse().unwrap_or(Xform::IDENTITY);
                (inverse.compose(&local), local)
            }
            false => (local, world_above.compose(&local)),
        };
        let data = self.data(node, 0)?;
        let instance = match node.instance {
            Some(p) => self.instance(p, node.material, 0)?,
            None => None,
        };
        let (data, instance) = match (data, instance) {
            (None, instance) => (instance, None),
            both => both,
        };
        let item = self.items.len();
        self.items.push(FileItem {
            name: node.name.to_string(),
            parent: above.map(|a| a.item),
            placement,
            visible: node.visible,
            data,
        });
        if let Some(instance) = instance {
            self.items.push(FileItem {
                parent: Some(item),
                ..FileItem::top(node.name.to_string(), instance)
            });
        }
        Ok(At { item, world })
    }

    /// What `node` holds as a value; none for a group. `depth`: how deep
    /// in prototypes it is.
    fn data(&mut self, node: &Node, depth: usize) -> Result<Option<Data>, Error> {
        let cx = &self.shaping;
        Ok(Some(match &node.kind {
            NodeKind::Group => return Ok(None),
            NodeKind::Mesh(mesh) => {
                let polygons = geometry::mesh(mesh, node.material, cx, self.warnings)?;
                standard::geometry().wrap(polygons)
            }
            NodeKind::Shape(shape) => {
                let polygons = geometry::mesh(&shape.mesh(), node.material, cx, self.warnings)?;
                standard::geometry().wrap(polygons)
            }
            NodeKind::Curves(curves) => match geometry::curves(curves, cx, self.warnings) {
                Ok(curves) => standard::geometry().wrap(curves),
                Err(e) => return self.left_out(node, &e),
            },
            NodeKind::Points(points) => standard::geometry().wrap(geometry::points(points, cx)?),
            NodeKind::Camera(camera) => {
                let space = cx.space;
                let lens = lights::camera(camera, self.scene.meters_per_unit, space.scale());
                match standard::camera().wrap(lens) {
                    Ok(lens) => lens,
                    Err(e) => return self.left_out(node, &e),
                }
            }
            NodeKind::Light(light) => {
                let scale = self.shaping.space.scale();
                let image = light.texture.as_ref().and_then(|path| self.image(path));
                let light = lights::light(light, scale, image, self.reading.colors);
                match light.and_then(|l| standard::light().wrap(l)) {
                    Ok(light) => light,
                    Err(e) => return self.left_out(node, &e),
                }
            }
            NodeKind::Instancer(instancer) => {
                match self.instancer(instancer, node.material, depth)? {
                    Some(instances) => instances,
                    None => return Ok(None),
                }
            }
        }))
    }

    /// Says `node` is left out, and why.
    fn left_out(&mut self, node: &Node, why: &Error) -> Result<Option<Data>, Error> {
        let name = &node.name;
        self.warnings
            .push(format!("{name}: {}; left out", why.message()));
        Ok(None)
    }

    /// `op:` and the key of the image the file at `path` is, made once;
    /// none for a file left unresolved.
    fn image(&mut self, path: &Token) -> Option<String> {
        let source = ImageSource::new(self.files.file(path)?, Default::default());
        let mut key = path.to_string();
        if self.shaping.keys.iter().any(|k| **k == *key) {
            key.push_str(".image");
        }
        if !self.images.iter().any(|i| i.key == key) {
            let name = FilePath::new(file_name(path)).file_stem();
            let name = name.map_or("Image".into(), |s| s.to_string_lossy());
            self.images.push(ImportedImage {
                key: key.clone(),
                name: name.into_owned(),
                source,
            });
        }
        Some(format!("{OP}{key}"))
    }

    /// An instance of prototype `p` where its prim is, naming `material`
    /// where the prototype binds none.
    fn instance(
        &mut self,
        p: usize,
        material: Option<usize>,
        depth: usize,
    ) -> Result<Option<Data>, Error> {
        let Some(prototype) = self.prototype(p, depth + 1)? else {
            return Ok(None);
        };
        let attributes = self.bound(&[p], material, 1)?;
        let one = vec![Xform::IDENTITY].into();
        self.instances(vec![prototype], one, None, attributes)
    }

    /// `instancer`'s instances that show, each placed relative to it.
    fn instancer(
        &mut self,
        instancer: &Instancer,
        material: Option<usize>,
        depth: usize,
    ) -> Result<Option<Data>, Error> {
        let mut prototypes = Vec::new();
        let mut index_of = Vec::with_capacity(instancer.prototypes.len());
        for &p in &instancer.prototypes {
            let made = self.prototype(p, depth + 1)?;
            index_of.push(made.as_ref().map(|_| prototypes.len() as u32));
            prototypes.extend(made);
        }
        let shown = instancer.shown();
        let space = self.shaping.space;
        let (mut xforms, mut which) = (Vec::new(), Vec::new());
        for (i, m) in instancer.transforms().iter().enumerate() {
            let named = usize::try_from(instancer.proto_indices[i]).ok();
            let index = named.and_then(|k| index_of.get(k).copied().flatten());
            if let (true, Some(index)) = (shown[i], index) {
                xforms.push(space.placement(&space::xform(m)));
                which.push(index);
            }
        }
        if prototypes.is_empty() {
            return Ok(None);
        }
        let attributes = self.bound(&instancer.prototypes, material, xforms.len())?;
        let which = (prototypes.len() > 1).then(|| which.into());
        self.instances(prototypes, xforms.into(), which, attributes)
    }

    /// Instances whose nesting is too deep are left out, said.
    fn instances(
        &mut self,
        prototypes: Vec<Data>,
        xforms: Column<Xform>,
        which: Option<Column<u32>>,
        attributes: Attributes,
    ) -> Result<Option<Data>, Error> {
        match Instances::new(prototypes, xforms, which, attributes) {
            Ok(instances) => Ok(Some(standard::instances().wrap(instances))),
            Err(e) => {
                self.warnings
                    .push(format!("instances left out: {}", e.message()));
                Ok(None)
            }
        }
    }

    /// The Instance domain's `material` of `count` instances binding
    /// `material`, when none of `prototypes` binds its own.
    fn bound(
        &mut self,
        prototypes: &[usize],
        material: Option<usize>,
        count: usize,
    ) -> Result<Attributes, Error> {
        let Some(material) = material else {
            return Ok(Attributes::new());
        };
        let own = |node: &Node| {
            node.material.is_some()
                || matches!(&node.kind, NodeKind::Mesh(m) if m.subsets.iter().any(|s| s.material.is_some()))
        };
        let scene = self.scene;
        let binds = prototypes.iter().any(|&p| {
            scene
                .prototypes
                .get(p)
                .is_some_and(|p| p.nodes.iter().any(own))
        });
        if binds {
            self.warnings.push(
                "an instance's material reaches only the prims its prototype leaves unbound; \
                 the prototype's own bindings kept"
                    .into(),
            );
            return Ok(Attributes::new());
        }
        let key = &*self.shaping.keys[material];
        let text = Text::from_strs(std::iter::repeat_n(key, count));
        Attributes::new().with(Domain::Instance, names::MATERIAL, Attribute::text(text))
    }

    /// Prototype `p` as one value, made once: its geometry gathered where
    /// its prims place it, its instances nested; none when it shows
    /// nothing, or nests deeper than instances may.
    fn prototype(&mut self, p: usize, depth: usize) -> Result<Option<Data>, Error> {
        if let Some(made) = self.prototypes.get(&p) {
            return Ok(made.clone());
        }
        let made = self.make_prototype(p, depth)?;
        self.prototypes.insert(p, made.clone());
        Ok(made)
    }

    fn make_prototype(&mut self, p: usize, depth: usize) -> Result<Option<Data>, Error> {
        let scene = self.scene;
        let Some(prototype) = scene.prototypes.get(p) else {
            return Ok(None);
        };
        if depth > MAX_DEPTH {
            let root = &prototype.root;
            self.warnings.push(format!(
                "{root}: instances nest deeper than {MAX_DEPTH}; left out"
            ));
            return Ok(None);
        }
        let space = self.shaping.space;
        let mut worlds: Vec<Xform> = Vec::with_capacity(prototype.nodes.len());
        let mut geometry: Vec<(Data, Xform)> = Vec::new();
        let mut parts = Vec::new();
        for node in &prototype.nodes {
            self.reading.check()?;
            let local = space.placement(&space::xform(&node.transform));
            let world = match (node.parent, node.resets_transform) {
                (Some(parent), false) => worlds[parent].compose(&local),
                _ => local,
            };
            worlds.push(world);
            if matches!(node.kind, NodeKind::Light(_) | NodeKind::Camera(_)) {
                let name = &node.name;
                self.warnings.push(format!(
                    "{name}: a light or camera in a prototype; left out"
                ));
                continue;
            }
            // A point instancer's instances, placed where it is.
            match self.data(node, depth)? {
                Some(data) if standard::instances().get(&data).is_some() => {
                    let at = vec![world].into();
                    parts.extend(self.instances(vec![data], at, None, Attributes::new())?);
                }
                Some(data) => geometry.push((data, world)),
                None => {}
            }
            if let Some(q) = node.instance
                && let Some(nested) = self.prototype(q, depth + 1)?
            {
                let attributes = self.bound(&[q], node.material, 1)?;
                let at = vec![world].into();
                parts.extend(self.instances(vec![nested], at, None, attributes)?);
            }
        }
        let placed: Vec<Placed<'_>> = (geometry.iter())
            .filter_map(|(data, at)| {
                let value = standard::geometry().get(data)?;
                Some(Placed { value, at: *at })
            })
            .collect();
        if !placed.is_empty() {
            parts.insert(0, standard::geometry().wrap(gather(&placed)?));
        }
        Ok(match parts.len() {
            0 => None,
            1 if standard::geometry().get(&parts[0]).is_some() => parts.pop(),
            _ => match Instances::gathered(&parts) {
                Ok(instances) => Some(standard::instances().wrap(instances)),
                Err(e) => {
                    self.warnings
                        .push(format!("instances left out: {}", e.message()));
                    None
                }
            },
        })
    }
}
