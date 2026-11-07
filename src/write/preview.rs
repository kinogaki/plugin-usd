//! A USD Preview Surface made from a MaterialX network, for applications
//! reading no other render context (AR Quick Look, most viewers).
//!
//! The shown surface's model is translated by MaterialX's own translation
//! graphs (`open_pbr_to_standard_surface`, then
//! `standard_surface_to_UsdPreviewSurface`; Kinogaki's from glTF PBR and
//! Surface Unlit), evaluated on its inputs' values. A texture feeding an
//! input carries over as a USD UV Texture of the same file: the outputs it
//! moves, and how (the graphs are affine in it), are found by evaluating
//! them with that input at zero and at one, which gives the texture's
//! `scale` and `bias`. A normal map carries over into `normal`. Anything
//! else feeding the surface (a procedural, arithmetic over textures)
//! leaves its input at its value, said.
use super::materials::definition;
use crate::names;
use kinogaki_mtlx::{Binding, Graph, NodeDef, Type, Value as M, library};
use kinogaki_studio_api::Value;
use kinogaki_studio_api::image_files::IMAGE_NODE;
use kinogaki_studio_api::network::{NetworkSeed, NewNode};
use std::collections::HashMap;
use std::sync::Arc;

/// USD Preview Surface's definition.
const SURFACE: &str = "ND_UsdPreviewSurface_surfaceshader";

/// The USD Preview Surface `seed`'s shown MaterialX surface translates
/// to, and what was approximated or couldn't be; none for a surface no
/// translation reaches.
pub(super) fn preview(seed: &NetworkSeed) -> (Option<NetworkSeed>, Vec<String>) {
    let mut said = Vec::new();
    let made = make(seed, &mut said);
    (made, said)
}

fn make(seed: &NetworkSeed, said: &mut Vec<String>) -> Option<NetworkSeed> {
    let shown = seed.nodes.iter().position(|n| n.display)?;
    let surface = match definition(&seed.nodes[shown]) {
        Some(def) if def.node == "surfacematerial" => wired(seed, shown, "surfaceshader")?.0,
        _ => shown,
    };
    let Some(def) = definition(&seed.nodes[surface]) else {
        said.push("its standard form has no MaterialX surface; no USD Preview Surface".into());
        return None;
    };
    let Some(chain) = library().translation(&def.node, |m| m == "UsdPreviewSurface") else {
        said.push(format!(
            "{} has no translation into USD Preview Surface",
            def.node
        ));
        return None;
    };
    // The surface's inputs: values, and textures.
    let mut values: HashMap<String, M> = HashMap::new();
    let mut textures: Vec<(&str, Texture)> = Vec::new();
    let mut normal: Option<Texture> = None;
    for port in &def.inputs {
        match wired(seed, surface, &port.name) {
            Some((from, output)) => {
                let node = &seed.nodes[from];
                let normals = definition(node)
                    .is_some_and(|d| d.node == "normalmap" || d.node == "gltf_normalmap");
                if port.ty == Type::Vector3 && port.geomprop.as_deref() == Some("Nworld") {
                    normal = match normals {
                        true => {
                            texture_into(seed, from, "in").or_else(|| texture(seed, from, output))
                        }
                        false => None,
                    };
                    if normal.is_none() {
                        said.push(format!(
                            "{} reads no normal map USD Preview Surface holds; left out",
                            port.name
                        ));
                    }
                    continue;
                }
                match texture(seed, from, output) {
                    Some(t) => textures.push((&port.name, t)),
                    None => said.push(format!(
                        "{}'s network has no USD Preview Surface form; its value stands",
                        port.name
                    )),
                }
            }
            None => {
                let held = seed.nodes[surface].parameters.get(&port.name);
                if let Some(v) = held.and_then(mtlx_of) {
                    values.insert(port.name.clone(), v);
                }
            }
        }
    }
    let base = translate(&chain, &values);
    let target = library().def(SURFACE)?;
    let mut out = NetworkSeed::default();
    let at = out.node(NewNode::materialx(SURFACE)?.named("Preview").displayed());
    // What differs from USD Preview Surface's defaults.
    for (name, value) in &base {
        if let Some(port) = target.input(name)
            && port.value.as_ref() != Some(value)
            && let Some(v) = value_of(value, &port.ty)
        {
            out.nodes[at].parameters.insert(name.clone(), v);
        }
    }
    let mut made = Made::default();
    for (input, texture) in &textures {
        let width = def.input(input).map_or(1, |p| p.ty.width().max(1));
        let at_value = |k: f64| {
            let mut v = values.clone();
            v.insert((*input).to_owned(), M::Numbers(vec![k; width]));
            translate(&chain, &v)
        };
        let (zero, one) = (at_value(0.0), at_value(1.0));
        for (output, low) in &zero {
            let (Some(high), Some(port)) = (one.get(output), target.input(output)) else {
                continue;
            };
            let (Some(low), Some(high)) = (low.numbers(), high.numbers()) else {
                continue;
            };
            if low == high {
                continue;
            }
            if made.driven.contains(output) {
                said.push(format!(
                    "{output} reads several textures; the first carries over"
                ));
                continue;
            }
            let channel = match (port.ty.width(), texture.channel) {
                (3, None) => "rgb",
                (1, Some(c)) => ["r", "g", "b", "a"][c],
                (1, None) => "r",
                _ => {
                    said.push(format!(
                        "{input}'s texture doesn't fit {output}; its value stands"
                    ));
                    continue;
                }
            };
            let scale: Vec<f64> = high.iter().zip(low).map(|(h, l)| h - l).collect();
            let node = made.texture(seed, &mut out, texture, (low, &scale));
            out.wire(node, channel, at, output);
            out.nodes[at].parameters.remove(output);
            made.driven.push(output.clone());
        }
    }
    if let Some(texture) = normal {
        let node = made.texture(seed, &mut out, &texture, (&[-1.0; 3], &[2.0; 3]));
        out.wire(node, "rgb", at, "normal");
    }
    if texture_transforms_approximated(&textures) {
        said.push("glTF texture transforms are approximated".into());
    }
    Some(out)
}

