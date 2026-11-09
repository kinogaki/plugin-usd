//! A USD file read into the standard values: the stage opened (from the
//! file read once, a package's entries ranges of it), composed and read as
//! a scene, then mapped into the project's space as items.
//!
//! - **Materials**: every Material prim keyed by its path ([`materials`]),
//!   in a Material Library at the place of the prim holding it
//!   ([`libraries`]): a Scope of Materials alone is a library, its item where
//!   the Scope is.
//! - **Prims**, parents first, each placed where the stage places it
//!   (`!resetXformStack!` kept): a group, or a mesh, shape, curves or points
//!   ([`geometry`]), a light or a camera ([`crate::lights`]).
//! - **Instances.** A prototype is held once as one value (its geometry
//!   gathered, the prototypes it shows nested as instances, at most
//!   [`MAX_DEPTH`] deep); an instanceable prim and a point instancer place
//!   it. A point instancer's prototype that is an instance of another
//!   (placed where the instancer puts it, holding nothing else) is that
//!   other, as Export writes them. An instance binding a material the
//!   prototype's prims leave to it, or one stronger than theirs, names it
//!   on the Instance domain; a point instancer's `ids` and `velocities` are
//!   its instances' `id` and `v`, and each primvar of one value an instance
//!   an attribute of theirs (`displayColor` their `Cd`).
mod files;
mod geometry;
mod libraries;
mod materials;
mod named;
mod primvars;

use crate::lights;
use crate::space;
use crate::usd::package::Bytes;
use crate::usd::scene::{
    Instancer, Interpolation, Matrix4d, Node, NodeKind, Primvar, ReadOptions, Scene,
};
use crate::usd::sdf::{Path, Token};
use crate::usd::{ErrorKind, Stage, StageOptions};
use files::{Files, Held};
use geometry::Shaping;
use kinogaki_render_api::standard::LightShape;
use kinogaki_studio_api::edit::editor::{Placed, gather};
use kinogaki_studio_api::format::files::Named;
use kinogaki_studio_api::geometry::image::{ImageColor, ImageSource};
use kinogaki_studio_api::geometry::{
    Attribute, Attributes, Column, Domain, Meaning, Text, Vec3, Xform, names,
};
use kinogaki_studio_api::standard::{self, Instances, MAX_DEPTH, Materials};
use kinogaki_studio_api::value::ImageRef;
use kinogaki_studio_api::{
    Data, Diagnostic, Error, FileItem, FormatOptions, Imported, Input, ObjectRef, Purpose, Reading,
};
use libraries::Place;
use primvars::Spread;
use std::collections::HashMap;
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
    let mut looks = Vec::with_capacity(scene.materials.len());
    let single = single_sided(&scene);
    for (k, material) in scene.materials.iter().enumerate() {
        reading.check()?;
        let sides = (space.scale(), single[k]);
        let read = materials::material(material, &mut files, reading.colors, &mut warnings, sides);
        looks.push(read);
    }
    if reading.purpose == Purpose::Value && reading.wants == Some(standard::materials().id()) {
        let all = Materials { materials: looks };
        let item = FileItem::top(input.name(), standard::materials().wrap(all));
        let warnings = said(warnings);
        return Ok(Imported {
            items: vec![item],
            warnings,
        });
    }
    let mut libraries = HashMap::new();
    for (place, held) in libraries::libraries(&scene, &mut warnings) {
        let materials = held.into_iter().map(|m| looks[m].clone()).collect();
        libraries.insert(place, standard::materials().wrap(Materials { materials }));
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
        libraries,
        prototypes: HashMap::new(),
        files: &mut files,
        warnings: &mut warnings,
    };
    walk.library(Place::Under(None), None);
    let mut at = Vec::with_capacity(scene.nodes.len());
    for node in &scene.nodes {
        reading.check()?;
        let placed = walk.node(node, &at)?;
        at.push(placed);
    }
    let Walk { mut items, .. } = walk;
    named::resolve(&scene, &at, &mut items, &mut warnings);
    let mut warnings = said(warnings);
    warnings.extend(named.warnings());
    Ok(Imported { items, warnings })
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

/// A material an instance binds (an index into the scene's), and whether
/// it binds the prototype's prims whatever they bind.
type Bound = Option<(usize, bool)>;

/// What `node` binds, as an instance binds it.
fn bound(node: &Node) -> Bound {
    node.material.map(|m| (m, node.stronger))
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
    /// Each library's materials by its place, until its item is made.
    libraries: HashMap<Place, Data>,
    /// Each prototype as one value, made once; none when it shows nothing.
    prototypes: HashMap<usize, Option<Data>>,
    files: &'w mut Files<'s>,
    warnings: &'w mut Vec<String>,
}

