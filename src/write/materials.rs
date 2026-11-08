//! Materials authored as UsdShade networks, every authored network kept
//! (the owner's ruling for USDZ, 2026-11-07; DESIGN §8.2):
//!
//! | A material whose network is | is written in |
//! |---|---|
//! | USD Preview Surface's nodes alone | `outputs:surface`, as it is |
//! | MaterialX's nodes | `outputs:mtlx:surface`, and a USD Preview Surface made from it |
//! | an engine's own | `outputs:kinogaki:surface`, its standard form in `outputs:mtlx:surface`, and a USD Preview Surface made from that |
//!
//! An Image node is the file its texture node's input names: an asset of
//! the scene, packed in a usdz or written beside a layer, with its Color as
//! the input's `colorSpace` (USD UV Texture's `sourceColorSpace` too). A
//! node of Kinogaki's definitions is the MaterialX definition it inherits,
//! its own inputs beside, every one, so other applications know each
//! shader's `info:id` and Studio reads it back as it was. The material's
//! name and Double Sided are its `customData` (`kinogaki:name`,
//! `kinogaki:doubleSided`).
use super::assets::Assets;
use super::preview;
use crate::names;
use crate::shading::{
    self, DOUBLE_SIDED_KEY, KINOGAKI, MTLX, NAME_KEY, OPACITY_MODES, UNIVERSAL, terminal,
};
use crate::usd::sdf::{Path, Token, Value as Usd};
use crate::usd::shade::{Material, Port, ShadeKind, ShadeNode, Source};
use kinogaki_mtlx::{NodeDef, Type};
use kinogaki_render_api::material::mtlx::SIGNATURE;
use kinogaki_studio_api::color::{ColorService, LIN_REC709, SRGB_TEXTURE};
use kinogaki_studio_api::format::Unique;
use kinogaki_studio_api::geometry::image::ImageColor;
use kinogaki_studio_api::geometry::names::CD;
use kinogaki_studio_api::image_files::{IMAGE_NODE, image_source};
use kinogaki_studio_api::network::{NetworkSeed, NewNode};
use kinogaki_studio_api::standard::ImportedMaterial;
use kinogaki_studio_api::{Error, Value};
use std::sync::Arc;

/// What a material's network is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Family {
    /// USD Preview Surface's nodes (and Image nodes) alone.
    Preview,
    /// MaterialX's nodes.
    Mtlx,
    /// An engine's own nodes among them.
    Engine,
}

/// The MaterialX definition node `node` holds; none for another node.
pub(super) fn definition(node: &NewNode) -> Option<&'static Arc<NodeDef>> {
    match node.parameters.get(SIGNATURE) {
        Some(Value::Choice(signature)) => kinogaki_mtlx::library().def(signature),
        _ => None,
    }
}

/// The inputs of Kinogaki's definition `def` that the MaterialX one it is
/// written as ([`in_materialx`](kinogaki_mtlx::Library::in_materialx))
/// lacks or holds another default of, at their defaults: a node writes them
/// all, held or not, so another application reads what Studio does and a
/// reader knows it for `def` again
/// ([`holding`](kinogaki_mtlx::Library::holding)).
fn own_inputs(def: &Arc<NodeDef>) -> Vec<(String, Value)> {
    let library = kinogaki_mtlx::library();
    let (Some(written), Some(made)) = (
        library.in_materialx(def).filter(|m| m.name != def.name),
        kinogaki_render_api::material::mtlx::def(&def.name),
    ) else {
        return Vec::new();
    };
    (def.inputs.iter())
        .filter(|p| written.input(&p.name).is_none_or(|w| w.value != p.value))
        .filter_map(|p| {
            let default = made.find_input(&p.name)?.value()?;
            Some((p.name.clone(), default.value.clone()))
        })
        .collect()
}

/// What `seed` is made of.
pub(super) fn family(seed: &NetworkSeed) -> Family {
    let shading = seed.nodes.iter().filter(|n| n.node_type != IMAGE_NODE);
    let defs: Vec<Option<&Arc<NodeDef>>> = shading.map(definition).collect();
    if defs.iter().any(Option::is_none) {
        return Family::Engine;
    }
    let preview =
        |d: &&Arc<NodeDef>| shading::preview_id(&d.name).is_some() || d.node == "surfacematerial";
    match defs.iter().flatten().all(preview) && !defs.is_empty() {
        true => Family::Preview,
        false => Family::Mtlx,
    }
}