/// What wires `node`'s input `input`: the node and its output.
fn wired<'s>(seed: &'s NetworkSeed, node: usize, input: &str) -> Option<(usize, &'s str)> {
    (seed.wires.iter())
        .find(|w| w.to == node && w.input == input)
        .map(|w| (w.from, w.output.as_str()))
}

/// A texture a surface's input reads: its Image node, the channel read
/// (none: its color), wrap, texture coordinates and their placement, and
/// the factor its node scales by.
#[derive(Clone, Debug)]
struct Texture {
    image: usize,
    channel: Option<usize>,
    wrap: [String; 2],
    uvset: String,
    /// Scale, rotation (degrees) and translation of the coordinates.
    placement: Option<([f64; 2], f64, [f64; 2])>,
    factor: [f64; 4],
    /// From a glTF node with a transform.
    gltf_transform: bool,
}

/// The texture input `input` of `node` reads, if it reads one.
fn texture_into(seed: &NetworkSeed, node: usize, input: &str) -> Option<Texture> {
    let (from, output) = wired(seed, node, input)?;
    texture(seed, from, output)
}

/// The texture output `output` of node `from` is, looking through the
/// nodes that pick its channels.
fn texture(seed: &NetworkSeed, from: usize, output: &str) -> Option<Texture> {
    let node = &seed.nodes[from];
    let def = definition(node)?;
    let number = |name: &str| -> Option<Vec<f64>> {
        match node.parameters.get(name) {
            Some(Value::Number(n)) => Some(vec![*n]),
            Some(Value::Numbers(n)) => Some(n.clone()),
            Some(Value::Color(c)) => Some(c.to_vec()),
            Some(Value::Integer(n)) => Some(vec![*n as f64]),
            _ => def
                .input(name)?
                .value
                .as_ref()?
                .numbers()
                .map(<[f64]>::to_vec),
        }
    };
    let text = |name: &str| -> String {
        match node.parameters.get(name) {
            Some(Value::Choice(t)) => t.to_string(),
            Some(Value::Text(t)) => t.clone(),
            _ => match def.input(name).and_then(|p| p.value.as_ref()) {
                Some(M::Text(t)) => t.clone(),
                _ => String::new(),
            },
        }
    };
    let file = || {
        let (image, _) = wired(seed, from, "file")?;
        (seed.nodes[image].node_type == IMAGE_NODE).then_some(image)
    };
    let mut made = match def.node.as_str() {
        "extract" => {
            let mut t = texture_into(seed, from, "in")?;
            t.channel = Some(number("index")?.first().copied()? as usize).filter(|c| *c < 4);
            return t.channel.is_some().then_some(t);
        }
        "convert" | "dot" => return texture_into(seed, from, "in"),
        "image" | "tiledimage" | "gltf_image" | "gltf_colorimage" => {
            let mut t = Texture {
                image: file()?,
                channel: None,
                wrap: [text("uaddressmode"), text("vaddressmode")],
                uvset: uvset(seed, from, "texcoord"),
                placement: None,
                factor: [1.0; 4],
                gltf_transform: false,
            };
            if def.node == "tiledimage" {
                let (tiling, offset) = (number("uvtiling")?, number("uvoffset")?);
                if tiling != [1.0, 1.0] || offset != [0.0, 0.0] {
                    t.placement = Some(([tiling[0], tiling[1]], 0.0, [-offset[0], -offset[1]]));
                }
            }
            if def.node.starts_with("gltf") {
                let (scale, rotate, offset) =
                    (number("scale")?, number("rotate")?, number("offset")?);
                if scale != [1.0, 1.0] || rotate != [0.0] || offset != [0.0, 0.0] {
                    t.placement = Some(([scale[0], scale[1]], rotate[0], [offset[0], offset[1]]));
                    t.gltf_transform = true;
                }
                let factor = number(if def.node == "gltf_colorimage" {
                    "color"
                } else {
                    "factor"
                });
                if let Some(f) = factor {
                    let f = if f.len() == 1 { vec![f[0]; 3] } else { f };
                    for (k, v) in f.iter().take(4).enumerate() {
                        t.factor[k] = *v;
                    }
                }
                if output == "outa" {
                    t.channel = Some(3);
                }
            }
            t
        }
        "UsdUVTexture" => {
            let mut t = Texture {
                image: file()?,
                channel: ["r", "g", "b", "a"].iter().position(|c| *c == output),
                wrap: [text("wrapS"), text("wrapT")],
                uvset: uvset(seed, from, "st"),
                placement: None,
                factor: [1.0; 4],
                gltf_transform: false,
            };
            if let Some(scale) = number("scale") {
                for (k, v) in scale.iter().take(4).enumerate() {
                    t.factor[k] = *v;
                }
            }
            t
        }
        _ => return None,
    };
    if def.outputs.first().is_some_and(|o| o.ty == Type::Float) && made.channel.is_none() {
        made.channel = Some(0);
    }
    Some(made)
}