impl Walk<'_, '_> {
    /// The library at `place` as an item under item `parent`, named
    /// [`libraries::MATERIALS`], when there is one.
    fn library(&mut self, place: Place, parent: Option<usize>) {
        if let Some(materials) = self.libraries.remove(&place) {
            self.items.push(FileItem {
                parent,
                ..FileItem::top(libraries::MATERIALS, materials)
            });
        }
    }

    /// `node` as an item under its parent's (`at`, each earlier node's): a
    /// Scope of Materials its library, with the library of Materials beside
    /// its children first.
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
        let k = at.len();
        let data = match self.libraries.remove(&Place::Node(k)) {
            Some(library) => Some(library),
            None => self.data(node, &self.scene.path(k), 0)?,
        };
        let instance = match node.instance {
            Some(p) => self.instance(p, bound(node), 0)?,
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
        if let NodeKind::Mesh(mesh) = &node.kind
            && let Some(light) = &mesh.light
            && self.items[item].data.is_some()
        {
            let path = self.scene.path(k);
            self.mesh_light(light, node, item, &path)?;
        }
        self.library(Place::Under(Some(k)), Some(item));
        Ok(At { item, world })
    }

    /// The geometry light the mesh `node` (its item `mesh`, the prim at
    /// `path`) is, as an item beside it named after it, emitting from it.
    fn mesh_light(
        &mut self,
        light: &crate::usd::scene::Light,
        node: &Node,
        mesh: usize,
        path: &Path,
    ) -> Result<(), Error> {
        if let Some(lost) = lights::left_out(light) {
            self.warnings.push(format!("{path}: {lost}"));
        }
        let scale = self.shaping.space.scale();
        let made = lights::light(light, scale, lights::Files::default(), self.reading.colors);
        let made = made.map(|mut light| {
            if let LightShape::Geometry { geometry, .. } = &mut light.shape {
                *geometry = Some(ObjectRef(mesh as u64));
            }
            light
        });
        match made.and_then(|l| standard::light().wrap(l)) {
            Ok(data) => self.items.push(FileItem {
                name: format!("{} Light", node.name),
                parent: self.items[mesh].parent,
                placement: Xform::IDENTITY,
                visible: node.visible,
                data: Some(data),
            }),
            Err(e) => self
                .warnings
                .push(format!("{path}: its light {}; left out", e.message())),
        }
        Ok(())
    }

    /// What `node`, the prim at `path`, holds as a value; none for a
    /// group. `depth`: how deep in prototypes it is.
    fn data(&mut self, node: &Node, path: &Path, depth: usize) -> Result<Option<Data>, Error> {
        let cx = &self.shaping;
        Ok(Some(match &node.kind {
            NodeKind::Group => return Ok(None),
            NodeKind::Mesh(mesh) => {
                let polygons = geometry::mesh(mesh, node.material, cx, path, self.warnings)?;
                standard::geometry().wrap(polygons)
            }
            NodeKind::Shape(shape) => {
                let mesh = shape.mesh();
                let polygons = geometry::mesh(&mesh, node.material, cx, path, self.warnings)?;
                standard::geometry().wrap(polygons)
            }
            NodeKind::Curves(curves) => {
                match geometry::curves(curves, node.material, cx, path, self.warnings) {
                    Ok(curves) => standard::geometry().wrap(curves),
                    Err(e) => return self.left_out(path, &e),
                }
            }
            NodeKind::Points(points) => {
                standard::geometry().wrap(geometry::points(points, cx, path, self.warnings)?)
            }
            NodeKind::Camera(camera) => {
                let space = cx.space;
                let lens = lights::camera(camera, self.scene.meters_per_unit, space.scale());
                match standard::camera().wrap(lens) {
                    Ok(lens) => lens,
                    Err(e) => return self.left_out(path, &e),
                }
            }
            NodeKind::Light(light) => {
                let scale = self.shaping.space.scale();
                let mut image = |file: Option<&Token>, color: ImageColor| {
                    let file = self.files.file(file?)?;
                    Some(ImageRef::File(ImageSource::new(file, color)))
                };
                let ies = light.shaping.as_ref().and_then(|s| s.ies.as_ref());
                let files = lights::Files {
                    texture: image(light.texture.as_ref(), ImageColor::default()),
                    ies: image(ies, ImageColor::Data),
                };
                if let Some(lost) = lights::left_out(light) {
                    self.warnings.push(format!("{path}: {lost}"));
                }
                let light = lights::light(light, scale, files, self.reading.colors);
                match light.and_then(|l| standard::light().wrap(l)) {
                    Ok(light) => light,
                    Err(e) => return self.left_out(path, &e),
                }
            }
            NodeKind::Instancer(instancer) => {
                match self.instancer((instancer, path), bound(node), depth)? {
                    Some(instances) => instances,
                    None => return Ok(None),
                }
            }
        }))
    }

