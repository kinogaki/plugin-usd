//! Instances as USD's own instancing. Each prototype is written once (the
//! scene authors it beneath the default prim's `Prototypes` class) and
//! shown by reference, never copied:
//!
//! - **One instance** is an instanceable prim referencing its prototype,
//!   as an instanceable prim imports.
//! - **Several** are a PointInstancer, as one imports and as Copy to
//!   Points makes them: its `Prototypes` scope holds an instanceable prim
//!   for each prototype and material its instances show it in, each
//!   instance placed by position, orientation and scale, its `id` among
//!   the `ids`, its `v` the `velocities` and every other attribute a
//!   primvar of one value an instance (`vertex`), as a mesh's are written
//!   (`Cd` the `displayColor`, in linear Rec.709). Instances a point
//!   instancer can't place (a shear) are each an instanceable prim with its
//!   own matrix.
//!
//! An instance's own `material` binds its prim, stronger than descendants
//! where its prototype binds materials of its own (Studio renders every
//! one of them in it), and a `material_map` shows a prototype written in
//! the materials it maps to, once a map.
use super::geometry::primvar;
use super::{Kinds, Of, Out};
use crate::space::matrix;
use crate::usd::scene::{Instancer, Interpolation, Node, NodeKind, Primvar, Prototype};
use crate::usd::sdf::{Array, Path, Token, Value as Usd};
use kinogaki_studio_api::format::Unique;
use kinogaki_studio_api::geometry::{Domain, Values, Xform, names};
use kinogaki_studio_api::standard::{self, Instances};
use kinogaki_studio_api::{Data, Error};
use std::collections::HashMap;
use std::sync::Arc;

/// The Instance attributes instances' prims hold: their names, and the
/// materials their prims and prototypes bind.
const PRIMS_HOLD: [&str; 3] = [names::NAME, names::MATERIAL, names::MATERIAL_MAP];

/// The Instance attributes a point instancer has no place for: names and
/// parts (every other is its `ids`, `velocities`, a primvar, or the
/// materials its prototypes bind).
const INSTANCER_LEAVES: [&str; 2] = [names::NAME, names::PART];

/// What one instance shows: the prototype (an index into the scene's), its
/// placement relative to the instances' value, and the material it binds.
pub(super) struct Shows {
    pub prototype: usize,
    pub xform: Xform,
    pub material: Option<usize>,
    pub stronger: bool,
    pub name: String,
}

impl Shows {
    /// The instance as the node `name` under `parent`.
    pub(super) fn node(&self, name: &str, parent: Option<usize>) -> Node {
        Node {
            transform: matrix(&self.xform),
            material: self.material,
            stronger: self.stronger,
            instance: Some(self.prototype),
            ..Node::group(name, parent)
        }
    }
}