/// The texture coordinates input `input` of node `node` reads: the
/// attribute a primvar reader or geometric property names, `uv2` for a
/// second set, else `uv`.
fn uvset(seed: &NetworkSeed, node: usize, input: &str) -> String {
    let Some((from, _)) = wired(seed, node, input) else {
        return "uv".into();
    };
    let reader = &seed.nodes[from];
    let held = |name: &str| match reader.parameters.get(name) {
        Some(Value::Text(t)) => Some(t.clone()),
        Some(Value::Integer(k)) if name == "index" => Some(match k {
            0 => "uv".into(),
            k => format!("uv{}", k + 1),
        }),
        _ => None,
    };
    match definition(reader).map(|d| d.node.as_str()) {
        Some("UsdPrimvarReader") => held("varname"),
        Some("geompropvalue") => held("geomprop"),
        Some("texcoord") => held("index"),
        _ => None,
    }
    .unwrap_or_else(|| "uv".into())
}

fn texture_transforms_approximated(textures: &[(&str, Texture)]) -> bool {
    textures.iter().any(|(_, t)| t.gltf_transform)
}

/// The nodes made so far, each once: texture coordinates by set and
/// placement, textures by what they read.
#[derive(Default)]
struct Made {
    images: HashMap<usize, usize>,
    readers: HashMap<String, usize>,
    placed: Vec<((String, [u64; 5]), usize)>,
    /// The surface inputs a texture drives.
    driven: Vec<String>,
}

