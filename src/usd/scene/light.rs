//! UsdLux lights as plain data: area lights (RectLight, DiskLight), the
//! DomeLight, the DistantLight and the SphereLight, with their shared
//! inputs (intensity, exposure, color, color temperature, normalize) and
//! sizes, ShapingAPI's (a cone, focus, an IES profile) on any of them,
//! Kinogaki's `kinogaki:emission` (a spectrum UsdLux cannot name), and a
//! DomeLight's image file. An input left unauthored holds its schema's
//! fallback (a DistantLight's intensity is 50,000). Inputs read under their
//! `inputs:` names and, for files written before UsdLux took that prefix,
//! under their bare names. Other UsdLux kinds are not read (see
//! [`LightKind::unread`]).

use super::author::Author;
use super::values::{Time, color_space, reals, value_at};
use crate::usd::sdf::{LayerData, Path, Token, Value, field};

/// Which UsdLux light a prim is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    /// `RectLight`: a rectangle in its XY plane, emitting down -Z.
    Rect,
    /// `DiskLight`: a disk in its XY plane, emitting down -Z.
    Disk,
    /// `DomeLight` (and `DomeLight_1`): light from every direction.
    Dome,
    /// `DistantLight`: parallel light down its -Z from infinitely far.
    Distant,
    /// `SphereLight`: a sphere at its origin.
    Sphere,
}

impl LightKind {
    /// Every kind read.
    pub const ALL: [LightKind; 5] = [
        LightKind::Rect,
        LightKind::Disk,
        LightKind::Dome,
        LightKind::Distant,
        LightKind::Sphere,
    ];

    /// The prim type name written.
    pub fn type_name(self) -> &'static str {
        match self {
            LightKind::Rect => "RectLight",
            LightKind::Disk => "DiskLight",
            LightKind::Dome => "DomeLight",
            LightKind::Distant => "DistantLight",
            LightKind::Sphere => "SphereLight",
        }
    }

    /// The kind a prim type name reads as.
    pub fn of_type(name: &str) -> Option<LightKind> {
        Some(match name {
            "RectLight" => LightKind::Rect,
            "DiskLight" => LightKind::Disk,
            "DomeLight" | "DomeLight_1" => LightKind::Dome,
            "DistantLight" => LightKind::Distant,
            "SphereLight" => LightKind::Sphere,
            _ => return None,
        })
    }

    /// UsdLux light types that are not read: they are left out with a
    /// warning.
    pub fn unread(name: &str) -> bool {
        matches!(name, "CylinderLight" | "PortalLight" | "GeometryLight")
    }
}

/// ShapingAPI's cone: light leaves within `angle` of the light's -Z.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cone {
    /// `inputs:shaping:cone:angle`, the half-angle in degrees (90 by
    /// default: no cone).
    pub angle: f64,
    /// `inputs:shaping:cone:softness`, 0 to 1: the share of the cone the
    /// light fades over.
    pub softness: f64,
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
    /// metadata, else the nearest ColorSpaceAPI's); none: the file doesn't
    /// say (linear Rec.709). Not written: a writer converts to linear
    /// Rec.709.
    pub color_space: Option<Token>,
    /// `inputs:enableColorTemperature`.
    pub enable_color_temperature: bool,
    /// `inputs:colorTemperature`, in kelvin.
    pub color_temperature: f64,
    /// `inputs:normalize`: the power does not change with the size.
    pub normalize: bool,
    /// `kinogaki:emission` (a token): the emission spectrum Kinogaki gave
    /// the light when it is not UsdLux's color or temperature (a CIE
    /// illuminant's name, `D65`); `inputs:color` then holds its RGB.
    pub kinogaki_emission: Option<String>,
    /// `inputs:width` (Rect).
    pub width: f64,
    /// `inputs:height` (Rect).
    pub height: f64,
    /// `inputs:radius` (Disk, Sphere).
    pub radius: f64,
    /// `inputs:angle` (Distant): its size across, in degrees.
    pub angle: f64,
    /// ShapingAPI's cone, when the light is shaped: it applies ShapingAPI
    /// or authors its inputs.
    pub cone: Option<Cone>,
    /// `inputs:shaping:focus` (0: none): how much the light gathers toward
    /// its -Z.
    pub focus: f64,
    /// `inputs:shaping:focusTint`: the color its light takes off its axis
    /// as it focuses.
    pub focus_tint: [f64; 3],
    /// `inputs:shaping:ies:file`: its IES profile, as authored.
    pub ies: Option<Token>,
    /// `inputs:texture:file` (Dome): its image, the path of one of the
    /// scene's assets.
    pub texture: Option<Token>,
    /// `inputs:texture:format` (Dome): `automatic`, `latlong`, `mirroredBall`,
    /// `angular` or `cubeMapVerticalCross`.
    pub texture_format: Token,
}

impl Light {
    /// A light of `kind` with its schema's fallback values: LightAPI's and
    /// ShapingAPI's, with the ones its kind overrides (a DistantLight's
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
            kinogaki_emission: None,
            width: 1.0,
            height: 1.0,
            radius: 0.5,
            angle: 0.53,
            cone: None,
            focus: 0.0,
            focus_tint: [0.0; 3],
            ies: None,
            texture: None,
            texture_format: Token::new("automatic"),
        }
    }
}

