//! The scene a stage describes, as plain data: what the format's plugin
//! maps to and from the standard values.
//!
//! A [`Scene`] is a tree of [`Node`]s (parents before children), each a
//! prim with its local transform and visibility, holding a mesh, an
//! implicit shape, curves, points, a camera, a light or a point instancer,
//! or grouping others. Instances share [`Prototype`]s: an instanceable
//! prim shows one, and a point instancer places several. Materials are
//! UsdShade networks ([`crate::usd::shade`]); the files they and lights
//! name are [`Asset`]s, a package's entries held as their bytes.
//!
//! Values are read at one time code (animation is not read) and kept as
//! authored: arrays share the layer's columns, units are the stage's
//! (`metersPerUnit`, `upAxis`), colors say the space they are in. [`read`]
//! makes a scene of a composed stage; [`author`] and [`write_file`] make
//! a layer and files of one.

mod assets;
mod author;
mod bindings;
mod camera;
mod curves;
mod instancer;
mod light;
mod matrix;
mod mesh;
mod primvar;
mod read;
mod shapes;
pub(crate) mod values;
mod write;
mod xform;

pub use assets::{Asset, AssetSource};
pub use author::Author;
pub use camera::{Camera, Projection};
pub use curves::{CurveBasis, CurveForm, CurveWrap, Curves, Points};
pub use instancer::Instancer;
pub use light::{CAMERA, CAMERA_READ, Light, LightKind, Shadow, Shaping};
pub use matrix::Matrix4d;
pub use mesh::{Mesh, Subdivision, Subset, SubsetElement};
pub use primvar::{Interpolation, Primvar};
pub use read::{ReadOptions, read};
pub use shapes::{Shape, ShapeForm};
pub use values::Time;
pub use write::{author, write_file};

use crate::usd::sdf::{Path, Token};
use crate::usd::shade::Material;

/// Which axis points up (`upAxis`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UpAxis {
    /// Y up (USD's fallback).
    #[default]
    Y,
    /// Z up.
    Z,
}

/// A stage's prims, prototypes, materials and the files they name.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// The up axis.
    pub up_axis: UpAxis,
    /// Meters per scene unit (`metersPerUnit`; USD's fallback is 0.01).
    pub meters_per_unit: f64,
    /// The prims, parents before their children.
    pub nodes: Vec<Node>,
    /// What instances show.
    pub prototypes: Vec<Prototype>,
    /// Every Material prim of the stage, bound or not.
    pub materials: Vec<Material>,
    /// The files materials and lights name.
    pub assets: Vec<Asset>,
    /// What reading left out, one line each.
    pub warnings: Vec<String>,
}

impl Default for Scene {
    /// An empty scene in USD's fallback units (centimetres, Y up).
    fn default() -> Scene {
        Scene {
            up_axis: UpAxis::Y,
            meters_per_unit: 0.01,
            nodes: Vec::new(),
            prototypes: Vec::new(),
            materials: Vec::new(),
            assets: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

impl Scene {
    /// The path of node `index` on the stage.
    pub fn path(&self, index: usize) -> Path {
        path_in(&self.nodes, Path::absolute_root(), index)
    }

    /// The asset layers name by `path`.
    pub fn asset(&self, path: &str) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.path.as_str() == path)
    }

    /// The material at `path`.
    pub fn material(&self, path: &Path) -> Option<&Material> {
        self.materials
            .iter()
            .find(|material| material.path == *path)
    }
}

/// One prim: its name, its place in the tree, its local transform and
/// what it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// The prim's name.
    pub name: Token,
    /// Its parent's index in the same list (always earlier); `None`: at the
    /// top.
    pub parent: Option<usize>,
    /// Its schema type as authored (`Xform`, `Mesh`, `Scope`, …).
    pub type_name: Token,
    /// Its model kind (`kind` metadata: `assembly`, `group`, `component`,
    /// …); empty for none.
    pub model: Token,
    /// Its transform relative to its parent (row vectors).
    pub transform: Matrix4d,
    /// Whether it ignores its parents' transforms (`!resetXformStack!`).
    pub resets_transform: bool,
    /// Its own `visibility` is not `invisible`. Visibility inherits: a
    /// visible prim under an invisible one is hidden.
    pub visible: bool,
    /// The material bound to it (UsdShade's resolution, ancestors
    /// included), an index into [`Scene::materials`], on prims that show
    /// geometry. On an instance or a point instancer, it is what the
    /// prototypes' prims bind when they bind nothing themselves, or
    /// whatever they bind when it is [`stronger`](Self::stronger).
    pub material: Option<usize>,
    /// Whether `material` is bound `strongerThanDescendants`: it binds the
    /// prims beneath, an instance's prototype's too, whatever they bind.
    pub stronger: bool,
    /// The prototype this instanceable prim shows beneath itself, an index
    /// into [`Scene::prototypes`]. Written, the prim is instanceable and
    /// references the prototype, authored once ([`author`]).
    pub instance: Option<usize>,
    /// What it holds.
    pub kind: NodeKind,
}

impl Node {
    /// A visible group named `name` under `parent`, untransformed.
    pub fn group(name: &str, parent: Option<usize>) -> Node {
        Node {
            name: Token::new(name),
            parent,
            type_name: Token::new("Xform"),
            model: Token::empty(),
            transform: Matrix4d::IDENTITY,
            resets_transform: false,
            visible: true,
            material: None,
            stronger: false,
            instance: None,
            kind: NodeKind::Group,
        }
    }
}

/// What a node holds.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    /// Other prims only (an Xform, a Scope, an instance).
    Group,
    /// A Mesh.
    Mesh(Box<Mesh>),
    /// A Cube, Sphere, Cylinder, Cone, Capsule or Plane.
    Shape(Box<Shape>),
    /// BasisCurves or NurbsCurves.
    Curves(Box<Curves>),
    /// Points.
    Points(Box<Points>),
    /// A Camera.
    Camera(Box<Camera>),
    /// A UsdLux light.
    Light(Box<Light>),
    /// A PointInstancer.
    Instancer(Box<Instancer>),
}

/// Prims instances share: an instanceable prim's descendants, or a point
/// instancer's prototype prim with its own. Written, a prototype instances
/// show is authored once, beneath the default prim's `Prototypes` class,
/// and a point instancer's beneath it ([`author`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Prototype {
    /// The path its top prims are children of on the stage: the instance
    /// its prims are read from (an instanceable prim's prototype), or the
    /// point instancer it is a prototype of. Writing chooses its own.
    pub root: Path,
    /// The model kind of the prim it is written beneath when instances
    /// show it (which their prims take through the reference); empty for
    /// none, and when read.
    pub model: Token,
    /// Its prims, parents before children; the ones at the top are placed
    /// by what shows the prototype.
    pub nodes: Vec<Node>,
}

impl Prototype {
    /// The path of node `index` on the stage.
    pub fn path(&self, index: usize) -> Path {
        path_in(&self.nodes, self.root.clone(), index)
    }
}

/// The path of `nodes[index]` below `root`.
fn path_in(nodes: &[Node], root: Path, index: usize) -> Path {
    let mut names = Vec::new();
    let mut at = Some(index);
    while let Some(node) = at.and_then(|at| nodes.get(at)) {
        names.push(node.name.clone());
        at = node.parent;
    }
    names
        .into_iter()
        .rev()
        .fold(root, |path, name| path.child(name))
}
