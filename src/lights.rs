//! UsdLux lights and UsdGeomCameras as the standard light and camera, both
//! ways. A light's sizes, its shadows' distances and a camera's distances
//! are lengths of the stage's units; a camera's lens and film back are
//! tenths of a stage unit (millimetres in a centimetre stage), the
//! standard's millimetres; a light's colors are linear Rec.709 unless the
//! file says otherwise, the standard's the working space. Every intrinsic
//! UsdLux kind is a standard one; ShapingAPI on a distant light or a dome
//! (whose light leaves no surface) is left out, said ([`left_out`]).
use crate::names::file_name;
use crate::usd::scene::{
    Camera as UsdCamera, Light as UsdLight, LightKind, Projection, Shadow as UsdShadow,
    Shaping as UsdShaping,
};
use crate::usd::sdf::Token;
use kinogaki_render_api::standard::{
    DomeFormat, Emission, Light, LightShape, Pole, Shadow, Shaping,
};
use kinogaki_render_api::{Camera, Projection as Lens};
use kinogaki_studio_api::Error;
use kinogaki_studio_api::color::{ColorService, LIN_REC709};
use kinogaki_studio_api::value::ImageRef;

/// Millimetres a tenth of a stage unit of `meters_per_unit` is.
fn millimetres(meters_per_unit: f64) -> f64 {
    meters_per_unit * 100.0
}

/// The least size a light holds: a point's or line's radius of 0 is made
/// it (a zero-size light of another shape is no light, and left out).
const LEAST: f32 = 1e-6;

/// The files a light reads, as the scene holds them: a dome's or rect's
/// image, an IES profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Files<T> {
    /// `inputs:texture:file`.
    pub texture: Option<T>,
    /// `inputs:shaping:ies:file`.
    pub ies: Option<T>,
}

impl<T> Default for Files<T> {
    /// None.
    fn default() -> Self {
        Files {
            texture: None,
            ies: None,
        }
    }
}

/// `light` as the standard light: its sizes and shadow distances times
/// `scale` (the project's units per stage unit), its colors in the working
/// space, its files `files`.
pub(crate) fn light(
    light: &UsdLight,
    scale: f64,
    files: Files<ImageRef>,
    colors: &dyn ColorService,
) -> Result<Light, Error> {
    let shaping = (light.shaping.as_ref()).filter(|_| light.kind.shaped());
    let mut held = [
        light.color,
        shaping.map_or([0.0; 3], |s| s.focus_tint),
        light.shadow.color,
        light.ground.unwrap_or([0.0; 3]),
    ];
    let space = light.color_space.as_ref().map_or(LIN_REC709, Token::as_str);
    colors.to_working(space, &mut held)?;
    let [color, tint, shadow_color, ground] = held.map(|c| c.map(|v| v as f32));
    let size = |v: f64| (v * scale) as f32;
    let hinted = |v: f64| size(v).max(LEAST);
    let shape = match light.kind {
        LightKind::Rect => LightShape::Rect {
            width: size(light.width),
            height: size(light.height),
            texture: files.texture.clone(),
        },
        LightKind::Disk => LightShape::Disk {
            radius: size(light.radius),
        },
        LightKind::Sphere => LightShape::Sphere {
            radius: match light.treat_as_point {
                true => hinted(light.radius),
                false => size(light.radius),
            },
            point: light.treat_as_point,
        },
        LightKind::Cylinder => LightShape::Cylinder {
            length: size(light.length),
            radius: match light.treat_as_line {
                true => hinted(light.radius),
                false => size(light.radius),
            },
            line: light.treat_as_line,
        },
        LightKind::Dome => LightShape::Dome {
            image: files.texture.clone(),
            format: DomeFormat::of_key(&light.texture_format).unwrap_or_default(),
            pole: match &light.pole_axis {
                None => Pole::Y,
                Some(pole) => Pole::of_key(pole).unwrap_or_default(),
            },
            ground: light.ground.map(|_| ground),
        },
        LightKind::Distant => LightShape::Distant {
            angle: light.angle.clamp(0.0, 180.0) as f32,
        },
    };
    let emission = match (&light.kinogaki_emission, light.enable_color_temperature) {
        (Some(key), _) => Emission::of_key(key).unwrap_or_default(),
        (None, true) => Emission::Temperature,
        (None, false) => Emission::Color,
    };
    let reach = |v: f64| if v < 0.0 { -1.0 } else { (v * scale) as f32 };
    Ok(Light {
        shape,
        emission,
        color,
        intensity: light.intensity as f32,
        exposure: light.exposure as f32,
        normalize: light.normalize && light.kind != LightKind::Dome,
        color_temperature: light.color_temperature.clamp(1000.0, 40_000.0) as f32,
        diffuse: light.diffuse.max(0.0) as f32,
        specular: light.specular.max(0.0) as f32,
        shaping: shaping.map(|s| Shaping {
            cone_angle: s.cone_angle.clamp(0.0, 180.0) as f32,
            cone_softness: s.cone_softness.clamp(0.0, 1.0) as f32,
            // UsdLux ignores a focus below 0.
            focus: s.focus.max(0.0) as f32,
            focus_tint: tint,
            ies: files.ies.clone(),
            ies_angle_scale: s.ies_angle_scale as f32,
            ies_normalize: s.ies_normalize,
        }),
        shadow: Shadow {
            enable: light.shadow.enable,
            color: shadow_color,
            distance: reach(light.shadow.distance),
            falloff: reach(light.shadow.falloff),
            falloff_gamma: light.shadow.falloff_gamma.max(0.0) as f32,
        },
        camera: light.camera.unwrap_or(false),
        ..Light::default()
    })
}

