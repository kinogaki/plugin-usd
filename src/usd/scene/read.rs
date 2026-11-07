//! A composed stage read as a [`Scene`]: the stage flattened with its
//! instances kept (a single layer is read as it is), then walked prim by
//! prim, accumulating material bindings.
//!
//! Which prims load: defined, active prims (`over`s too within
//! prototypes), of purpose `default` and `render` (`proxy` and `guide` when
//! asked). Invisible prims load, marked. A group that holds nothing loads
//! as nothing. Instanceable prims show shared prototypes, and a point
//! instancer's prototypes are its own (not drawn in place beneath it).
//! The scene's warnings start with what composing the stage reported.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::light::LightKind;
use super::values::{Time, token_at};
use super::xform::local_transform;
use super::{
    Asset, AssetSource, Node, NodeKind, Prototype, Scene, UpAxis, camera, curves, instancer, light,
    mesh, shapes,
};
use crate::usd::compose::{ForeignArc, assets};
use crate::usd::error::Result;
use crate::usd::sdf::{LayerData, Path, PathMap, Specifier, Token, Value, field};
use crate::usd::shade::{self, Material, MtlxDocument};
use crate::usd::{FlattenOptions, Stage};

/// The deepest prim nesting walked.
const MOST_DEPTH: usize = 512;

/// The most warnings a read keeps.
const MOST_WARNINGS: usize = 1000;

/// What [`read`] reads.
#[derive(Clone, Debug, Default)]
pub struct ReadOptions {
    /// The time code to read at; the stage's `startTimeCode` (else the
    /// default values) when none.
    pub time: Option<f64>,
    /// Also prims of purpose `proxy`.
    pub proxy: bool,
    /// Also prims of purpose `guide`.
    pub guide: bool,
}

/// The scene the composed `stage` describes.
pub fn read(stage: &mut Stage, options: &ReadOptions) -> Result<Scene> {
    let time = match options.time.or(stage.start_time_code()) {
        Some(code) => Time::Code(code),
        None => Time::Default,
    };
    let up_axis = match stage.up_axis().as_str() {
        "Z" => UpAxis::Z,
        _ => UpAxis::Y,
    };
    let meters_per_unit = Some(stage.meters_per_unit())
        .filter(|m| *m > 0.0 && m.is_finite())
        .unwrap_or(0.01);
    let flattened = match stage.is_single_layer() {
        true => None,
        false => Some(stage.flatten_with(FlattenOptions {
            comment: false,
            instancing: true,
        })?),
    };
    let anchor = stage.root_identifier().to_owned();
    let cancel = stage.cancel();
    let mut walker = Walker {
        cancel,
        layer: flattened.as_ref().unwrap_or_else(|| stage.root_layer()),
        foreign: stage.foreign_arcs(),
        options,
        time,
        assets: AssetPaths {
            anchor: &anchor,
            seen: HashSet::new(),
            paths: Vec::new(),
        },
        prototypes: Vec::new(),
        prototype_at: PathMap::default(),
        reading: Vec::new(),
        materials: Vec::new(),
        material_at: PathMap::default(),
        warnings: Vec::new(),
        collections_said: false,
    };
    let mut nodes = Vec::new();
    let top = Inherited {
        binding: None,
        depth: 0,
    };
    walker.children(&Path::absolute_root(), None, top, false, &mut nodes);
    walker.materials_everywhere(&Path::absolute_root(), 0);
    stage.check()?;
    let Walker {
        prototypes,
        materials,
        warnings,
        assets,
        ..
    } = walker;
    let paths = assets.paths;
    let mut scene = Scene {
        up_axis,
        meters_per_unit,
        nodes,
        prototypes,
        materials,
        assets: Vec::with_capacity(paths.len()),
        warnings: stage.warnings().into_iter().chain(warnings).collect(),
    };
    for (path, authored) in paths {
        let source = asset_source(stage, &path);
        if source == AssetSource::Missing {
            scene.warnings.push(format!("{authored}: no such file"));
        }
        scene.assets.push(Asset {
            path,
            authored,
            source,
        });
    }
    Ok(scene)
}

/// Where the asset `path` names is: held bytes (a package's entry, a file
/// given in memory), a file on disk, or nowhere.
fn asset_source(stage: &mut Stage, path: &str) -> AssetSource {
    if let Some(bytes) = stage.bytes(path) {
        return AssetSource::Bytes(bytes);
    }
    if assets::package_split(path).is_none() && stage.exists(path) {
        return AssetSource::File(path.into());
    }
    AssetSource::Missing
}

/// What a prim takes from its ancestors.
#[derive(Clone, Copy)]
struct Inherited {
    binding: Option<Binding>,
    depth: usize,
}

