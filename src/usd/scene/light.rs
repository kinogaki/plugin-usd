//! UsdLux lights as plain data: every intrinsic kind (RectLight, DiskLight,
//! SphereLight, CylinderLight, DistantLight, DomeLight and DomeLight_1, a
//! dome's portals, PortalLight, the deprecated GeometryLight) and a Mesh
//! that is a light (MeshLightAPI or LightAPI applied: [`mesh_light`]),
//! LightAPI's inputs (intensity, exposure, color, color temperature,
//! normalize, diffuse, specular), ShapingAPI's ([`Shaping`]), ShadowAPI's
//! ([`Shadow`]), whether the camera sees the light ([`CAMERA`] and the
//! renderers' own attributes, [`CAMERA_READ`]), Kinogaki's
//! `kinogaki:emission` (a spectrum UsdLux cannot name) and
//! `kinogaki:groundColor` (a dome's ground), and the files a light reads (a
//! dome's or rect's `texture:file`, an IES profile). An input left
//! unauthored holds its schema's fallback (a DistantLight's intensity is
//! 50,000). Inputs read under their `inputs:` names and, for files written
//! before UsdLux took that prefix, under their bare names. The prims a
//! light names (a dome's portals, a GeometryLight's geometry) are kept as
//! paths. Other UsdLux kinds are not read (see [`LightKind::unread`]).

use super::author::Author;
use super::values::{Time, color_space, reals, value_at};
use crate::usd::sdf::{LayerData, Path, Token, Value, field};

/// Kinogaki's attribute saying whether the camera sees a light (UsdLux has
/// none yet; this is the AOUSD forum's `visibility:camera` under our
/// namespace).
pub const CAMERA: &str = "kinogaki:visibility:camera";

/// The attributes saying whether the camera sees a light, in the order
/// they are read, the first authored one deciding: ours, the proposed
/// UsdLux one, Karma's, RenderMan's, Omniverse's and Arnold's.
pub const CAMERA_READ: [&str; 6] = [
    CAMERA,
    "visibility:camera",
    "inputs:karma:light:renderlightgeo",
    "primvars:ri:attributes:visibility:camera",
    "visibleInPrimaryRay",
    "primvars:arnold:camera",
];

/// Which UsdLux light a prim is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    /// `RectLight`: a rectangle in its XY plane, emitting down -Z.
    Rect,
    /// `DiskLight`: a disk in its XY plane, emitting down -Z.
    Disk,
    /// `SphereLight`: a sphere at its origin.
    Sphere,
    /// `CylinderLight`: a tube along its X axis.
    Cylinder,
    /// `DistantLight`: parallel light down its -Z from infinitely far.
    Distant,
    /// `DomeLight` and `DomeLight_1`: light from every direction.
    Dome,
    /// `PortalLight`: an opening in its XY plane a dome lights through,
    /// toward its -Z.
    Portal,
    /// A mesh that emits: the deprecated `GeometryLight` (naming its
    /// geometry), or a Mesh applying MeshLightAPI or LightAPI.
    Geometry,
}

impl LightKind {
    /// Every kind read.
    pub const ALL: [LightKind; 8] = [
        LightKind::Rect,
        LightKind::Disk,
        LightKind::Sphere,
        LightKind::Cylinder,
        LightKind::Distant,
        LightKind::Dome,
        LightKind::Portal,
        LightKind::Geometry,
    ];