    /// Says the prim at `path` is left out, and why.
    fn left_out(&mut self, path: &Path, why: &Error) -> Result<Option<Data>, Error> {
        self.warnings
            .push(format!("{path}: {}; left out", why.message()));
        Ok(None)
    }

    /// An instance of prototype `p` where its prim is, binding `material`.
    fn instance(&mut self, p: usize, material: Bound, depth: usize) -> Result<Option<Data>, Error> {
        let Some(prototype) = self.prototype(p, depth + 1)? else {
            return Ok(None);
        };
        let attributes = self.bound(&[(p, material)])?;
        let one = vec![Xform::IDENTITY].into();
        self.instances(vec![prototype], one, None, attributes)
    }

    /// `instancer`'s instances that show, each placed relative to it,
    /// binding what its prototype's prim binds, else `material`.
    fn instancer(
        &mut self,
        (instancer, path): (&Instancer, &Path),
        material: Bound,
        depth: usize,
    ) -> Result<Option<Data>, Error> {
        let mut prototypes = Vec::new();
        // The scene's prototype each value is made of.
        let mut made_of: Vec<usize> = Vec::new();
        // Each target's value and binding.
        let mut targets: Vec<Option<(usize, usize, Bound)>> = Vec::new();
        for &p in &instancer.prototypes {
            let (shows, binds) = self.shown_by(p, material);
            let index = match made_of.iter().position(|&q| q == shows) {
                Some(k) => Some(k),
                None => self.prototype(shows, depth + 1)?.map(|made| {
                    prototypes.push(made);
                    made_of.push(shows);
                    prototypes.len() - 1
                }),
            };
            targets.push(index.map(|k| (k, shows, binds)));
        }
        let shown = instancer.shown();
        let space = self.shaping.space;
        let (mut xforms, mut which, mut kept, mut bindings) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (i, m) in instancer.transforms().iter().enumerate() {
            let named = usize::try_from(instancer.proto_indices[i]).ok();
            let target = named.and_then(|k| targets.get(k).copied().flatten());
            if let (true, Some((index, shows, binds))) = (shown[i], target) {
                xforms.push(space.placement(&space::xform(m)));
                which.push(index as u32);
                kept.push(i);
                bindings.push((shows, binds));
            }
        }
        if prototypes.is_empty() {
            return Ok(None);
        }
        let attributes = self.bound(&bindings)?;
        let attributes = self.per_instance(attributes, (instancer, path), &kept)?;
        let which = (prototypes.len() > 1).then(|| which.into());
        self.instances(prototypes, xforms.into(), which, attributes)
    }

    /// What the point instancer's prototype `p` shows, and what its
    /// instances bind: the prototype of the instance it is when it is one
    /// alone (placed where the instancer puts it, shown), binding its own
    /// material, else `material`; else itself, binding `material`.
    fn shown_by(&self, p: usize, material: Bound) -> (usize, Bound) {
        let nodes = self.scene.prototypes.get(p).map_or(&[][..], |p| &p.nodes);
        match nodes {
            [node]
                if matches!(node.kind, NodeKind::Group)
                    && node.transform == Matrix4d::IDENTITY
                    && !node.resets_transform
                    && node.visible =>
            {
                match node.instance {
                    Some(q) => (q, bound(node).or(material)),
                    None => (p, material),
                }
            }
            _ => (p, material),
        }
    }

    /// `attributes` with the point instancer's `ids` as the instances' `id`,
    /// its `velocities` their `v`, and every primvar of one value an
    /// instance (`vertex` or `varying`) an attribute of theirs as a mesh's
    /// primvar is (`displayColor` their `Cd`), of the instances `kept`;
    /// a primvar of another interpolation is left out, said.
    fn per_instance(
        &mut self,
        attributes: Attributes,
        (instancer, path): (&Instancer, &Path),
        kept: &[usize],
    ) -> Result<Attributes, Error> {
        let mut attributes = attributes;
        if let Some(ids) = &instancer.ids {
            let ids: Vec<i64> = kept.iter().map(|&i| ids[i]).collect();
            attributes = attributes.with(
                Domain::Instance,
                names::ID,
                Attribute::tuples(ids, Meaning::None)?,
            )?;
        }
        let picked: Vec<u32> = kept.iter().map(|&i| i as u32).collect();
        let spread = Spread {
            primitives: Some(&picked),
            corners: None,
            points: 0,
            faces: instancer.len(),
            vertices: 0,
        };
        for primvar in &instancer.primvars {
            attributes = match (primvar.name.as_str(), primvar.interpolation) {
                ("velocities", _) => self.velocities(attributes, primvar, kept)?,
                (_, Interpolation::Vertex | Interpolation::Varying) => {
                    let on = (primvar, Domain::Instance);
                    let cx = &self.shaping;
                    primvars::attribute_on(attributes, on, &spread, cx, path, self.warnings)?
                }
                (name, other) => {
                    let other = other.token();
                    self.warnings.push(format!(
                        "{path}: primvar {name} is {other}, not one value an instance; left out"
                    ));
                    attributes
                }
            };
        }
        Ok(attributes)
    }