/// A material binding: the material, and whether it overrides the
/// bindings of descendants (`bindMaterialAs = "strongerThanDescendants"`).
#[derive(Clone, Copy)]
struct Binding {
    material: usize,
    stronger: bool,
}

/// The asset paths a scene names, anchored, each once in the order met,
/// with the path first authored for each.
struct AssetPaths<'a> {
    anchor: &'a str,
    seen: HashSet<Token>,
    paths: Vec<(Token, Token)>,
}

impl AssetPaths<'_> {
    /// The path the scene names `authored` by (anchored at the root layer
    /// where a flattening left it relative), recorded.
    fn name(&mut self, authored: &Token) -> Token {
        let path = Token::from(assets::anchored(authored, self.anchor));
        if self.seen.insert(path.clone()) {
            self.paths.push((path.clone(), authored.clone()));
        }
        path
    }
}

struct Walker<'a> {
    /// Raised once the scene is no longer wanted: the walk stops.
    cancel: Option<Arc<AtomicBool>>,
    layer: &'a LayerData,
    /// Arcs to MaterialX documents, which composition leaves to us.
    foreign: &'a [ForeignArc],
    options: &'a ReadOptions,
    time: Time,
    assets: AssetPaths<'a>,
    prototypes: Vec<Prototype>,
    prototype_at: PathMap<usize>,
    /// The prototypes being read, innermost last.
    reading: Vec<usize>,
    materials: Vec<Material>,
    material_at: PathMap<Option<usize>>,
    warnings: Vec<String>,
    collections_said: bool,
}

