//! UsdGeomCamera prims as plain data: the lens and film back as authored
//! (tenths of a stage unit, as the schema defines them), clipping and focus
//! in stage units. Callers convert to their own units.

use super::author::Author;
use super::values::{Time, reals, token_at, value_at};
use crate::usd::sdf::{LayerData, Path, Token, Value};

/// How a camera projects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Projection {
    /// `perspective` (the fallback).
    #[default]
    Perspective,
    /// `orthographic`: parallel rays across the apertures.
    Orthographic,
}

/// One Camera prim. It looks down its -Z with +Y up.
#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    /// `projection`.
    pub projection: Projection,
    /// `focalLength`, in tenths of a stage unit (millimetres in a
    /// centimetre stage).
    pub focal_length: f64,
    /// `horizontalAperture`, in tenths of a stage unit.
    pub horizontal_aperture: f64,
    /// `verticalAperture`, in tenths of a stage unit.
    pub vertical_aperture: f64,
    /// `horizontalApertureOffset`: the film back's shift, in tenths of a
    /// stage unit.
    pub horizontal_aperture_offset: f64,
    /// `verticalApertureOffset`, in tenths of a stage unit.
    pub vertical_aperture_offset: f64,
    /// `clippingRange`: near and far, in stage units.
    pub clipping_range: [f64; 2],
    /// `focusDistance`, in stage units.
    pub focus_distance: f64,
    /// `fStop`; 0 is a pinhole (no depth of field).
    pub f_stop: f64,
    /// `shutter:open` and `shutter:close`, in frames relative to the frame
    /// (equal: no motion blur).
    pub shutter: [f64; 2],
}

impl Default for Camera {
    /// The schema's fallback values.
    fn default() -> Camera {
        Camera {
            projection: Projection::Perspective,
            focal_length: 50.0,
            horizontal_aperture: 20.955,
            vertical_aperture: 15.2908,
            horizontal_aperture_offset: 0.0,
            vertical_aperture_offset: 0.0,
            clipping_range: [1.0, 1_000_000.0],
            focus_distance: 0.0,
            f_stop: 0.0,
            shutter: [0.0, 0.0],
        }
    }
}

/// Read the camera at `source`.
pub(super) fn read(layer: &LayerData, source: &Path, time: Time) -> Camera {
    let mut camera = Camera::default();
    let number = |name: &str, fallback: f64| {
        value_at(layer, source, name, time)
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite())
            .unwrap_or(fallback)
    };
    if token_at(layer, source, "projection", "perspective").as_str() == "orthographic" {
        camera.projection = Projection::Orthographic;
    }
    camera.focal_length = number("focalLength", camera.focal_length);
    camera.horizontal_aperture = number("horizontalAperture", camera.horizontal_aperture);
    camera.vertical_aperture = number("verticalAperture", camera.vertical_aperture);
    camera.horizontal_aperture_offset = number(
        "horizontalApertureOffset",
        camera.horizontal_aperture_offset,
    );
    camera.vertical_aperture_offset =
        number("verticalApertureOffset", camera.vertical_aperture_offset);
    camera.focus_distance = number("focusDistance", camera.focus_distance);
    camera.f_stop = number("fStop", camera.f_stop);
    camera.shutter = [
        number("shutter:open", camera.shutter[0]),
        number("shutter:close", camera.shutter[1]),
    ];
    if let Some((range, 2)) = value_at(layer, source, "clippingRange", time).and_then(|v| reals(&v))
        && range.iter().all(|v| v.is_finite())
    {
        camera.clipping_range = [range[0], range[1]];
    }
    camera
}

/// Author `camera` as a Camera prim (ancestors made as Xforms).
pub(super) fn write(author: &mut Author, path: &Path, camera: &Camera) {
    author.prim(path, "Camera");
    let projection = match camera.projection {
        Projection::Perspective => "perspective",
        Projection::Orthographic => "orthographic",
    };
    author.attribute(
        path,
        "projection",
        "token",
        Value::Token(Token::new(projection)),
        false,
    );
    for (name, value) in [
        ("focalLength", camera.focal_length),
        ("horizontalAperture", camera.horizontal_aperture),
        ("verticalAperture", camera.vertical_aperture),
        (
            "horizontalApertureOffset",
            camera.horizontal_aperture_offset,
        ),
        ("verticalApertureOffset", camera.vertical_aperture_offset),
        ("focusDistance", camera.focus_distance),
        ("fStop", camera.f_stop),
    ] {
        author.attribute(path, name, "float", Value::Float(value as f32), false);
    }
    for (name, value) in [
        ("shutter:open", camera.shutter[0]),
        ("shutter:close", camera.shutter[1]),
    ] {
        author.attribute(path, name, "double", Value::Double(value), false);
    }
    let [near, far] = camera.clipping_range;
    author.attribute(
        path,
        "clippingRange",
        "float2",
        Value::Vec2f([near as f32, far as f32]),
        false,
    );
}