impl Out<'_> {
    /// `value` (of the item named `name`) as an instance, a point
    /// instancer, or instances each of their own; none when no prototype
    /// writes anything.
    pub(super) fn instances(
        &mut self,
        value: &Instances,
        name: &str,
    ) -> Result<Option<Kinds>, Error> {
        let mut shown = Vec::with_capacity(value.len());
        for i in 0..value.len() {
            if let Some(shows) = self.shows(value, i, name)? {
                shown.push((i, shows));
            }
        }
        let kinds = match shown.len() {
            0 => return Ok(None),
            1 => shown.pop().map(|(_, shows)| Kinds::Instance(shows)),
            _ => match self.instancer(value, &shown, name)? {
                Some(instancer) => Some(Kinds::Instancer(instancer)),
                None => Some(Kinds::Instances(
                    shown.into_iter().map(|(_, s)| s).collect(),
                )),
            },
        };
        let instancer = matches!(kinds, Some(Kinds::Instancer(_)));
        let left: Vec<&str> = (value.attributes().iter())
            .filter(|(domain, _, _)| *domain == Domain::Instance)
            .filter(|(_, name, _)| match instancer {
                true => INSTANCER_LEAVES.contains(name),
                false => !PRIMS_HOLD.contains(name),
            })
            .map(|(_, name, _)| name)
            .collect();
        if !left.is_empty() {
            let left = left.join(", ");
            self.warnings.push(format!(
                "{name}: the instances' {left} have no place in USD's instancing; left out"
            ));
        }
        Ok(kinds)
    }

    /// What instance `i` of `value` shows: its prototype, in the materials
    /// its `material_map` maps to unless a whole `material` of its own
    /// binds it; none when its prototype writes nothing.
    fn shows(&mut self, value: &Instances, i: usize, name: &str) -> Result<Option<Shows>, Error> {
        let attributes = value.attributes();
        let key = attributes
            .text(Domain::Instance, names::MATERIAL)
            .and_then(|t| t.get(i));
        let map = match key {
            Some(_) => Vec::new(),
            None => standard::material_map(attributes, i),
        };
        let Some(prototype) =
            self.prototype(&value.prototypes()[value.prototype_index(i)], &map)?
        else {
            return Ok(None);
        };
        let material = key.and_then(|key| {
            let found = self.materials.get(key).copied();
            if found.is_none() {
                self.warnings.push(format!(
                    "an instance names {key}, which isn't written; left unbound"
                ));
            }
            found
        });
        let named = attributes
            .text(Domain::Instance, names::NAME)
            .and_then(|t| t.get(i));
        Ok(Some(Shows {
            prototype,
            xform: value.xforms()[i],
            material,
            stronger: material.is_some() && self.binds(prototype),
            name: named.unwrap_or(name).to_owned(),
        }))
    }

    /// `shown` (each instance of `value` that shows something, and what it
    /// shows) as a point instancer of the item named `name`; none when one
    /// of the placements is one it can't hold.
    fn instancer(
        &mut self,
        value: &Instances,
        shown: &[(usize, Shows)],
        name: &str,
    ) -> Result<Option<Instancer>, Error> {
        let (mut positions, mut orientations, mut scales) = (Vec::new(), Vec::new(), Vec::new());
        for (_, shows) in shown {
            let Some((position, orientation, scale)) = Instancer::placement(&matrix(&shows.xform))
            else {
                return Ok(None);
            };
            positions.push(position);
            orientations.push(orientation);
            scales.push(scale);
        }
        // One prototype prim for each prototype and binding shown.
        let mut targets: HashMap<(usize, Option<usize>), i32> = HashMap::new();
        let mut prototypes = Vec::new();
        let mut taken = Unique::names();
        let mut indices = Vec::with_capacity(shown.len());
        for (_, shows) in shown {
            let key = (shows.prototype, shows.material);
            let index = *targets.entry(key).or_insert_with(|| {
                // The instance's placement is the instancer's to give.
                let target = Shows {
                    xform: Xform::IDENTITY,
                    name: taken.take(name),
                    ..*shows
                };
                self.scene.prototypes.push(Prototype {
                    root: Path::absolute_root(),
                    model: Token::empty(),
                    nodes: vec![target.node(&target.name, None)],
                });
                prototypes.push(self.scene.prototypes.len() - 1);
                prototypes.len() as i32 - 1
            });
            indices.push(index);
        }
        let kept: Vec<usize> = shown.iter().map(|(i, _)| *i).collect();
        let scaled = scales.iter().any(|s| *s != [1.0; 3]);
        let attributes = value.attributes();
        let ids = (attributes.get(Domain::Instance, names::ID)).and_then(|id| match id.values() {
            Values::I64(v) if id.ty().width == 1 => Some(kept.iter().map(|&i| v[i]).collect()),
            Values::I32(v) if id.ty().width == 1 => {
                Some(kept.iter().map(|&i| i64::from(v[i])).collect())
            }
            _ => None,
        });
        Ok(Some(Instancer {
            prototypes,
            proto_indices: Arc::new(indices),
            positions: Arc::new(positions),
            orientations: Some(Arc::new(orientations)),
            scales: scaled.then(|| Arc::new(scales)),
            ids: ids.map(Arc::new),
            invisible_ids: Vec::new(),
            inactive_ids: Vec::new(),
            primvars: self.instance_primvars(value, &kept, name),
        }))
    }

    /// The instances `kept` of `value`'s `v` as `velocities` and its other
    /// attributes (but `id`, materials, names and parts) as primvars of one
    /// value an instance, as a mesh's are written (`Cd` the `displayColor`
    /// in linear Rec.709).
    fn instance_primvars(&mut self, value: &Instances, kept: &[usize], name: &str) -> Vec<Primvar> {
        const HELD: [&str; 5] = [
            names::ID,
            names::MATERIAL,
            names::MATERIAL_MAP,
            names::NAME,
            names::PART,
        ];
        let mut primvars = Vec::new();
        for (domain, attribute_name, attribute) in value.attributes().iter() {
            if domain != Domain::Instance || HELD.contains(&attribute_name) {
                continue;
            }
            if attribute_name == names::V
                && let Some(v) = value
                    .attributes()
                    .tuples::<[f32; 3]>(Domain::Instance, names::V)
            {
                let values = kept.iter().map(|&i| v[i]).collect();
                primvars.push(velocities(values));
                continue;
            }
            let (colors, warnings) = (self.writing.colors, &mut self.warnings);
            let vertex = Interpolation::Vertex;
            match primvar(
                attribute_name,
                attribute,
                vertex,
                Some(kept),
                colors,
                warnings,
            ) {
                Some(written) => primvars.push(written),
                None => warnings.push(format!(
                    "{name}: the instances' {attribute_name} has no USD primvar; left out"
                )),
            }
        }
        primvars
    }

    /// The prototype `value` is, rendered through `map` (pairs of a
    /// material its primitives name and the one they render with instead),
    /// written once: its index in the scene; none for a value nothing here
    /// writes.
    pub(super) fn prototype(
        &mut self,
        value: &Data,
        map: &[(&str, &str)],
    ) -> Result<Option<usize>, Error> {
        let pairs = map
            .iter()
            .map(|&(from, to)| (from.to_owned(), to.to_owned()));
        let of: Of = (value.identity(), pairs.collect());
        if let Some(&made) = self.stored.get(&of) {
            return Ok(made);
        }
        let mut names = Unique::names();
        let mut node = |name: &str, kind: NodeKind, material: Option<usize>| Node {
            kind,
            material,
            ..Node::group(&names.take(name), None)
        };
        let nodes = match self.kinds(value, map, "a prototype")? {
            Some(Kinds::Prims(prims)) => (prims.into_iter())
                .map(|(suffix, kind, material)| {
                    node(
                        if suffix.is_empty() {
                            "Geometry"
                        } else {
                            suffix
                        },
                        kind,
                        material,
                    )
                })
                .collect(),
            Some(Kinds::One(kind)) => vec![node("Prototype", kind, None)],
            Some(Kinds::Instance(shows)) => vec![shows.node(&names.take("Instance"), None)],
            Some(Kinds::Instancer(instancer)) => {
                vec![node(
                    "Instances",
                    NodeKind::Instancer(Box::new(instancer)),
                    None,
                )]
            }
            Some(Kinds::Instances(each)) => (each.iter())
                .map(|shows| shows.node(&names.take(&shows.name), None))
                .collect(),
            None => Vec::new(),
        };
        let made = (!nodes.is_empty()).then(|| {
            self.scene.prototypes.push(Prototype {
                root: Path::absolute_root(),
                model: Token::empty(),
                nodes,
            });
            self.scene.prototypes.len() - 1
        });
        self.stored.insert(of, made);
        Ok(made)
    }

    /// Whether prototype `index`'s prims bind materials of their own.
    fn binds(&self, index: usize) -> bool {
        (self.scene.prototypes[index].nodes.iter()).any(|node| {
            node.material.is_some()
                || node.instance.is_some_and(|p| self.binds(p))
                || match &node.kind {
                    NodeKind::Mesh(mesh) => mesh.subsets.iter().any(|s| s.material.is_some()),
                    NodeKind::Instancer(instancer) => {
                        instancer.prototypes.iter().any(|&p| self.binds(p))
                    }
                    _ => false,
                }
        })
    }
}

/// Instances' velocities, one an instance: the point instancer's own
/// `velocities`.
fn velocities(values: Vec<[f32; 3]>) -> Primvar {
    Primvar {
        name: Token::new("velocities"),
        type_name: Token::new("vector3f[]"),
        interpolation: Interpolation::Vertex,
        element_size: 1,
        values: Usd::Array(Array::Vec3f(Arc::new(values))),
        indices: None,
        color_space: None,
    }
}