/// What of `light` the standard light can't hold, in words, which is left
/// out: ShapingAPI on a distant light or a dome (their light leaves no
/// surface a cone or a profile could shape); none when nothing is.
pub(crate) fn left_out(light: &UsdLight) -> Option<String> {
    let shaping = light.shaping.as_ref().filter(|_| !light.kind.shaped())?;
    let profile = (shaping.ies.as_ref()).map_or(String::new(), |file| {
        format!(" (IES profile {})", file_name(file))
    });
    Some(format!(
        "ShapingAPI on a {}{profile} is left out",
        light.kind.type_name()
    ))
}

/// The standard light `light` as UsdLux: sizes and shadow distances over
/// `scale` (the project's units per stage unit), its colors in linear
/// Rec.709, its files the scene's assets `files`.
pub(crate) fn usd_light(
    light: &Light,
    scale: f64,
    files: Files<Token>,
    colors: &dyn ColorService,
) -> Result<UsdLight, Error> {
    let length = |v: f32| f64::from(v) / scale;
    let kind = match light.shape {
        LightShape::Rect { .. } => LightKind::Rect,
        LightShape::Disk { .. } => LightKind::Disk,
        LightShape::Sphere { .. } => LightKind::Sphere,
        LightShape::Cylinder { .. } => LightKind::Cylinder,
        LightShape::Dome { .. } => LightKind::Dome,
        LightShape::Distant { .. } => LightKind::Distant,
    };
    let mut out = UsdLight::new(kind);
    let mut held = [
        light.color,
        light.shaping.as_ref().map_or([0.0; 3], |s| s.focus_tint),
        light.shadow.color,
        [0.0; 3],
    ]
    .map(|c| c.map(f64::from));
    match &light.shape {
        LightShape::Rect { width, height, .. } => {
            (out.width, out.height) = (length(*width), length(*height));
            out.texture = files.texture.clone();
        }
        LightShape::Disk { radius } => out.radius = length(*radius),
        LightShape::Sphere { radius, point } => {
            out.radius = length(*radius);
            out.treat_as_point = *point;
        }
        LightShape::Cylinder {
            length: long,
            radius,
            line,
        } => {
            (out.length, out.radius) = (length(*long), length(*radius));
            out.treat_as_line = *line;
        }
        LightShape::Dome {
            format,
            pole,
            ground,
            ..
        } => {
            out.texture = files.texture.clone();
            out.texture_format = Token::new(format.key());
            out.pole_axis = (*pole != Pole::Y).then(|| Token::new(pole.key()));
            if let Some(g) = ground {
                held[3] = g.map(f64::from);
                out.ground = Some(held[3]);
            }
        }
        LightShape::Distant { angle } => out.angle = f64::from(*angle),
    }
    colors.working_to(LIN_REC709, &mut held)?;
    let [color, tint, shadow_color, ground] = held;
    out.ground = out.ground.map(|_| ground);
    out.color = color;
    out.intensity = f64::from(light.intensity);
    out.exposure = f64::from(light.exposure);
    out.normalize = light.normalize;
    out.color_temperature = f64::from(light.color_temperature);
    out.diffuse = f64::from(light.diffuse);
    out.specular = f64::from(light.specular);
    match &light.emission {
        Emission::Color => {}
        Emission::Temperature => out.enable_color_temperature = true,
        illuminant => out.kinogaki_emission = Some(illuminant.key().to_owned()),
    }
    out.shaping = light.shaping.as_ref().map(|s| UsdShaping {
        cone_angle: f64::from(s.cone_angle),
        cone_softness: f64::from(s.cone_softness),
        focus: f64::from(s.focus),
        focus_tint: tint,
        ies: files.ies.clone(),
        ies_angle_scale: f64::from(s.ies_angle_scale),
        ies_normalize: s.ies_normalize,
    });
    let reach = |v: f32| if v < 0.0 { -1.0 } else { length(v) };
    out.shadow = UsdShadow {
        enable: light.shadow.enable,
        color: shadow_color,
        distance: reach(light.shadow.distance),
        falloff: reach(light.shadow.falloff),
        falloff_gamma: f64::from(light.shadow.falloff_gamma),
    };
    out.camera = Some(light.camera);
    Ok(out)
}