/// `material` as a Material prim at `path`, with what it left out.
pub(super) fn material(
    material: &ImportedMaterial,
    path: Path,
    assets: &mut Assets,
    colors: &dyn ColorService,
    warnings: &mut Vec<String>,
) -> Result<Material, Error> {
    let mut out = Material::new(path);
    out.custom_data
        .insert(Token::new(NAME_KEY), Usd::Token(Token::new(&material.name)));
    out.custom_data.insert(
        Token::new(DOUBLE_SIDED_KEY),
        Usd::Bool(material.double_sided),
    );
    let mut writer = Network {
        material: &mut out,
        names: Unique::default(),
        assets,
        colors,
        warnings,
        name: &material.name,
    };
    let said = |what: &str| format!("{}: {what}", material.name);
    match family(&material.inside) {
        Family::Preview => writer.network(UNIVERSAL, &material.inside)?,
        Family::Mtlx => {
            writer.network(MTLX, &material.inside)?;
            writer.preview(&material.inside)?;
        }
        Family::Engine => {
            writer.network(KINOGAKI, &material.inside)?;
            match &material.standard {
                Some(standard) => {
                    writer.network(MTLX, standard)?;
                    writer.preview(standard)?;
                }
                None => writer.warnings.push(said(
                    "no installed engine gives it a standard form; other applications show none",
                )),
            }
        }
    }
    Ok(out)
}

/// Authors networks into one Material prim.
struct Network<'a> {
    material: &'a mut Material,
    /// The shaders' prim names, unique across its networks.
    names: Unique,
    assets: &'a mut Assets,
    colors: &'a dyn ColorService,
    warnings: &'a mut Vec<String>,
    /// The material's name, for what is said.
    name: &'a str,
}