    /// The prim type name a light of the kind is written as (a dome whose
    /// pole isn't its +Y is a `DomeLight_1`: [`Light::type_name`]).
    pub fn type_name(self) -> &'static str {
        match self {
            LightKind::Rect => "RectLight",
            LightKind::Disk => "DiskLight",
            LightKind::Sphere => "SphereLight",
            LightKind::Cylinder => "CylinderLight",
            LightKind::Distant => "DistantLight",
            LightKind::Dome => "DomeLight",
            LightKind::Portal => "PortalLight",
            LightKind::Geometry => "GeometryLight",
        }
    }

    /// The kind a prim type name reads as.
    pub fn of_type(name: &str) -> Option<LightKind> {
        Some(match name {
            "RectLight" => LightKind::Rect,
            "DiskLight" => LightKind::Disk,
            "SphereLight" => LightKind::Sphere,
            "CylinderLight" => LightKind::Cylinder,
            "DistantLight" => LightKind::Distant,
            "DomeLight" | "DomeLight_1" => LightKind::Dome,
            "PortalLight" => LightKind::Portal,
            "GeometryLight" => LightKind::Geometry,
            _ => return None,
        })
    }

    /// Whether Studio's light of the kind takes ShapingAPI: a rect, disk,
    /// sphere or cylinder (a distant light's, a dome's and a portal's light
    /// leave no surface; a geometry light's shaping isn't held).
    pub fn shaped(self) -> bool {
        matches!(
            self,
            LightKind::Rect | LightKind::Disk | LightKind::Sphere | LightKind::Cylinder
        )
    }

    /// Whether Studio's light of the kind takes ShadowAPI (a portal casts
    /// none, a geometry light's shadow controls aren't held).
    pub fn shadowed(self) -> bool {
        !matches!(self, LightKind::Portal | LightKind::Geometry)
    }

    /// UsdLux light types that are not read, and why: they are left out
    /// with a warning.
    pub fn unread(name: &str) -> Option<&'static str> {
        match name {
            "PluginLight" | "PluginLightFilter" => {
                Some("names a renderer's own shader, which no engine has")
            }
            "LightFilter" => Some("is a renderer's own light filter"),
            _ => None,
        }
    }
}

/// A mesh light's `light:materialSyncMode` where MeshLightAPI applies.
pub const MESH_SYNC: &str = "materialGlowTintsLight";
/// LightAPI's `light:materialSyncMode` (a GeometryLight's, or a Mesh
/// applying LightAPI alone).
pub const LIGHT_SYNC: &str = "noMaterialResponse";

/// ShapingAPI's inputs, as authored or their fallbacks.
#[derive(Clone, Debug, PartialEq)]
pub struct Shaping {
    /// `inputs:shaping:cone:angle`, the half-angle in degrees (90 by
    /// default).
    pub cone_angle: f64,
    /// `inputs:shaping:cone:softness`, 0 to 1: the share of the cone the
    /// light fades over.
    pub cone_softness: f64,
    /// `inputs:shaping:focus` (0: none).
    pub focus: f64,
    /// `inputs:shaping:focusTint`, linear.
    pub focus_tint: [f64; 3],
    /// `inputs:shaping:ies:file`: its IES profile, the path of one of the
    /// scene's assets.
    pub ies: Option<Token>,
    /// `inputs:shaping:ies:angleScale`.
    pub ies_angle_scale: f64,
    /// `inputs:shaping:ies:normalize`.
    pub ies_normalize: bool,
}

impl Default for Shaping {
    fn default() -> Shaping {
        Shaping {
            cone_angle: 90.0,
            cone_softness: 0.0,
            focus: 0.0,
            focus_tint: [0.0; 3],
            ies: None,
            ies_angle_scale: 0.0,
            ies_normalize: false,
        }
    }
}

/// ShadowAPI's inputs, as authored or their fallbacks.
#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    /// `inputs:shadow:enable`.
    pub enable: bool,
    /// `inputs:shadow:color`, linear.
    pub color: [f64; 3],
    /// `inputs:shadow:distance` (−1: no limit).
    pub distance: f64,
    /// `inputs:shadow:falloff` (−1: none).
    pub falloff: f64,
    /// `inputs:shadow:falloffGamma`.
    pub falloff_gamma: f64,
}

impl Default for Shadow {
    fn default() -> Shadow {
        Shadow {
            enable: true,
            color: [0.0; 3],
            distance: -1.0,
            falloff: -1.0,
            falloff_gamma: 1.0,
        }
    }
}