impl Made {
    /// A USD UV Texture reading `texture`, its values `bias` + `scale` ×
    /// the texture (per channel, a float's on its channel).
    fn texture(
        &mut self,
        seed: &NetworkSeed,
        out: &mut NetworkSeed,
        texture: &Texture,
        (bias, scale): (&[f64], &[f64]),
    ) -> usize {
        let image = *self
            .images
            .entry(texture.image)
            .or_insert_with(|| out.node(seed.nodes[texture.image].clone()));
        let reader = match self.readers.get(&texture.uvset) {
            Some(&r) => r,
            None => {
                let named = format!("Reader_{}", names::identifier(&texture.uvset));
                let node = NewNode::materialx("ND_UsdPrimvarReader_vector2")
                    .expect("MaterialX's")
                    .named(named)
                    .with("varname", Value::Text(texture.uvset.clone()));
                let r = out.node(node);
                self.readers.insert(texture.uvset.clone(), r);
                r
            }
        };
        let st = match texture.placement {
            None => (reader, "out"),
            Some((scale, rotation, translation)) => {
                let key = (
                    texture.uvset.clone(),
                    [scale[0], scale[1], rotation, translation[0], translation[1]]
                        .map(f64::to_bits),
                );
                let placed = match self.placed.iter().find(|(k, _)| *k == key) {
                    Some((_, p)) => *p,
                    None => {
                        let node = NewNode::materialx("ND_UsdTransform2d")
                            .expect("MaterialX's")
                            .named("Placement")
                            .with("scale", Value::Numbers(scale.to_vec()))
                            .with("rotation", Value::Number(rotation))
                            .with("translation", Value::Numbers(translation.to_vec()));
                        let p = out.node(node);
                        out.wire(reader, "out", p, "in");
                        self.placed.push((key, p));
                        p
                    }
                };
                (placed, "out")
            }
        };
        // Per channel: what the texture's factor and the translation give.
        let mut s = [1.0; 4];
        let mut b = [0.0; 4];
        let channels: Vec<usize> = match texture.channel {
            Some(c) => vec![c],
            None => vec![0, 1, 2],
        };
        for (k, &c) in channels.iter().enumerate() {
            let at = k.min(scale.len() - 1);
            s[c] = scale[at] * texture.factor[c];
            b[c] = bias[at.min(bias.len() - 1)];
        }
        let wrap = |w: &str| {
            let w = match w {
                "" => "periodic",
                "constant" => "black",
                w => w,
            };
            Value::Choice(Arc::from(w))
        };
        let mut node = NewNode::materialx("ND_UsdUVTexture_23")
            .expect("MaterialX's")
            .named("Texture")
            .with("wrapS", wrap(&texture.wrap[0]))
            .with("wrapT", wrap(&texture.wrap[1]));
        if s != [1.0; 4] {
            node = node.with("scale", Value::Numbers(s.to_vec()));
        }
        if b != [0.0; 4] {
            node = node.with("bias", Value::Numbers(b.to_vec()));
        }
        let t = out.node(node);
        out.wire(
            image,
            kinogaki_studio_api::image_files::image_node::IMAGE,
            t,
            "file",
        );
        out.wire(st.0, st.1, t, "st");
        t
    }
}

/// A node's value as MaterialX's.
fn mtlx_of(value: &Value) -> Option<M> {
    Some(match value {
        Value::Bool(b) => M::Boolean(*b),
        Value::Integer(n) => M::Integer(*n),
        Value::Number(n) => M::Numbers(vec![*n]),
        Value::Color(c) => M::Numbers(c.to_vec()),
        Value::Numbers(n) => M::Numbers(n.clone()),
        Value::Choice(c) => M::Text(c.to_string()),
        Value::Text(t) => M::Text(t.clone()),
        _ => return None,
    })
}

/// MaterialX's value as a node's of type `ty`.
fn value_of(value: &M, ty: &Type) -> Option<Value> {
    Some(match (ty, value) {
        (Type::Float, v) => Value::Number(v.float()?),
        (Type::Color3, M::Numbers(n)) if n.len() == 3 => Value::Color([n[0], n[1], n[2]]),
        (Type::Integer, M::Integer(n)) => Value::Integer(*n),
        (Type::Integer, v) => Value::Integer(v.float()?.round() as i64),
        (Type::Boolean, M::Boolean(b)) => Value::Bool(*b),
        (_, M::Numbers(n)) if n.len() == ty.width() => Value::Numbers(n.clone()),
        _ => return None,
    })
}

/// `values` (a model's inputs by name) through `chain`'s translation
/// graphs: the last model's inputs by name.
fn translate(chain: &[Arc<NodeDef>], values: &HashMap<String, M>) -> HashMap<String, M> {
    let mut values = values.clone();
    for step in chain {
        let Some(graph) = library().implementation(step) else {
            return HashMap::new();
        };
        let interface =
            |name: &str| (values.get(name).cloned()).or_else(|| step.input(name)?.value.clone());
        let mut eval = Eval {
            graph: &graph,
            interface: &interface,
            memo: HashMap::new(),
        };
        let next: HashMap<String, M> = (graph.outputs.iter())
            .filter_map(|(name, binding)| {
                let target = name.strip_suffix("_out")?.to_owned();
                Some((target, eval.binding(binding)?))
            })
            .collect();
        values = next;
    }
    values
}