/// `camera` as the standard camera's lens: its lens and film back in
/// millimetres (a stage of `meters_per_unit`), its distances times `scale`.
pub(crate) fn camera(camera: &UsdCamera, meters_per_unit: f64, scale: f64) -> Camera {
    let mm = |v: f64| (v * millimetres(meters_per_unit)) as f32;
    let length = |v: f64| (v * scale) as f32;
    Camera {
        projection: match camera.projection {
            Projection::Perspective => Lens::Perspective,
            Projection::Orthographic => Lens::Orthographic,
        },
        focal_length: mm(camera.focal_length),
        horizontal_aperture: mm(camera.horizontal_aperture),
        vertical_aperture: mm(camera.vertical_aperture),
        horizontal_aperture_offset: mm(camera.horizontal_aperture_offset),
        vertical_aperture_offset: mm(camera.vertical_aperture_offset),
        clipping_range: camera.clipping_range.map(length),
        f_stop: camera.f_stop as f32,
        focus_distance: length(camera.focus_distance),
        shutter: camera.shutter.map(|s| s as f32),
        ..Camera::default()
    }
}

/// The standard camera's lens as a UsdGeomCamera of a stage of
/// `meters_per_unit`, distances over `scale`.
pub(crate) fn usd_camera(camera: &Camera, meters_per_unit: f64, scale: f64) -> UsdCamera {
    let tenths = |v: f32| f64::from(v) / millimetres(meters_per_unit);
    let length = |v: f32| f64::from(v) / scale;
    UsdCamera {
        projection: match camera.projection {
            Lens::Perspective => Projection::Perspective,
            Lens::Orthographic => Projection::Orthographic,
        },
        focal_length: tenths(camera.focal_length),
        horizontal_aperture: tenths(camera.horizontal_aperture),
        vertical_aperture: tenths(camera.vertical_aperture),
        horizontal_aperture_offset: tenths(camera.horizontal_aperture_offset),
        vertical_aperture_offset: tenths(camera.vertical_aperture_offset),
        clipping_range: camera.clipping_range.map(length),
        focus_distance: length(camera.focus_distance),
        f_stop: f64::from(camera.f_stop),
        shutter: camera.shutter.map(f64::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kinogaki_studio_api_testing::Unconverted;

    #[test]
    fn lights_read_and_write_back_in_any_units() {
        let shapes = [
            LightShape::Rect {
                width: 2.0,
                height: 0.5,
                texture: None,
            },
            LightShape::Disk { radius: 0.25 },
            LightShape::Sphere {
                radius: 0.1,
                point: true,
            },
            LightShape::Cylinder {
                length: 0.8,
                radius: 0.015,
                line: false,
            },
            LightShape::Distant { angle: 0.53 },
        ];
        for shape in shapes {
            let shaped = !matches!(shape, LightShape::Distant { .. });
            let standard = Light {
                shape,
                emission: Emission::Temperature,
                color: [1.0, 0.5, 0.25],
                intensity: 40.0,
                exposure: 1.5,
                normalize: true,
                color_temperature: 3200.0,
                diffuse: 0.5,
                specular: 2.0,
                shaping: shaped.then(|| Shaping {
                    cone_angle: 30.0,
                    cone_softness: 0.2,
                    focus: 2.0,
                    focus_tint: [0.0, 0.0, 1.0],
                    ..Shaping::default()
                }),
                shadow: Shadow {
                    color: [0.5, 0.0, 0.0],
                    distance: 2.0,
                    falloff: 0.5,
                    ..Shadow::default()
                },
                camera: true,
                ..Light::default()
            };
            // A centimetre stage under a metre project.
            let usd = usd_light(&standard, 100.0, Files::default(), &Unconverted).unwrap();
            let back = light(&usd, 100.0, Files::default(), &Unconverted).unwrap();
            assert_eq!(back, standard);
        }
        // A dome's pole: its own +Y is a DomeLight, the scene's a DomeLight_1.
        for (pole, written) in [(Pole::Y, "DomeLight"), (Pole::Scene, "DomeLight_1")] {
            let dome = Light {
                shape: LightShape::Dome {
                    image: None,
                    format: DomeFormat::Angular,
                    pole,
                    ground: Some([0.1, 0.2, 0.3]),
                },
                ..Light::default()
            };
            let usd = usd_light(&dome, 1.0, Files::default(), &Unconverted).unwrap();
            assert_eq!(usd.type_name(), written);
            assert_eq!(
                light(&usd, 1.0, Files::default(), &Unconverted).unwrap(),
                dome
            );
        }
    }

    #[test]
    fn a_lens_is_millimetres_whatever_the_stage_unit() {
        let lens = Camera {
            focal_length: 35.0,
            clipping_range: [0.1, 500.0],
            focus_distance: 4.0,
            f_stop: 2.8,
            ..Camera::default()
        };
        for meters in [0.01, 1.0] {
            let usd = usd_camera(&lens, meters, 1.0);
            assert_eq!(camera(&usd, meters, 1.0), lens);
        }
        // In a centimetre stage, a tenth of a unit is a millimetre.
        assert_eq!(usd_camera(&lens, 0.01, 1.0).focal_length, 35.0);
    }
}