/// One light prim. Sizes that do not apply to its kind keep their
/// fallbacks and are not written.
#[derive(Clone, Debug, PartialEq)]
pub struct Light {
    /// The light's kind.
    pub kind: LightKind,
    /// `inputs:intensity`.
    pub intensity: f64,
    /// `inputs:exposure`: the intensity is scaled by 2 to this power.
    pub exposure: f64,
    /// `inputs:color`, linear.
    pub color: [f64; 3],
    /// The color space `inputs:color` is authored in (`colorSpace`
    /// metadata, else the nearest ColorSpaceAPI's), which its other colors
    /// share; none: the file doesn't say (linear Rec.709). Not written: a
    /// writer converts to linear Rec.709.
    pub color_space: Option<Token>,
    /// `inputs:enableColorTemperature`.
    pub enable_color_temperature: bool,
    /// `inputs:colorTemperature`, in kelvin.
    pub color_temperature: f64,
    /// `inputs:normalize`: the power does not change with the size.
    pub normalize: bool,
    /// `inputs:diffuse`: its effect on diffuse lobes is multiplied by it.
    pub diffuse: f64,
    /// `inputs:specular`: its effect on specular lobes is multiplied by it.
    pub specular: f64,
    /// `kinogaki:emission` (a token): the emission spectrum Kinogaki gave
    /// the light when it is not UsdLux's color or temperature (a CIE
    /// illuminant's name, `D65`); `inputs:color` then holds its RGB.
    pub kinogaki_emission: Option<String>,
    /// `inputs:width` (Rect).
    pub width: f64,
    /// `inputs:height` (Rect).
    pub height: f64,
    /// `inputs:radius` (Disk, Sphere, Cylinder).
    pub radius: f64,
    /// `inputs:length` (Cylinder).
    pub length: f64,
    /// `inputs:angle` (Distant): its size across, in degrees.
    pub angle: f64,
    /// `treatAsPoint` (Sphere).
    pub treat_as_point: bool,
    /// `treatAsLine` (Cylinder).
    pub treat_as_line: bool,
    /// `inputs:texture:file` (Dome, Rect): its image, the path of one of
    /// the scene's assets.
    pub texture: Option<Token>,
    /// `inputs:texture:format` (Dome): `automatic`, `latlong`, `mirroredBall`,
    /// `angular` or `cubeMapVerticalCross`.
    pub texture_format: Token,
    /// A dome's pole: none for a `DomeLight` (its +Y), a `DomeLight_1`'s
    /// `poleAxis` (`scene`, `Y` or `Z`).
    pub pole_axis: Option<Token>,
    /// `kinogaki:groundColor` (Dome): the color it shines below the horizon,
    /// linear.
    pub ground: Option<[f64; 3]>,
    /// ShapingAPI's inputs, when the light is shaped: it applies ShapingAPI
    /// or authors its inputs.
    pub shaping: Option<Shaping>,
    /// ShadowAPI's inputs (written when off their fallbacks).
    pub shadow: Shadow,
    /// Whether the camera sees it, as the first of [`CAMERA_READ`] authored
    /// says; none: no attribute does.
    pub camera: Option<bool>,
    /// `light:materialSyncMode` (Geometry): what the mesh shows and whether
    /// its material's glow tints the light.
    pub material_sync: Token,
    /// `portals` (Dome): the portals it lights through, as authored.
    pub portals: Vec<Path>,
    /// `geometry` (a GeometryLight): the prim that emits, as authored.
    pub geometry: Option<Path>,
}

impl Light {
    /// A light of `kind` with its schema's fallback values: LightAPI's and
    /// ShadowAPI's, with the ones its kind overrides (a DistantLight's
    /// intensity of 50,000, to approximate sunlight).
    pub fn new(kind: LightKind) -> Light {
        Light {
            kind,
            intensity: match kind {
                LightKind::Distant => 50_000.0,
                _ => 1.0,
            },
            exposure: 0.0,
            color: [1.0; 3],
            color_space: None,
            enable_color_temperature: false,
            color_temperature: 6500.0,
            normalize: false,
            diffuse: 1.0,
            specular: 1.0,
            kinogaki_emission: None,
            width: 1.0,
            height: 1.0,
            radius: 0.5,
            length: 1.0,
            angle: 0.53,
            treat_as_point: false,
            treat_as_line: false,
            texture: None,
            texture_format: Token::new("automatic"),
            pole_axis: None,
            ground: None,
            shaping: None,
            shadow: Shadow::default(),
            camera: None,
            material_sync: Token::new(LIGHT_SYNC),
            portals: Vec::new(),
            geometry: None,
        }
    }

    /// The prim type name it is written as: a dome whose pole isn't its
    /// own +Y is a `DomeLight_1`.
    pub fn type_name(&self) -> &'static str {
        match (self.kind, &self.pole_axis) {
            (LightKind::Dome, Some(pole)) if pole.as_str() != "Y" => "DomeLight_1",
            (kind, _) => kind.type_name(),
        }
    }
}

/// The value of input `name`, under `inputs:` or its bare name.
fn input(layer: &LayerData, source: &Path, name: &str, time: Time) -> Option<Value> {
    value_at(layer, source, &format!("inputs:{name}"), time)
        .or_else(|| value_at(layer, source, name, time))
}

/// Three finite numbers.
fn color(value: Option<Value>) -> Option<[f64; 3]> {
    match value.as_ref().and_then(reals) {
        Some((c, 3)) if c.iter().all(|v| v.is_finite()) => Some([c[0], c[1], c[2]]),
        _ => None,
    }
}

