//! Materials as UsdShade networks, kept as authored: a [`Material`]'s
//! terminal outputs, its interface inputs, and the Shader and NodeGraph
//! prims beneath it with their inputs, outputs and connections.
//!
//! One material holds a network per render context, each behind its own
//! terminal output (DESIGN §8.2):
//! - `outputs:surface`: the universal context, USD Preview Surface
//!   (`UsdPreviewSurface`, `UsdUVTexture`, `UsdPrimvarReader_*`,
//!   `UsdTransform2d`), what every reader shows;
//! - `outputs:mtlx:surface`: MaterialX in USD (UsdMtlx), each shader's
//!   `info:id` a MaterialX node definition (`ND_open_pbr_surface_surfaceshader`);
//! - `outputs:kinogaki:surface`: a render engine's own nodes, each
//!   `info:id` the engine's node type.
//!
//! Nothing is interpreted here: values, types and connections are what the
//! layer says, so a network reads and writes back unchanged. Asset-valued
//! inputs (texture files) name the scene's assets. MaterialX documents a
//! material references (`references = @look.mtlx@</MaterialX/…>`) are
//! kept as [`MtlxDocument`]s for MaterialX's reader.

mod read;
mod write;

pub(crate) use read::read;
pub(crate) use write::write;

use crate::usd::sdf::{Dictionary, Path, Token, Value};

/// A Material prim and the shading network beneath it.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// Where the Material prim is.
    pub path: Path,
    /// Its `customData` (`kinogaki:name`, the name a Kinogaki material had).
    pub custom_data: Dictionary,
    /// Its interface inputs (`inputs:*`), in name order.
    pub inputs: Vec<Port>,
    /// Its terminal outputs (`outputs:surface`, `outputs:mtlx:surface`,
    /// `outputs:displacement`, …), by name without `outputs:`, in name order.
    pub outputs: Vec<Port>,
    /// The Shader and NodeGraph prims beneath it, parents before children.
    pub nodes: Vec<ShadeNode>,
    /// MaterialX documents it references.
    pub mtlx: Vec<MtlxDocument>,
}

impl Material {
    /// A material at `path` with no network.
    pub fn new(path: Path) -> Material {
        Material {
            path,
            custom_data: Dictionary::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            nodes: Vec::new(),
            mtlx: Vec::new(),
        }
    }

    /// The terminal output `name` (`surface`, `mtlx:surface`).
    pub fn output(&self, name: &str) -> Option<&Port> {
        self.outputs.iter().find(|port| port.name.as_str() == name)
    }

    /// The surface of render context `context` (`""` the universal one,
    /// `mtlx`, `kinogaki`): the node its terminal is connected to.
    pub fn surface(&self, context: &str) -> Option<&ShadeNode> {
        let name = if context.is_empty() {
            "surface".to_owned()
        } else {
            format!("{context}:surface")
        };
        let source = self.output(&name)?.sources.first()?;
        self.nodes.get(source.node?)
    }

    /// The node at `path`.
    pub fn node(&self, path: &Path) -> Option<usize> {
        self.nodes.iter().position(|node| node.path == *path)
    }
}

/// Which UsdShade prim a node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadeKind {
    /// A `Shader`: one node, its implementation named by `info:id`.
    Shader,
    /// A `NodeGraph`: nodes behind an interface of inputs and outputs.
    NodeGraph,
}

/// A Shader or NodeGraph prim of a material.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadeNode {
    /// Where the prim is.
    pub path: Path,
    /// Shader or NodeGraph.
    pub kind: ShadeKind,
    /// The NodeGraph it is inside, an index into the material's nodes.
    pub parent: Option<usize>,
    /// Its `info:*` attributes (`id`, `implementationSource`,
    /// `mtlx:sourceAsset`, …), by name without `info:`.
    pub info: Vec<Port>,
    /// Its inputs, by name without `inputs:`, in name order.
    pub inputs: Vec<Port>,
    /// Its outputs, by name without `outputs:`, in name order.
    pub outputs: Vec<Port>,
}

impl ShadeNode {
    /// `info:id`: the shader's identifier (`UsdPreviewSurface`,
    /// `ND_image_color3`).
    pub fn id(&self) -> Option<&Token> {
        let id = self.info.iter().find(|port| port.name.as_str() == "id")?;
        id.value.as_ref()?.as_token()
    }

    /// The input `name`.
    pub fn input(&self, name: &str) -> Option<&Port> {
        self.inputs.iter().find(|port| port.name.as_str() == name)
    }
}

/// One input or output.
#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    /// Its name without `inputs:` or `outputs:` (`diffuseColor`,
    /// `mtlx:surface`).
    pub name: Token,
    /// Its value type as authored (`color3f`, `float`, `asset`, `token`).
    pub type_name: Token,
    /// Whether it is `uniform` (as `info:id` is).
    pub uniform: bool,
    /// Its authored value (an asset names one of the scene's assets).
    pub value: Option<Value>,
    /// What it is connected to, strongest first.
    pub sources: Vec<Source>,
    /// The color space its value is authored in (`colorSpace`, else the
    /// nearest ColorSpaceAPI's); none: the file doesn't say.
    pub color_space: Option<Token>,
}

impl Port {
    /// An unconnected port `name` of type `type_name` holding `value`.
    pub fn new(name: &str, type_name: &str, value: Option<Value>) -> Port {
        Port {
            name: Token::new(name),
            type_name: Token::new(type_name),
            uniform: false,
            value,
            sources: Vec::new(),
            color_space: None,
        }
    }
}

/// The far end of a connection.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// The node, an index into the material's nodes; none: the material
    /// itself (one of its interface inputs).
    pub node: Option<usize>,
    /// The port's name without its prefix.
    pub port: Token,
    /// Whether the port is an output (else an interface input).
    pub output: bool,
}

/// A MaterialX document a material references as a layer: what OpenUSD
/// composes through UsdMtlx, kept for MaterialX's reader.
#[derive(Clone, Debug, PartialEq)]
pub struct MtlxDocument {
    /// The document, the path of one of the scene's assets.
    pub asset: Token,
    /// The prim referenced in it (`/MaterialX/Materials/Steel`); empty: its
    /// default.
    pub target: Path,
}