impl Walker<'_> {
    fn warn(&mut self, message: String) {
        if self.warnings.len() < MOST_WARNINGS {
            self.warnings.push(message);
        }
    }

    /// The children of `prim`, each walked into `out` under `parent`.
    fn children(
        &mut self,
        prim: &Path,
        parent: Option<usize>,
        inherited: Inherited,
        prototype: bool,
        out: &mut Vec<Node>,
    ) {
        let Some(spec) = self.layer.spec(prim) else {
            return;
        };
        for name in spec.tokens(field::PRIM_CHILDREN) {
            // The flattened stage's prototypes are reached through their instances.
            if prim.is_absolute_root() && name.starts_with("Flattened_Prototype_") {
                continue;
            }
            self.prim(&prim.child(name.clone()), parent, inherited, prototype, out);
        }
    }

    /// The prim at `path` and its descendants, into `out` under `parent`;
    /// `prototype`: within a prototype, where `over`s define prims.
    fn prim(
        &mut self,
        path: &Path,
        parent: Option<usize>,
        inherited: Inherited,
        prototype: bool,
        out: &mut Vec<Node>,
    ) {
        if self
            .cancel
            .as_deref()
            .is_some_and(|c| c.load(Ordering::Relaxed))
        {
            return;
        }
        if inherited.depth > MOST_DEPTH {
            self.warn(format!("{path}: prims nest too deeply; left out"));
            return;
        }
        let Some(spec) = self.layer.spec(path) else {
            return;
        };
        let specifier = spec.get(field::SPECIFIER).and_then(Value::as_specifier);
        let defined =
            specifier == Some(Specifier::Def) || (prototype && specifier == Some(Specifier::Over));
        if !defined || spec.get(field::ACTIVE).and_then(Value::as_bool) == Some(false) {
            return;
        }
        let type_name = (spec.get(field::TYPE_NAME))
            .and_then(Value::as_token)
            .cloned()
            .unwrap_or_default();
        match type_name.as_str() {
            "Material" => {
                self.material(path);
                return;
            }
            "GeomSubset" | "Shader" | "NodeGraph" => return,
            _ => {}
        }
        let loads = match token_at(self.layer, path, "purpose", "default").as_str() {
            "default" | "render" => true,
            "proxy" => self.options.proxy,
            "guide" => self.options.guide,
            _ => false,
        };
        if !loads {
            return;
        }
        let own = self.binding(path);
        let binding = match (inherited.binding, own) {
            (Some(up), _) if up.stronger => Some(up),
            (_, Some(own)) => Some(own),
            (up, None) => up,
        };
        let here = Inherited {
            binding,
            depth: inherited.depth + 1,
        };
        let kind = self.kind(path, &type_name, here.depth);
        let instance = self.instance(path, here.depth);
        let local = local_transform(self.layer, path, self.time);
        let shows_material = instance.is_some()
            || !matches!(
                kind,
                NodeKind::Group | NodeKind::Camera(_) | NodeKind::Light(_)
            );
        let index = out.len();
        out.push(Node {
            name: path.name_token(),
            parent,
            type_name,
            transform: local.matrix,
            resets_transform: local.resets,
            visible: token_at(self.layer, path, "visibility", "inherited").as_str() != "invisible",
            material: binding.filter(|_| shows_material).map(|b| b.material),
            instance,
            kind,
        });
        // A point instancer's children are its prototypes, not drawn in place.
        if !matches!(out[index].kind, NodeKind::Instancer(_)) {
            self.children(path, Some(index), here, prototype, out);
        }
        let empty = matches!(out[index].kind, NodeKind::Group)
            && out[index].instance.is_none()
            && out.len() == index + 1;
        if empty {
            out.truncate(index);
        }
    }

    /// What the prim at `path` of type `type_name` holds; a group when it
    /// can't be read, said once.
    fn kind(&mut self, path: &Path, type_name: &Token, depth: usize) -> NodeKind {
        let (layer, time) = (self.layer, self.time);
        let read = match type_name.as_str() {
            "Mesh" => mesh::read(layer, path, time, &mut self.warnings).map(|mut mesh| {
                for subset in &mut mesh.subsets {
                    let own = self.binding(&path.child(subset.name.clone()));
                    subset.material = own.map(|b| b.material);
                }
                NodeKind::Mesh(Box::new(mesh))
            }),
            "BasisCurves" | "NurbsCurves" => {
                let nurbs = type_name.as_str() == "NurbsCurves";
                curves::read(layer, path, nurbs, time).map(|c| NodeKind::Curves(Box::new(c)))
            }
            "Points" => {
                curves::read_points(layer, path, time).map(|p| NodeKind::Points(Box::new(p)))
            }
            "Camera" => Ok(NodeKind::Camera(Box::new(camera::read(layer, path, time)))),
            "PointInstancer" => self
                .instancer_prototypes(path, depth)
                .and_then(|prototypes| {
                    instancer::read(layer, path, prototypes, time)
                        .map(|i| NodeKind::Instancer(Box::new(i)))
                }),
            name if LightKind::unread(name) => Err(format!("{name} is not read")),
            name => match (
                LightKind::of_type(name),
                shapes::read(layer, path, name, time),
            ) {
                (Some(kind), _) => {
                    let mut light = light::read(layer, path, kind, time);
                    light.texture = light.texture.map(|file| self.assets.name(&file));
                    Ok(NodeKind::Light(Box::new(light)))
                }
                (None, Some(shape)) => Ok(NodeKind::Shape(Box::new(shape))),
                (None, None) => Ok(NodeKind::Group),
            },
        };
        read.unwrap_or_else(|why| {
            self.warn(format!("{path}: {why}; read as a group"));
            NodeKind::Group
        })
    }

    /// The prototype an instanceable prim at `path` shows: the flattened
    /// stage's prototype its internal reference names.
    fn instance(&mut self, path: &Path, depth: usize) -> Option<usize> {
        let spec = self.layer.spec(path)?;
        if spec.get(field::INSTANCEABLE).and_then(Value::as_bool) != Some(true) {
            return None;
        }
        let Some(Value::ReferenceListOp(list)) = spec.get(field::REFERENCES) else {
            return None;
        };
        let target = (list.applied().into_iter())
            .find(|r| r.asset.is_empty() && r.prim.name().starts_with("Flattened_Prototype_"))?
            .prim;
        match self.prototype(&target, false, depth) {
            Ok(index) => Some(index),
            Err(why) => {
                self.warn(format!("{path}: {why}; it shows nothing"));
                None
            }
        }
    }

    /// The prototypes of the point instancer at `path`, in its
    /// `prototypes` targets' order.
    fn instancer_prototypes(
        &mut self,
        path: &Path,
        depth: usize,
    ) -> std::result::Result<Vec<usize>, String> {
        let relationship = path.property("prototypes");
        let targets = match self.layer.get(&relationship, field::TARGET_PATHS) {
            Some(Value::PathListOp(list)) => list.applied(),
            _ => Vec::new(),
        };
        (targets.iter())
            .map(|target| self.prototype(target, true, depth))
            .collect()
    }

    /// The prototype read from `source`, once: the prim itself (`whole`)
    /// or its children. `Err` when `source` is being read, a prototype
    /// that holds itself.
    fn prototype(
        &mut self,
        source: &Path,
        whole: bool,
        depth: usize,
    ) -> std::result::Result<usize, String> {
        if let Some(&known) = self.prototype_at.get(source) {
            if self.reading.contains(&known) {
                return Err(format!("its prototype {source} holds itself"));
            }
            return Ok(known);
        }
        let index = self.prototypes.len();
        let root = if whole {
            source.parent()
        } else {
            source.clone()
        };
        self.prototypes.push(Prototype {
            root,
            nodes: Vec::new(),
        });
        self.prototype_at.insert(source.clone(), index);
        self.reading.push(index);
        let mut nodes = Vec::new();
        let top = Inherited {
            binding: None,
            depth,
        };
        if whole {
            self.prim(source, None, top, true, &mut nodes);
        } else {
            self.children(source, None, top, true, &mut nodes);
        }
        self.reading.pop();
        self.prototypes[index].nodes = nodes;
        Ok(index)
    }

    /// The material bound to the prim at `path` itself: its full-purpose
    /// binding, else its all-purpose one.
    fn binding(&mut self, path: &Path) -> Option<Binding> {
        if !self.collections_said && self.binds_collections(path) {
            self.collections_said = true;
            self.warn(format!(
                "{path}: collection-based material bindings are not read"
            ));
        }
        for name in ["material:binding:full", "material:binding"] {
            let relationship = path.property(name);
            let Some(spec) = self.layer.spec(&relationship) else {
                continue;
            };
            let Some(Value::PathListOp(list)) = spec.get(field::TARGET_PATHS) else {
                continue;
            };
            let Some(target) = list.applied().into_iter().next() else {
                continue;
            };
            let stronger = matches!(spec.get("bindMaterialAs"), Some(Value::Token(t)) if t.as_str() == "strongerThanDescendants");
            let material = self.material(&target)?;
            return Some(Binding { material, stronger });
        }
        None
    }

    fn binds_collections(&self, path: &Path) -> bool {
        let properties = self
            .layer
            .spec(path)
            .map_or(&[][..], |spec| spec.tokens(field::PROPERTIES));
        properties
            .iter()
            .any(|p| p.starts_with("material:binding:collection:"))
    }

    /// The Material at `path`, read once; none when no Material is there.
    /// A material in a referenced MaterialX document (UsdMtlx's layout:
    /// `/MaterialX/Materials/<name>` under the referencing prim) is one
    /// with no network, naming its document.
    fn material(&mut self, path: &Path) -> Option<usize> {
        if let Some(&known) = self.material_at.get(path) {
            return known;
        }
        let type_name = self
            .layer
            .get(path, field::TYPE_NAME)
            .and_then(Value::as_token);
        let typed = type_name.is_some_and(|t| t.as_str() == "Material");
        let mut documents = Vec::new();
        for arc in self.foreign {
            if path.has_prefix(&arc.prim) {
                let root = match arc.target.is_empty() {
                    true => Path::absolute_root().child("MaterialX"),
                    false => arc.target.clone(),
                };
                documents.push(MtlxDocument {
                    asset: self.assets.name(&Token::from(arc.asset.as_str())),
                    target: path.replace_prefix(&arc.prim, &root),
                });
            }
        }
        let found = if typed {
            let Walker {
                layer,
                time,
                assets,
                warnings,
                ..
            } = self;
            let mut material =
                shade::read(layer, path, *time, &mut |file| assets.name(file), warnings);
            material.mtlx = documents;
            self.materials.push(material);
            Some(self.materials.len() - 1)
        } else if !documents.is_empty() {
            let mut material = Material::new(path.clone());
            material.mtlx = documents;
            self.materials.push(material);
            Some(self.materials.len() - 1)
        } else {
            self.warn(format!("{path}: bound as a material, but it is not one"));
            None
        };
        self.material_at.insert(path.clone(), found);
        found
    }

    /// Every defined Material prim beneath `prim` (a library's unbound
    /// materials too), outside prototypes.
    fn materials_everywhere(&mut self, prim: &Path, depth: usize) {
        if depth > MOST_DEPTH {
            return;
        }
        let Some(spec) = self.layer.spec(prim) else {
            return;
        };
        for name in spec.tokens(field::PRIM_CHILDREN) {
            let child = prim.child(name.clone());
            let Some(child_spec) = self.layer.spec(&child) else {
                continue;
            };
            if child_spec
                .get(field::SPECIFIER)
                .and_then(Value::as_specifier)
                != Some(Specifier::Def)
                || child_spec.get(field::ACTIVE).and_then(Value::as_bool) == Some(false)
            {
                continue;
            }
            match child_spec
                .get(field::TYPE_NAME)
                .and_then(Value::as_token)
                .map(|t| t.as_str())
            {
                Some("Material") => {
                    self.material(&child);
                }
                Some("Shader" | "NodeGraph" | "GeomSubset") => {}
                _ => self.materials_everywhere(&child, depth + 1),
            }
        }
    }
}
