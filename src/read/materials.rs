//! A stage's Material prims as the materials a file brings: each one's
//! network of a render context ([`crate::shading`]) as a Shading network
//! of the nodes its shaders name, node graphs and interface inputs looked
//! through, each texture an Image node wired into the input naming it.
//!
//! **Which network a material keeps** (D23): the first it holds of an
//! engine's own (`kinogaki`), MaterialX's (`mtlx`) and USD Preview
//! Surface's; one holding an engine's keeps the next as its standard form,
//! which a host without that engine takes. Shaders no terminal reaches
//! travel with the kept network (D20).
use super::files::Files;
use crate::names::{self, Unique, file_name};
use crate::shading::{
    self, KINOGAKI, LINEAR_REC709, MTLX, NAME_KEY, OPACITY_MODES, UNIVERSAL, terminal,
};
use crate::usd::sdf::{Token, Value as Usd};
use crate::usd::shade::{Material, Port, ShadeKind, Source};
use kinogaki_mtlx::NodeDef;
use kinogaki_studio_api::color::ColorService;
use kinogaki_studio_api::geometry::image::{ImageColor, ImageSource};
use kinogaki_studio_api::image_files::image_node;
use kinogaki_studio_api::network::{NetworkSeed, NewNode};
use kinogaki_studio_api::standard::ImportedMaterial;
use kinogaki_studio_api::{NodeTypeId, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::Path as FilePath;
use std::sync::Arc;

/// The render contexts read, the one a material keeps first.
const CONTEXTS: [&str; 3] = [KINOGAKI, MTLX, UNIVERSAL];

/// How many node graphs and interface inputs a connection is followed
/// through.
const MOST_HOPS: usize = 64;

/// `material` as the material a file brings, keyed by its prim path.
pub(super) fn material(
    material: &Material,
    files: &mut Files<'_>,
    colors: &dyn ColorService,
    warnings: &mut Vec<String>,
) -> ImportedMaterial {
    let key = material.path.to_string();
    let name = match material.custom_data.get(NAME_KEY).and_then(Usd::as_token) {
        Some(name) => name.to_string(),
        None => material.path.name().to_owned(),
    };
    for port in &material.outputs {
        let context = port.name.strip_suffix(":surface");
        if let Some(context) = context.filter(|c| !CONTEXTS.contains(c)) {
            warnings.push(format!("{key}: its {context} network is left out"));
        }
    }
    for document in &material.mtlx {
        warnings.push(format!(
            "{key}: the MaterialX document {} it references is left out",
            file_name(&document.asset)
        ));
    }
    let held: Vec<&str> = (CONTEXTS.into_iter())
        .filter(|c| material.output(&terminal(c, "surface")).is_some())
        .collect();
    let mut reader = Reader {
        material,
        files,
        colors,
        warnings,
        said: BTreeSet::new(),
    };
    let inside = match held.first() {
        Some(context) => reader.network(context, true),
        None => NetworkSeed::default(),
    };
    let standard = match held[..] {
        [KINOGAKI, next, ..] => Some(reader.network(next, false)),
        _ => None,
    };
    ImportedMaterial {
        key,
        name,
        inside,
        standard,
    }
}

/// Where a connection leads once node graphs and interface inputs are
/// looked through.
enum Upstream<'m> {
    /// Output `.1` of shader `.0`.
    Output(usize, &'m str),
    /// The value this port holds.
    Value(&'m Port),
    /// Nothing.
    Nothing,
}

/// What a node of a network is.
enum Kind {
    /// A MaterialX node, of this definition; USD Preview Surface's by its
    /// `info:id` there.
    Mtlx(Arc<NodeDef>, Option<String>),
    /// An engine's own node.
    Engine,
}

impl Kind {
    /// The MaterialX name of its output `output` as USD names it.
    fn output<'a>(&self, output: &'a str) -> &'a str {
        match self {
            Kind::Mtlx(def, Some(_)) => shading::mtlx_output(&def.name, output),
            _ => output,
        }
    }
}

struct Reader<'m, 'r, 'f> {
    material: &'m Material,
    files: &'r mut Files<'f>,
    colors: &'r dyn ColorService,
    warnings: &'r mut Vec<String>,
    /// What was said already, so each is said once a material.
    said: BTreeSet<String>,
}

impl<'m> Reader<'m, '_, '_> {
    fn say(&mut self, what: String) {
        if self.said.insert(what.clone()) {
            self.warnings
                .push(format!("{}: {what}", self.material.path));
        }
    }