/// Whether LightAPI's light or shadow links narrow what the light at `path`
/// lights or shadows, which Studio doesn't hold (the owner, 2026-11-09):
/// an exclude, the root left out (`includeRoot`, on by default), or a
/// membership expression.
pub(super) fn links_narrowed(layer: &LayerData, path: &Path, time: Time) -> bool {
    ["lightLink", "shadowLink"].iter().any(|link| {
        let name = |part: &str| format!("collection:{link}:{part}");
        let excludes = layer.get(
            &path.property(name("excludes").as_str()),
            field::TARGET_PATHS,
        );
        let excluded = matches!(excludes, Some(Value::PathListOp(l)) if !l.applied().is_empty());
        let root = value_at(layer, path, &name("includeRoot"), time).and_then(|v| v.as_bool());
        let expression = value_at(layer, path, &name("membershipExpression"), time);
        excluded || root == Some(false) || expression.is_some()
    })
}

/// Read the light of `kind` at `source` (a prim of type `type_name`); its
/// files as authored.
pub(super) fn read(
    layer: &LayerData,
    source: &Path,
    (kind, type_name): (LightKind, &str),
    time: Time,
) -> Light {
    let mut light = Light::new(kind);
    let number = |name: &str, fallback: f64| {
        input(layer, source, name, time)
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite())
            .unwrap_or(fallback)
    };
    let flag = |name: &str, fallback: bool| {
        input(layer, source, name, time)
            .and_then(|v| v.as_bool())
            .unwrap_or(fallback)
    };
    let asset = |name: &str| match input(layer, source, name, time) {
        Some(Value::Asset(file)) if !file.is_empty() => Some(file),
        _ => None,
    };
    light.intensity = number("intensity", light.intensity);
    light.exposure = number("exposure", light.exposure);
    light.color = color(input(layer, source, "color", time)).unwrap_or(light.color);
    light.color_space =
        color_space(layer, source, "inputs:color").or_else(|| color_space(layer, source, "color"));
    light.enable_color_temperature = flag("enableColorTemperature", light.enable_color_temperature);
    light.color_temperature = number("colorTemperature", light.color_temperature);
    light.normalize = flag("normalize", light.normalize);
    light.diffuse = number("diffuse", light.diffuse);
    light.specular = number("specular", light.specular);
    light.kinogaki_emission = value_at(layer, source, "kinogaki:emission", time)
        .and_then(|v| v.as_token().map(|t| t.as_str().to_owned()));
    light.width = number("width", light.width);
    light.height = number("height", light.height);
    light.radius = number("radius", light.radius);
    light.length = number("length", light.length);
    light.angle = number("angle", light.angle);
    let plain = |name: &str| value_at(layer, source, name, time).and_then(|v| v.as_bool());
    light.treat_as_point = plain("treatAsPoint").unwrap_or(false);
    light.treat_as_line = plain("treatAsLine").unwrap_or(false);
    if matches!(kind, LightKind::Dome | LightKind::Rect) {
        light.texture = asset("texture:file");
    }
    if kind == LightKind::Dome {
        if let Some(Value::Token(format)) = input(layer, source, "texture:format", time) {
            light.texture_format = format;
        }
        light.pole_axis =
            (type_name == "DomeLight_1").then(|| match value_at(layer, source, "poleAxis", time) {
                Some(Value::Token(pole)) => pole,
                _ => Token::new("scene"),
            });
        light.ground = color(value_at(layer, source, "kinogaki:groundColor", time));
        light.portals = targets(layer, source, "portals");
    }
    if kind == LightKind::Geometry {
        light.geometry = targets(layer, source, "geometry").into_iter().next();
        if let Some(Value::Token(sync)) = value_at(layer, source, "light:materialSyncMode", time) {
            light.material_sync = sync;
        }
    }
    if applies(layer, source, "ShapingAPI") || authors(layer, source, "inputs:shaping:") {
        let fallback = Shaping::default();
        light.shaping = Some(Shaping {
            cone_angle: number("shaping:cone:angle", fallback.cone_angle),
            cone_softness: number("shaping:cone:softness", fallback.cone_softness),
            focus: number("shaping:focus", fallback.focus),
            focus_tint: color(input(layer, source, "shaping:focusTint", time))
                .unwrap_or(fallback.focus_tint),
            ies: asset("shaping:ies:file"),
            ies_angle_scale: number("shaping:ies:angleScale", fallback.ies_angle_scale),
            ies_normalize: flag("shaping:ies:normalize", fallback.ies_normalize),
        });
    }
    let fallback = Shadow::default();
    light.shadow = Shadow {
        enable: flag("shadow:enable", fallback.enable),
        color: color(input(layer, source, "shadow:color", time)).unwrap_or(fallback.color),
        distance: number("shadow:distance", fallback.distance),
        falloff: number("shadow:falloff", fallback.falloff),
        falloff_gamma: number("shadow:falloffGamma", fallback.falloff_gamma),
    };
    light.camera = (CAMERA_READ.iter())
        .find_map(|name| value_at(layer, source, name, time))
        .and_then(|v| v.as_bool().or_else(|| v.as_f64().map(|x| x > 0.0)));
    light
}