impl Network<'_> {
    /// A USD Preview Surface made from MaterialX network `seed`, for
    /// applications reading no other context.
    fn preview(&mut self, seed: &NetworkSeed) -> Result<(), Error> {
        let (made, said) = preview::preview(seed);
        let name = self.name;
        self.warnings
            .extend(said.into_iter().map(|s| format!("{name}: {s}")));
        match made {
            Some(made) => self.network(UNIVERSAL, &made),
            None => Ok(()),
        }
    }

    /// `seed` as the shaders of render context `context` and its terminals.
    fn network(&mut self, context: &str, seed: &NetworkSeed) -> Result<(), Error> {
        let base = self.material.nodes.len();
        let mut at: Vec<Option<usize>> = Vec::with_capacity(seed.nodes.len());
        // The shaders: every node but Image nodes (the files they name) and
        // a Surface Material (the Material prim itself).
        for node in &seed.nodes {
            let shader = node.node_type != IMAGE_NODE
                && definition(node).is_none_or(|d| d.node != "surfacematerial");
            if !shader {
                at.push(None);
                continue;
            }
            let def = definition(node);
            let id = match (def, context) {
                (Some(def), UNIVERSAL) => shading::preview_id(&def.name)
                    .ok_or_else(|| {
                        Error::invalid(format!("{} is no USD Preview Surface node", def.name))
                    })?
                    .to_owned(),
                (Some(def), _) => kinogaki_mtlx::library()
                    .in_materialx(def)
                    .unwrap_or(def)
                    .name
                    .clone(),
                (None, _) => node.node_type.to_string(),
            };
            let label = node
                .name
                .as_deref()
                .map_or_else(|| def.map_or("Shader", |d| d.node.as_str()), |n| n);
            let name = self.names.take(&names::identifier(label));
            let mut info = Port::new("id", "token", Some(Usd::Token(Token::new(&id))));
            info.uniform = true;
            at.push(Some(self.material.nodes.len()));
            self.material.nodes.push(ShadeNode {
                path: self.material.path.child(Token::new(&name)),
                kind: ShadeKind::Shader,
                parent: None,
                info: vec![info],
                inputs: Vec::new(),
                outputs: Vec::new(),
            });
        }
        for (k, node) in seed.nodes.iter().enumerate() {
            let Some(shader) = at[k] else { continue };
            let def = definition(node);
            let preview = (context == UNIVERSAL)
                .then(|| def.and_then(|d| shading::preview_id(&d.name)))
                .flatten();
            let own = def.map(own_inputs).unwrap_or_default();
            let unheld = own.iter().filter(|(i, _)| !node.parameters.contains_key(i));
            // A reader of the color attribute's fallback is a color.
            let named = node.parameters.get("varname").and_then(Value::text);
            let reads_color =
                def.is_some_and(|d| d.node == "UsdPrimvarReader") && named == Some(CD);
            for (input, value) in node.parameters.iter().chain(unheld.map(|(i, v)| (i, v))) {
                if input == SIGNATURE {
                    continue;
                }
                let colored = reads_color && input == "fallback";
                if let Some(port) = self.value((def, preview, colored), input, value, context) {
                    self.material.nodes[shader].inputs.push(port);
                }
            }
            for wire in seed.wires.iter().filter(|w| w.to == k) {
                let from = &seed.nodes[wire.from];
                let port = match at[wire.from] {
                    None if from.node_type == IMAGE_NODE => {
                        let uniform = def
                            .and_then(|d| d.input(&wire.input))
                            .is_some_and(|p| p.uniform);
                        self.file(from, &wire.input, uniform)?
                    }
                    None => continue,
                    Some(source) => {
                        let ty = port_type(def, &wire.input, preview, "token");
                        let output = self.output(source, definition(from), &wire.output, context);
                        let mut port = Port::new(&wire.input, ty, None);
                        port.sources.push(Source {
                            node: Some(source),
                            port: Token::new(&output),
                            output: true,
                        });
                        port
                    }
                };
                let inputs = &mut self.material.nodes[shader].inputs;
                inputs.retain(|p| p.name != port.name);
                inputs.push(port);
            }
            if preview == Some("UsdUVTexture") {
                let inputs = &mut self.material.nodes[shader].inputs;
                let space = inputs
                    .iter()
                    .find(|p| p.name.as_str() == "file")
                    .map(|p| p.color_space.clone());
                if let Some(space) = space {
                    // sRGB by its interop name or by MaterialX's.
                    let token = match space.as_deref() {
                        Some("raw") => "raw",
                        Some(SRGB_TEXTURE | "srgb_texture") => "sRGB",
                        _ => "auto",
                    };
                    let mut port = Port::new(
                        "sourceColorSpace",
                        "token",
                        Some(Usd::Token(Token::new(token))),
                    );
                    port.uniform = true;
                    inputs.push(port);
                }
            }
        }
        for node in &mut self.material.nodes[base..] {
            node.inputs.sort_by(|a, b| a.name.cmp(&b.name));
        }
        self.terminals(context, seed, &at)
    }

    /// The material's terminals of `context`: the shown node, or what its
    /// Surface Material's surface and displacement read.
    fn terminals(
        &mut self,
        context: &str,
        seed: &NetworkSeed,
        at: &[Option<usize>],
    ) -> Result<(), Error> {
        let Some(shown) = seed.nodes.iter().position(|n| n.display) else {
            let name = self.name;
            self.warnings.push(format!(
                "{name}: no node is shown; its {} network has no terminal",
                or_universal(context)
            ));
            return Ok(());
        };
        let mut ends: Vec<(&str, usize, String)> = Vec::new();
        match at[shown] {
            Some(shader) => {
                let output = first_output(definition(&seed.nodes[shown]));
                ends.push(("surface", shader, output));
            }
            None => {
                for wire in seed.wires.iter().filter(|w| w.to == shown) {
                    let which = match wire.input.as_str() {
                        "surfaceshader" => "surface",
                        "displacementshader" => "displacement",
                        "volumeshader" => "volume",
                        _ => continue,
                    };
                    if let Some(shader) = at[wire.from] {
                        ends.push((which, shader, wire.output.clone()));
                    }
                }
            }
        }
        for (which, shader, output) in ends {
            let def = seed
                .nodes
                .iter()
                .zip(at)
                .find(|(_, a)| **a == Some(shader))
                .and_then(|(n, _)| definition(n));
            let output = self.output(shader, def, &output, context);
            let mut port = Port::new(&terminal(context, which), "token", None);
            port.sources.push(Source {
                node: Some(shader),
                port: Token::new(&output),
                output: true,
            });
            self.material.outputs.push(port);
        }
        self.material.outputs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(())
    }

    /// Output `output` of shader `shader` (of MaterialX definition `def`, if
    /// any) as USD names it, declared on the shader.
    fn output(
        &mut self,
        shader: usize,
        def: Option<&Arc<NodeDef>>,
        output: &str,
        context: &str,
    ) -> String {
        let (name, ty) = match def {
            Some(def) => {
                let ty = def
                    .output(output)
                    .map_or("token", |o| shading::usd_type(&o.ty, None));
                let name = match context {
                    UNIVERSAL => shading::usd_output(&def.name, output),
                    _ => output,
                };
                // USD Preview Surface types its outputs as its own schema does.
                let ty = match (context, name) {
                    (UNIVERSAL, "surface" | "displacement") => "token",
                    (UNIVERSAL, "rgb") => "float3",
                    _ => ty,
                };
                (name.to_owned(), ty)
            }
            None => (output.to_owned(), "token"),
        };
        let outputs = &mut self.material.nodes[shader].outputs;
        if !outputs.iter().any(|o| o.name.as_str() == name) {
            outputs.push(Port::new(&name, ty, None));
            outputs.sort_by(|a, b| a.name.cmp(&b.name));
        }
        name
    }

    /// Input `input` of a node of definition `def` (none: an engine's)
    /// holding `value`, as a USD input: MaterialX's types, an engine's by
    /// the value's kind; colors (what USD types a color, and numbers
    /// `colored` says are one: a color reader's fallback) in the working
    /// space, USD Preview Surface's in its own linear Rec.709. None, said,
    /// for a value USD can't hold.
    fn value(
        &mut self,
        (def, preview, colored): (Option<&Arc<NodeDef>>, Option<&str>, bool),
        input: &str,
        value: &Value,
        context: &str,
    ) -> Option<Port> {
        let mut value = value.clone();
        let (ty, colored) = match def {
            Some(def) => {
                let port = def.input(input)?;
                if port.ty == Type::Filename {
                    return None;
                }
                let preview = preview.map(|p| (p, input));
                let colored = colored || shading::is_color(&port.ty, preview);
                (shading::usd_type(&port.ty, preview), colored)
            }
            None => {
                let (ty, _) = shading::engine_usd(&value)?;
                (ty, matches!(value, Value::Color(_)))
            }
        };
        match (preview, input, &value) {
            (Some("UsdPreviewSurface"), "opacityMode", Value::Integer(k)) => {
                value = Value::Choice(Arc::from(*OPACITY_MODES.get(*k as usize)?));
            }
            (Some("UsdUVTexture"), "wrapS" | "wrapT", Value::Choice(wrap)) => {
                value = Value::Choice(Arc::from(shading::usd_wrap(wrap)));
            }
            _ => {}
        }
        if let (Some(def), Value::Text(name)) = (def, &value) {
            let names = matches!(
                (def.node.as_str(), input),
                ("UsdPrimvarReader", "varname")
                    | ("geompropvalue" | "geompropvalueuniform", "geomprop")
            );
            if names {
                value = Value::Text(names::primvar(name).to_owned());
            }
        }
        let space = colored
            .then(|| self.color_space(&mut value, context))
            .flatten();
        let usd = match def {
            Some(_) => shading::usd_value(&value, ty),
            None => shading::engine_usd(&value).map(|(_, v)| v),
        };
        let Some(usd) = usd else {
            let name = self.name;
            self.warnings.push(format!(
                "{name}: {input}'s value can't be written; left out"
            ));
            return None;
        };
        let mut port = Port::new(input, ty, Some(usd));
        port.color_space = space.map(|s| Token::new(&s));
        port.uniform = def.and_then(|d| d.input(input)).is_some_and(|p| p.uniform);
        Some(port)
    }

    /// The space a color `value` (a color3, or a color4's or color reader's
    /// numbers) is written in: the working space, said on the input; USD
    /// Preview Surface's, linear Rec.709, converted to.
    fn color_space(&mut self, value: &mut Value, context: &str) -> Option<String> {
        let rgb: &mut [f64] = match value {
            Value::Color(c) => c,
            Value::Numbers(n) if n.len() >= 3 => &mut n[..3],
            _ => return None,
        };
        if context != UNIVERSAL {
            return Some(self.colors.working().to_owned());
        }
        let mut held = [[rgb[0], rgb[1], rgb[2]]];
        match self.colors.working_to(LIN_REC709, &mut held) {
            Ok(()) => rgb.copy_from_slice(&held[0]),
            Err(e) => {
                let name = self.name;
                self.warnings
                    .push(format!("{name}: colors kept in the working space: {e}"));
                return Some(self.colors.working().to_owned());
            }
        }
        None
    }

    /// Input `input` (`uniform` or not) naming the file Image node `image`
    /// reads, its Color as the input's `colorSpace`.
    fn file(&mut self, image: &NewNode, input: &str, uniform: bool) -> Result<Port, Error> {
        let source = image_source(&image.parameters)?;
        let Some(source) = source else {
            let name = self.name;
            self.warnings
                .push(format!("{name}: a texture reading a render is left out"));
            return Ok(Port::new(input, "asset", None));
        };
        let asset = self.assets.file(source.file())?;
        let mut port = Port::new(input, "asset", Some(Usd::Asset(asset)));
        port.uniform = uniform;
        // USD's spelling of data is `raw`.
        port.color_space = match source.color() {
            ImageColor::Auto => None,
            ImageColor::Data => Some(Token::new("raw")),
            ImageColor::Space(space) => Some(Token::new(space)),
        };
        Ok(port)
    }
}

/// The USD type of input `input` of a node of definition `def`, else
/// `fallback`.
fn port_type(
    def: Option<&Arc<NodeDef>>,
    input: &str,
    preview: Option<&str>,
    fallback: &'static str,
) -> &'static str {
    match def.and_then(|d| d.input(input)) {
        Some(port) => shading::usd_type(&port.ty, preview.map(|p| (p, input))),
        None => fallback,
    }
}

/// A shown node's output: its definition's first, an engine surface's
/// `out`.
fn first_output(def: Option<&Arc<NodeDef>>) -> String {
    def.and_then(|d| d.outputs.first())
        .map_or("out", |o| &o.name)
        .to_owned()
}

/// A context's name in what is said.
fn or_universal(context: &str) -> &str {
    match context {
        UNIVERSAL => "USD Preview Surface",
        context => context,
    }
}