    /// Where `source` leads.
    fn upstream(&self, source: &'m Source, hops: usize) -> Upstream<'m> {
        if hops > MOST_HOPS {
            return Upstream::Nothing;
        }
        let named = |ports: &'m [Port]| ports.iter().find(|p| p.name == source.port);
        let held = match (source.node, source.output) {
            (Some(n), true) => {
                let node = &self.material.nodes[n];
                match node.kind {
                    ShadeKind::Shader => return Upstream::Output(n, &source.port),
                    // A node graph's output: what its inside connects to it.
                    ShadeKind::NodeGraph => named(&node.outputs),
                }
            }
            // A node graph's interface input, read from inside it.
            (Some(n), false) => named(&self.material.nodes[n].inputs),
            // The material's own interface input.
            (None, false) => named(&self.material.inputs),
            (None, true) => None,
        };
        held.map_or(Upstream::Nothing, |port| self.port(port, hops + 1))
    }

    /// What `port` reads: what its strongest connection leads to, else its
    /// own value.
    fn port(&self, port: &'m Port, hops: usize) -> Upstream<'m> {
        match port.sources.first() {
            Some(source) => self.upstream(source, hops),
            None if port.value.is_some() => Upstream::Value(port),
            None => Upstream::Nothing,
        }
    }

    /// The shaders `ports`' connections reach, upstream all the way.
    fn reached(&self, ports: impl IntoIterator<Item = &'m Port>) -> BTreeSet<usize> {
        let mut reached = BTreeSet::new();
        let mut open: Vec<&Port> = ports.into_iter().collect();
        while let Some(port) = open.pop() {
            if let Upstream::Output(n, _) = self.port(port, 0)
                && reached.insert(n)
            {
                open.extend(&self.material.nodes[n].inputs);
            }
        }
        reached
    }

    /// The shader `context`'s terminal `which` shows, and its output.
    fn shown(&self, context: &str, which: &str) -> Option<(usize, &'m str)> {
        let port = self.material.output(&terminal(context, which))?;
        match self.port(port, 0) {
            Upstream::Output(n, output) => Some((n, output)),
            _ => None,
        }
    }

    /// The network of render context `context`: the shaders its terminals
    /// reach, and, when `unreached`, those no terminal of any context
    /// reaches.
    fn network(&mut self, context: &str, unreached: bool) -> NetworkSeed {
        let terminals = ["surface", "displacement"].map(|t| terminal(context, t));
        let own =
            (self.material.outputs.iter()).filter(|p| terminals.contains(&p.name.to_string()));
        let mut members = self.reached(own);
        if unreached {
            let any = self.reached(&self.material.outputs);
            let loose = (0..self.material.nodes.len())
                .filter(|n| self.material.nodes[*n].kind == ShadeKind::Shader && !any.contains(n));
            members.extend(loose);
        }
        let mut seed = NetworkSeed::default();
        let mut names = Unique::default();
        let mut made: HashMap<usize, (usize, Kind)> = HashMap::new();
        for &n in &members {
            if let Some((node, kind)) = self.node(n, context) {
                let name = names.take(self.material.nodes[n].path.name());
                made.insert(n, (seed.node(node.named(name)), kind));
            }
        }
        let mut images = Vec::new();
        for &n in &members {
            let Some((at, kind)) = made.get(&n) else {
                continue;
            };
            for port in &self.material.nodes[n].inputs {
                match self.port(port, 0) {
                    Upstream::Output(from, output) => {
                        if let Some((source, source_kind)) = made.get(&from) {
                            seed.wire(*source, source_kind.output(output), *at, &port.name);
                        }
                    }
                    Upstream::Value(holder) => {
                        self.value(&mut seed, (*at, kind), n, &port.name, holder, &mut images)
                    }
                    Upstream::Nothing => {}
                }
            }
        }
        let shown = |which| {
            let (n, output) = self.shown(context, which)?;
            let (at, kind) = made.get(&n)?;
            Some((*at, kind.output(output).to_owned()))
        };
        match (shown("surface"), shown("displacement")) {
            (Some((surface, out)), Some((displacement, out_d))) if context != UNIVERSAL => {
                let wrap = NewNode::materialx("ND_surfacematerial").expect("MaterialX's");
                let wrap = seed.node(wrap.named(names.take("Material")).displayed());
                seed.wire(surface, &out, wrap, "surfaceshader");
                seed.wire(displacement, &out_d, wrap, "displacementshader");
            }
            (Some((surface, _)), _) => seed.nodes[surface].display = true,
            _ => {}
        }
        seed
    }

    /// Shader `n` of `context` as a node to make, and what it is; none,
    /// said, for one no installed library or engine could name.
    fn node(&mut self, n: usize, context: &str) -> Option<(NewNode, Kind)> {
        let shader = &self.material.nodes[n];
        let Some(id) = shader.id() else {
            self.say(format!(
                "{} names no shader by id; left out",
                shader.path.name()
            ));
            return None;
        };
        let library = kinogaki_mtlx::library();
        let signature = match context {
            UNIVERSAL => shading::preview_signature(id),
            _ => library.def(id).map(|def| def.name.as_str()),
        };
        if let Some(signature) = signature {
            let def = library.def(signature)?.clone();
            let preview = (context == UNIVERSAL).then(|| id.to_string());
            return Some((NewNode::materialx(signature)?, Kind::Mtlx(def, preview)));
        }
        match NodeTypeId::parse(id).filter(|_| context == KINOGAKI) {
            Some(ty) => Some((NewNode::new(ty), Kind::Engine)),
            None => {
                self.say(format!(
                    "its shader {id} is no node Kinogaki knows; left out"
                ));
                None
            }
        }
    }

    /// Input `input` of node `at` (shader `n`) holding `holder`'s value: a
    /// file is an Image node wired in, one per file and color in a network.
    fn value(
        &mut self,
        seed: &mut NetworkSeed,
        (at, kind): (usize, &Kind),
        n: usize,
        input: &str,
        holder: &Port,
        images: &mut Vec<(ImageSource, usize)>,
    ) {
        let Some(value) = &holder.value else { return };
        if let Usd::Asset(path) = value {
            let source = ImageSource::new(self.files.file(path), self.texture_color(n, holder));
            let image = match images.iter().find(|(s, _)| *s == source) {
                Some((_, image)) => *image,
                None => {
                    let stem = FilePath::new(source.name()).file_stem();
                    let name = stem.map_or("Image".into(), |s| s.to_string_lossy());
                    let image = seed.node(image_node::reading(&source).named(name));
                    images.push((source, image));
                    image
                }
            };
            seed.wire(image, image_node::IMAGE, at, input);
            return;
        }
        let mut color4 = false;
        let read = match kind {
            Kind::Mtlx(def, preview) => {
                if preview.as_deref() == Some("UsdUVTexture") && input == "sourceColorSpace" {
                    return;
                }
                let Some(port) = def.input(input) else {
                    self.say(format!("{} has no input {input}; left out", def.node));
                    return;
                };
                color4 = port.ty == kinogaki_mtlx::Type::Color4;
                let read = match (preview.as_deref(), input) {
                    (Some("UsdPreviewSurface"), "opacityMode") => (shading::text(value))
                        .and_then(|t| OPACITY_MODES.iter().position(|m| *m == t))
                        .map(|k| Value::Integer(k as i64)),
                    (Some("UsdUVTexture"), "wrapS" | "wrapT") => (shading::text(value))
                        .map(|t| Value::Choice(Arc::from(shading::mtlx_wrap(t)))),
                    _ => shading::mtlx_value(value, &port.ty, !port.enumeration.is_empty()),
                };
                read.map(|v| primvar_named(&def.node, input, v))
            }
            Kind::Engine => shading::engine_value(value, &holder.type_name),
        };
        let Some(mut read) = read else {
            let ty = &holder.type_name;
            self.say(format!("{input} holds a {ty} it can't; left out"));
            return;
        };
        self.in_working(&mut read, color4, holder.color_space.as_ref());
        seed.nodes[at].parameters.insert(input.to_owned(), read);
    }

    /// How a texture's colors read: USD UV Texture's `sourceColorSpace`,
    /// else the file input's `colorSpace`, else as the file says.
    fn texture_color(&self, n: usize, file: &Port) -> ImageColor {
        let source = (self.material.nodes[n].input("sourceColorSpace"))
            .and_then(|p| p.value.as_ref())
            .and_then(shading::text);
        match (source, file.color_space.as_deref()) {
            (Some("raw"), _) => ImageColor::Data,
            (Some("sRGB"), _) => ImageColor::Space(Arc::from("srgb_texture")),
            (_, Some("raw" | "none" | "data")) => ImageColor::Data,
            (_, Some(space)) => ImageColor::Space(Arc::from(space)),
            (_, None) => ImageColor::Auto,
        }
    }

    /// A color `value` (a color, or a color4's numbers) authored in `space`
    /// (linear Rec.709 when the file doesn't say) in the working space.
    fn in_working(&mut self, value: &mut Value, color4: bool, space: Option<&Token>) {
        let rgb: &mut [f64] = match value {
            Value::Color(c) => c,
            Value::Numbers(n) if color4 => &mut n[..3],
            _ => return,
        };
        let from = space.map_or(LINEAR_REC709, Token::as_str);
        let mut held = [[rgb[0], rgb[1], rgb[2]]];
        match self.colors.to_working(from, &mut held) {
            Ok(()) => rgb.copy_from_slice(&held[0]),
            Err(e) => self.say(format!("colors in {from} kept as they are: {e}")),
        }
    }
}

/// `value` of input `input` of MaterialX node `node`, a primvar's name in
/// the attribute it is read as (`st` is `uv`).
fn primvar_named(node: &str, input: &str, value: Value) -> Value {
    let names = matches!(
        (node, input),
        ("UsdPrimvarReader", "varname") | ("geompropvalue" | "geompropvalueuniform", "geomprop")
    );
    match value {
        Value::Text(t) if names => Value::Text(names::attribute(&t).to_owned()),
        other => other,
    }
}