/// The targets of relationship `name` of the prim at `source`.
fn targets(layer: &LayerData, source: &Path, name: &str) -> Vec<Path> {
    match layer.get(&source.property(name), field::TARGET_PATHS) {
        Some(Value::PathListOp(list)) => list.applied(),
        _ => Vec::new(),
    }
}

/// The light the Mesh at `source` is, when it applies MeshLightAPI or
/// LightAPI: LightAPI's inputs as authored, its sync mode MeshLightAPI's
/// fallback ([`MESH_SYNC`]) or LightAPI's ([`LIGHT_SYNC`]).
pub(super) fn mesh_light(layer: &LayerData, source: &Path, time: Time) -> Option<Light> {
    let mesh = applies(layer, source, "MeshLightAPI");
    if !mesh && !applies(layer, source, "LightAPI") {
        return None;
    }
    let mut light = read(layer, source, (LightKind::Geometry, "Mesh"), time);
    if mesh && value_at(layer, source, "light:materialSyncMode", time).is_none() {
        light.material_sync = Token::new(MESH_SYNC);
    }
    Some(light)
}

/// Whether the prim at `source` applies the API schema `schema`.
pub(super) fn applies(layer: &LayerData, source: &Path, schema: &str) -> bool {
    match layer.get(source, "apiSchemas") {
        Some(Value::TokenListOp(list)) => list.applied().iter().any(|t| t.as_str() == schema),
        _ => false,
    }
}

/// Whether the prim at `source` has a property named `prefix…`.
fn authors(layer: &LayerData, source: &Path, prefix: &str) -> bool {
    let properties = layer
        .spec(source)
        .map_or(&[][..], |s| s.tokens(field::PROPERTIES));
    properties.iter().any(|p| p.starts_with(prefix))
}