    /// `attributes` with a point instancer's `velocities` as the instances'
    /// `v`, of the instances `kept`, in the project's space.
    fn velocities(
        &mut self,
        attributes: Attributes,
        primvar: &Primvar,
        kept: &[usize],
    ) -> Result<Attributes, Error> {
        let Some(crate::usd::sdf::Array::Vec3f(values)) = primvar.values.as_array() else {
            return Ok(attributes);
        };
        let space = self.shaping.space;
        let scale = space.scale();
        let moved: Option<Vec<[f32; 3]>> = (kept.iter())
            .map(|&i| {
                let v = values.get(i)?.map(f64::from);
                let d = space.direction(Vec3::new(v[0], v[1], v[2])) * scale;
                Some([d.x, d.y, d.z].map(|c| c as f32))
            })
            .collect();
        let Some(moved) = moved else {
            self.warnings
                .push("a point instancer's velocities aren't one an instance; left out".into());
            return Ok(attributes);
        };
        attributes.with(
            Domain::Instance,
            names::V,
            Attribute::tuples(moved, Meaning::Vector)?,
        )
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

    /// The Instance domain's `material`: what each instance binds
    /// (`bindings`: the prototype it shows, and its binding) where the
    /// prototype's prims bind nothing of their own or the binding is
    /// stronger than theirs; none when no instance binds one.
    fn bound(&mut self, bindings: &[(usize, Bound)]) -> Result<Attributes, Error> {
        let keys = self.shaping.keys;
        let mut weaker = false;
        let named: Vec<&str> = (bindings.iter())
            .map(|&(p, bound)| match bound {
                Some((m, stronger)) if stronger || !self.binds(p) => &*keys[m],
                Some(_) => {
                    weaker = true;
                    ""
                }
                None => "",
            })
            .collect();
        if weaker {
            self.warnings.push(
                "an instance's material reaches only the prims its prototype leaves unbound; \
                 the prototype's own bindings kept"
                    .into(),
            );
        }
        if named.iter().all(|k| k.is_empty()) {
            return Ok(Attributes::new());
        }
        let text = Attribute::text(Text::from_strs(named));
        Attributes::new().with(Domain::Instance, names::MATERIAL, text)
    }

    /// Whether prototype `p`'s prims bind materials of their own.
    fn binds(&self, p: usize) -> bool {
        let own = |node: &Node| {
            node.material.is_some()
                || matches!(&node.kind, NodeKind::Mesh(m) if m.subsets.iter().any(|s| s.material.is_some()))
        };
        (self.scene.prototypes.get(p)).is_some_and(|p| p.nodes.iter().any(own))
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
        // An invisible prim hides its descendants: nothing of it shows.
        let mut hidden: Vec<bool> = Vec::with_capacity(prototype.nodes.len());
        let mut geometry: Vec<(Data, Xform)> = Vec::new();
        let mut parts = Vec::new();
        for (k, node) in prototype.nodes.iter().enumerate() {
            self.reading.check()?;
            let local = space.placement(&space::xform(&node.transform));
            let world = match (node.parent, node.resets_transform) {
                (Some(parent), false) => worlds[parent].compose(&local),
                _ => local,
            };
            worlds.push(world);
            hidden.push(!node.visible || node.parent.is_some_and(|p| hidden[p]));
            if hidden[k] {
                continue;
            }
            let path = prototype.path(k);
            if matches!(node.kind, NodeKind::Light(_) | NodeKind::Camera(_)) {
                self.warnings.push(format!(
                    "{path}: a light or camera in a prototype; left out"
                ));
                continue;
            }
            // A point instancer's instances, placed where it is.
            match self.data(node, &path, depth)? {
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
                let attributes = self.bound(&[(q, bound(node))])?;
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
