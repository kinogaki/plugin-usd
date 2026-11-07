//! A USD Preview Surface made from a MaterialX network, for applications
//! reading no other render context (AR Quick Look, most viewers).
//!
//! The shown surface is translated as every format translates a material
//! into its own model (`MaterialNetwork::translated`: MaterialX's and
//! Kinogaki's translation graphs, folded). What is left feeding the USD
//! Preview Surface is made of its own nodes: a texture (MaterialX's
//! `image`, glTF's, a USD UV Texture) carries over as a USD UV Texture of
//! the same file, and the arithmetic the translation left between it and
//! the input (a base color scaled by its weight) is its `scale` and `bias`,
//! found by evaluating that arithmetic with the texture at zero and at one.
//! A normal map carries over into `normal`. Anything else feeding the
//! surface (a procedural, arithmetic over two textures) leaves its input at
//! its default, said.
use super::materials::definition;
use crate::names;
use kinogaki_mtlx::{Type, Value as M};
use kinogaki_render_api::MaterialNetwork;
use kinogaki_render_api::material::mtlx::SIGNATURE;
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
    let translated = MaterialNetwork::try_from(seed)
        .map_err(|e| said.push(format!("no USD Preview Surface: {}", e.message())))
        .ok()?
        .translated("UsdPreviewSurface");
    let Some(translated) = translated else {
        said.push("its surface translates to no USD Preview Surface".into());
        return None;
    };
    let folded = NetworkSeed::from(&translated);
    let shown = folded.nodes.iter().position(|n| n.display)?;
    let surface = match definition(&folded.nodes[shown]) {
        Some(def) if def.node == "surfacematerial" => wired(&folded, shown, "surfaceshader")?.0,
        _ => shown,
    };
    let target = definition(&folded.nodes[surface]).filter(|d| d.name == SURFACE)?;
    let mut out = NetworkSeed::default();
    let at = out.node(NewNode::materialx(SURFACE)?.named("Preview").displayed());
    // Its values that differ from USD Preview Surface's defaults.
    for (name, value) in &folded.nodes[surface].parameters {
        let default = target.input(name).and_then(|p| p.value.as_ref());
        if name != SIGNATURE && mtlx_of(value).as_ref() != default {
            out.nodes[at].parameters.insert(name.clone(), value.clone());
        }
    }
    let mut made = Made::default();
    for wire in folded.wires.iter().filter(|w| w.to == surface) {
        let input = wire.input.as_str();
        let Some(port) = target.input(input) else {
            continue;
        };
        if input == "normal" {
            let from = &folded.nodes[wire.from];
            let map =
                definition(from).filter(|d| d.node == "normalmap" || d.node == "gltf_normalmap");
            let texture = map.and_then(|d| {
                texture_into(
                    &folded,
                    wire.from,
                    if d.node == "normalmap" { "in" } else { "file" },
                )
            });
            match texture {
                Some(texture) => {
                    let node = made.texture(&folded, &mut out, &texture, (&[-1.0; 3], &[2.0; 3]));
                    out.wire(node, "rgb", at, "normal");
                }
                None => said.push(
                    "its normal reads no normal map USD Preview Surface holds; left out".into(),
                ),
            }
            continue;
        }
        let Some((texture, chain)) = carried(&folded, wire.from, &wire.output) else {
            said.push(format!(
                "{input}'s network has no USD Preview Surface form; its default stands"
            ));
            continue;
        };
        let width = if texture.channel.is_some() { 1 } else { 3 };
        let (Some(low), Some(high)) = (evaluate(&chain, width, 0.0), evaluate(&chain, width, 1.0))
        else {
            said.push(format!(
                "{input}'s texture can't be carried over; its default stands"
            ));
            continue;
        };
        let channel = match (port.ty.width(), texture.channel, low.len()) {
            (3, None, 3) => "rgb",
            (1, Some(c), 1) => ["r", "g", "b", "a"][c],
            _ => {
                said.push(format!(
                    "{input}'s texture doesn't fit it; its default stands"
                ));
                continue;
            }
        };
        let scale: Vec<f64> = high.iter().zip(&low).map(|(h, l)| h - l).collect();
        let node = made.texture(&folded, &mut out, &texture, (&low, &scale));
        out.wire(node, channel, at, input);
        out.nodes[at].parameters.remove(input);
    }
    if textures_approximated(&made) {
        said.push("glTF texture transforms are approximated".into());
    }
    Some(out)
}

/// A node of the arithmetic between a texture and a surface's input: the
/// node, the input the texture's value reaches it by, and the output it
/// goes on by.
struct Step<'s> {
    node: &'s NewNode,
    input: &'s str,
    output: &'s str,
}

/// The texture output `output` of node `from` is, through the arithmetic
/// nodes between: the texture, and those nodes from it outward.
fn carried<'s>(
    seed: &'s NetworkSeed,
    from: usize,
    output: &'s str,
) -> Option<(Texture, Vec<Step<'s>>)> {
    let mut chain = Vec::new();
    let (mut at, mut out) = (from, output);
    loop {
        if let Some(texture) = texture(seed, at, out) {
            chain.reverse();
            return Some((texture, chain));
        }
        let def = definition(&seed.nodes[at])?;
        if !def.evaluates() || chain.len() > 16 {
            return None;
        }
        // Arithmetic over one wired value, the rest values of its own.
        let mut wires = seed.wires.iter().filter(|w| w.to == at);
        let wire = wires.next()?;
        if wires.next().is_some() {
            return None;
        }
        chain.push(Step {
            node: &seed.nodes[at],
            input: &wire.input,
            output: out,
        });
        (at, out) = (wire.from, &wire.output);
    }
}

/// What `chain` makes of a texture value of `width` numbers each `k`.
fn evaluate(chain: &[Step<'_>], width: usize, k: f64) -> Option<Vec<f64>> {
    let mut value = M::Numbers(vec![k; width]);
    for step in chain {
        let def = definition(step.node)?;
        let held = |name: &str| match name == step.input {
            true => Some(value.clone()),
            false => step.node.parameters.get(name).and_then(mtlx_of),
        };
        value = def.evaluate_output(step.output, &held)?;
    }
    value.numbers().map(<[f64]>::to_vec)
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

/// Whether a texture carried over was placed by a glTF transform.
fn textures_approximated(made: &Made) -> bool {
    made.gltf_transform
}

/// The nodes made so far, each once: texture coordinates by set and
/// placement, textures by what they read.
#[derive(Default)]
struct Made {
    images: HashMap<usize, usize>,
    readers: HashMap<String, usize>,
    placed: Vec<((String, [u64; 5]), usize)>,
    /// Whether a texture was placed by a glTF transform.
    gltf_transform: bool,
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
        self.gltf_transform |= texture.gltf_transform;
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