/// The value of input `name`, under `inputs:` or its bare name.
fn input(layer: &LayerData, source: &Path, name: &str, time: Time) -> Option<Value> {
    value_at(layer, source, &format!("inputs:{name}"), time)
        .or_else(|| value_at(layer, source, name, time))
}

/// Read the light of `kind` at `source`; the dome's texture as authored.
pub(super) fn read(layer: &LayerData, source: &Path, kind: LightKind, time: Time) -> Light {
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
    light.intensity = number("intensity", light.intensity);
    light.exposure = number("exposure", light.exposure);
    if let Some((color, 3)) = input(layer, source, "color", time).and_then(|v| reals(&v))
        && color.iter().all(|v| v.is_finite())
    {
        light.color = [color[0], color[1], color[2]];
    }
    light.color_space =
        color_space(layer, source, "inputs:color").or_else(|| color_space(layer, source, "color"));
    light.enable_color_temperature = flag("enableColorTemperature", light.enable_color_temperature);
    light.color_temperature = number("colorTemperature", light.color_temperature);
    light.normalize = flag("normalize", light.normalize);
    light.kinogaki_emission = value_at(layer, source, "kinogaki:emission", time)
        .and_then(|v| v.as_token().map(|t| t.as_str().to_owned()));
    light.width = number("width", light.width);
    light.height = number("height", light.height);
    light.radius = number("radius", light.radius);
    light.angle = number("angle", light.angle);
    if kind == LightKind::Dome {
        if let Some(Value::Asset(file)) = input(layer, source, "texture:file", time) {
            light.texture = Some(file);
        }
        if let Some(Value::Token(format)) = input(layer, source, "texture:format", time) {
            light.texture_format = format;
        }
    }
    if applies(layer, source, "ShapingAPI") || authors(layer, source, "inputs:shaping:") {
        light.cone = Some(Cone {
            angle: number("shaping:cone:angle", 90.0),
            softness: number("shaping:cone:softness", 0.0),
        });
        light.focus = number("shaping:focus", light.focus);
        if let Some((tint, 3)) =
            input(layer, source, "shaping:focusTint", time).and_then(|v| reals(&v))
            && tint.iter().all(|v| v.is_finite())
        {
            light.focus_tint = [tint[0], tint[1], tint[2]];
        }
        if let Some(Value::Asset(file)) = input(layer, source, "shaping:ies:file", time)
            && !file.is_empty()
        {
            light.ies = Some(file);
        }
    }
    light
}

/// Whether the prim at `source` applies the API schema `schema`.
fn applies(layer: &LayerData, source: &Path, schema: &str) -> bool {
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
    author.prim(path, light.kind.type_name());
    let float = |author: &mut Author, name: &str, value: f64| {
        author.attribute(
            path,
            &format!("inputs:{name}"),
            "float",
            Value::Float(value as f32),
            false,
        );
    };
    let flag = |author: &mut Author, name: &str, value: bool| {
        author.attribute(
            path,
            &format!("inputs:{name}"),
            "bool",
            Value::Bool(value),
            false,
        );
    };
    float(author, "intensity", light.intensity);
    float(author, "exposure", light.exposure);
    let [r, g, b] = light.color;
    author.attribute(
        path,
        "inputs:color",
        "color3f",
        Value::Vec3f([r as f32, g as f32, b as f32]),
        false,
    );
    flag(
        author,
        "enableColorTemperature",
        light.enable_color_temperature,
    );
    float(author, "colorTemperature", light.color_temperature);
    flag(author, "normalize", light.normalize);
    if let Some(emission) = &light.kinogaki_emission {
        author.attribute(
            path,
            "kinogaki:emission",
            "token",
            Value::Token(emission.as_str().into()),
            true,
        );
    }
    match light.kind {
        LightKind::Rect => {
            float(author, "width", light.width);
            float(author, "height", light.height);
        }
        LightKind::Disk | LightKind::Sphere => float(author, "radius", light.radius),
        LightKind::Distant => float(author, "angle", light.angle),
        LightKind::Dome => {
            if let Some(file) = &light.texture {
                author.attribute(
                    path,
                    "inputs:texture:file",
                    "asset",
                    Value::Asset(file.clone()),
                    false,
                );
            }
            if light.texture_format.as_str() != "automatic" {
                let format = Value::Token(light.texture_format.clone());
                author.attribute(path, "inputs:texture:format", "token", format, false);
            }
        }
    }
    if let Some(cone) = light.cone {
        author.api_schema(path, "ShapingAPI");
        float(author, "shaping:cone:angle", cone.angle);
        float(author, "shaping:cone:softness", cone.softness);
        if light.focus != 0.0 {
            float(author, "shaping:focus", light.focus);
            let tint = Value::Vec3f(light.focus_tint.map(|c| c as f32));
            author.attribute(path, "inputs:shaping:focusTint", "color3f", tint, false);
        }
        if let Some(file) = &light.ies {
            let file = Value::Asset(file.clone());
            author.attribute(path, "inputs:shaping:ies:file", "asset", file, false);
        }
    }
}