/// A graph evaluated on its interface's values, each output once.
struct Eval<'g> {
    graph: &'g Graph,
    interface: &'g dyn Fn(&str) -> Option<M>,
    memo: HashMap<(usize, String), Option<M>>,
}

impl Eval<'_> {
    fn binding(&mut self, binding: &Binding) -> Option<M> {
        match binding {
            Binding::Value(v) => Some(v.clone()),
            Binding::Interface(name) => (self.interface)(name),
            Binding::Output { node, output } => self.output(*node, output),
        }
    }

    fn output(&mut self, node: usize, output: &str) -> Option<M> {
        let key = (node, output.to_owned());
        if let Some(known) = self.memo.get(&key) {
            return known.clone();
        }
        let n = &self.graph.nodes[node];
        let mut inputs: HashMap<&str, M> = HashMap::new();
        for (name, binding) in &n.inputs {
            if let Some(v) = self.binding(binding) {
                inputs.insert(name, v);
            }
        }
        let value = n
            .def
            .evaluate_output(output, &|name| inputs.get(name).cloned());
        self.memo.insert(key, value.clone());
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(signature: &str, values: &[(&str, Value)]) -> NetworkSeed {
        let mut seed = NetworkSeed::default();
        let mut node = NewNode::materialx(signature).unwrap().displayed();
        for (name, value) in values {
            node = node.with(*name, value.clone());
        }
        seed.node(node);
        seed
    }

    fn input(seed: &NetworkSeed, name: &str) -> Option<Value> {
        let shown = seed.nodes.iter().find(|n| n.display)?;
        shown.parameters.get(name).cloned()
    }

    #[test]
    fn an_open_pbr_surface_translates_through_materialx_graphs() {
        let seed = surface(
            "ND_open_pbr_surface_surfaceshader",
            &[
                ("base_color", Value::Color([0.8, 0.2, 0.1])),
                ("base_metalness", Value::Number(1.0)),
                ("specular_roughness", Value::Number(0.25)),
            ],
        );
        let (made, said) = preview(&seed);
        let made = made.unwrap();
        assert!(said.is_empty(), "{said:?}");
        let Some(Value::Color(diffuse)) = input(&made, "diffuseColor") else {
            panic!("no diffuse color");
        };
        assert!((diffuse[0] - 0.8).abs() < 1e-9 && (diffuse[2] - 0.1).abs() < 1e-9);
        assert_eq!(input(&made, "metallic"), Some(Value::Number(1.0)));
        assert_eq!(input(&made, "roughness"), Some(Value::Number(0.25)));
    }

    #[test]
    fn a_textured_base_color_carries_over_as_a_usd_uv_texture() {
        use kinogaki_studio_api::geometry::image::ImageSource;
        use kinogaki_studio_api::image_files::image_node;
        let mut seed = surface(
            "ND_standard_surface_surfaceshader",
            &[("base", Value::Number(0.5))],
        );
        let image = seed.node(NewNode::materialx("ND_image_color3").unwrap());
        let file = seed.node(image_node::reading(&ImageSource::path("/maps/wood.png")));
        seed.wire(file, image_node::IMAGE, image, "file");
        seed.wire(image, "out", 0, "base_color");
        let (made, _) = preview(&seed);
        let made = made.unwrap();
        let texture = (made.nodes.iter())
            .position(|n| definition(n).is_some_and(|d| d.node == "UsdUVTexture"))
            .expect("a texture");
        let wire = (made.wires.iter()).find(|w| w.from == texture).unwrap();
        assert_eq!(
            (wire.output.as_str(), wire.input.as_str()),
            ("rgb", "diffuseColor")
        );
        // standard_surface's base scales the color: the texture's scale.
        let scale = &made.nodes[texture].parameters["scale"];
        assert_eq!(*scale, Value::Numbers(vec![0.5, 0.5, 0.5, 1.0]));
        assert!(made.nodes.iter().any(|n| n.node_type == IMAGE_NODE));
    }
}