/// Author `light` as its UsdLux prim (ancestors made as Xforms).
pub(super) fn write(author: &mut Author, path: &Path, light: &Light) {
    author.prim(path, light.type_name());
    let float = |author: &mut Author, name: &str, value: f64| {
        let name = format!("inputs:{name}");
        author.attribute(path, &name, "float", Value::Float(value as f32), false);
    };
    let flag = |author: &mut Author, name: &str, value: bool| {
        author.attribute(path, name, "bool", Value::Bool(value), false);
    };
    let color = |author: &mut Author, name: &str, [r, g, b]: [f64; 3]| {
        let value = Value::Vec3f([r as f32, g as f32, b as f32]);
        author.attribute(path, name, "color3f", value, false);
    };
    let asset = |author: &mut Author, name: &str, file: &Token| {
        author.attribute(path, name, "asset", Value::Asset(file.clone()), false);
    };
    if light.kind != LightKind::Portal {
        inputs(author, path, light);
    }
    match light.kind {
        LightKind::Rect | LightKind::Portal => {
            float(author, "width", light.width);
            float(author, "height", light.height);
        }
        LightKind::Disk => float(author, "radius", light.radius),
        LightKind::Sphere => {
            float(author, "radius", light.radius);
            flag(author, "treatAsPoint", light.treat_as_point);
        }
        LightKind::Cylinder => {
            float(author, "length", light.length);
            float(author, "radius", light.radius);
            flag(author, "treatAsLine", light.treat_as_line);
        }
        LightKind::Distant => float(author, "angle", light.angle),
        LightKind::Dome => {
            if light.texture_format.as_str() != "automatic" {
                let format = Value::Token(light.texture_format.clone());
                author.attribute(path, "inputs:texture:format", "token", format, false);
            }
            if let Some(pole) = (light.pole_axis.as_ref()).filter(|p| p.as_str() != "Y") {
                let pole = Value::Token(pole.clone());
                author.attribute(path, "poleAxis", "token", pole, true);
            }
            if let Some([r, g, b]) = light.ground {
                let value = Value::Vec3f([r as f32, g as f32, b as f32]);
                own(author, path, "kinogaki:groundColor", "color3f", value);
            }
            if !light.portals.is_empty() {
                author.relationship(path, "portals", light.portals.clone());
            }
        }
        LightKind::Geometry => sync_mode(author, path, light, LIGHT_SYNC),
    }
    if let Some(file) = &light.texture {
        asset(author, "inputs:texture:file", file);
    }
    if let Some(shaping) = &light.shaping {
        author.api_schema(path, "ShapingAPI");
        float(author, "shaping:cone:angle", shaping.cone_angle);
        float(author, "shaping:cone:softness", shaping.cone_softness);
        if shaping.focus != 0.0 {
            float(author, "shaping:focus", shaping.focus);
            color(author, "inputs:shaping:focusTint", shaping.focus_tint);
        }
        if let Some(file) = &shaping.ies {
            asset(author, "inputs:shaping:ies:file", file);
            float(author, "shaping:ies:angleScale", shaping.ies_angle_scale);
            flag(
                author,
                "inputs:shaping:ies:normalize",
                shaping.ies_normalize,
            );
        }
    }
    if light.shadow != Shadow::default() {
        let shadow = &light.shadow;
        author.api_schema(path, "ShadowAPI");
        flag(author, "inputs:shadow:enable", shadow.enable);
        color(author, "inputs:shadow:color", shadow.color);
        float(author, "shadow:distance", shadow.distance);
        float(author, "shadow:falloff", shadow.falloff);
        float(author, "shadow:falloffGamma", shadow.falloff_gamma);
    }
    if light.kind.shadowed() {
        let seen = Value::Bool(light.camera.unwrap_or(false));
        own(author, path, CAMERA, "bool", seen);
    }
}

/// Author `light` (a geometry light) on the Mesh prim at `path` as
/// MeshLightAPI: LightAPI's inputs, its sync mode where it isn't
/// MeshLightAPI's fallback.
pub(super) fn write_mesh_light(author: &mut Author, path: &Path, light: &Light) {
    author.api_schema(path, "MeshLightAPI");
    inputs(author, path, light);
    sync_mode(author, path, light, MESH_SYNC);
}

/// `light:materialSyncMode` where it isn't `fallback`.
fn sync_mode(author: &mut Author, path: &Path, light: &Light, fallback: &str) {
    if light.material_sync.as_str() != fallback {
        let sync = Value::Token(light.material_sync.clone());
        author.attribute(path, "light:materialSyncMode", "token", sync, true);
    }
}

/// Kinogaki's attribute `name` of `type_name`, which no schema declares:
/// custom.
fn own(author: &mut Author, path: &Path, name: &str, type_name: &str, value: Value) {
    let at = author.attribute(path, name, type_name, value, type_name == "token");
    author.property_metadata(&at, "custom", Value::Bool(true));
}

/// LightAPI's inputs of `light` on the prim at `path`: intensity,
/// exposure, color and temperature, Normalize (not a dome's), Diffuse,
/// Specular, and Kinogaki's emission.
fn inputs(author: &mut Author, path: &Path, light: &Light) {
    let float = |author: &mut Author, name: &str, value: f64| {
        let name = format!("inputs:{name}");
        author.attribute(path, &name, "float", Value::Float(value as f32), false);
    };
    let flag = |author: &mut Author, name: &str, value: bool| {
        author.attribute(path, name, "bool", Value::Bool(value), false);
    };
    float(author, "intensity", light.intensity);
    float(author, "exposure", light.exposure);
    let [r, g, b] = light.color;
    let value = Value::Vec3f([r as f32, g as f32, b as f32]);
    author.attribute(path, "inputs:color", "color3f", value, false);
    flag(
        author,
        "inputs:enableColorTemperature",
        light.enable_color_temperature,
    );
    float(author, "colorTemperature", light.color_temperature);
    if light.kind != LightKind::Dome {
        flag(author, "inputs:normalize", light.normalize);
    }
    float(author, "diffuse", light.diffuse);
    float(author, "specular", light.specular);
    if let Some(emission) = &light.kinogaki_emission {
        let emission = Value::Token(emission.as_str().into());
        own(author, path, "kinogaki:emission", "token", emission);
    }
}
