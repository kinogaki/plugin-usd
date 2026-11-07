//! UsdLux lights and UsdGeomCameras as the standard light and camera, both
//! ways. A light's sizes and a camera's distances are lengths of the
//! stage's units; a camera's lens and film back are tenths of a stage unit
//! (millimetres in a centimetre stage), the standard's millimetres; a
//! light's color is linear Rec.709 unless the file says otherwise, the
//! standard's the working space.
use crate::usd::scene::{Camera as UsdCamera, Cone, Light as UsdLight, LightKind, Projection};
use crate::usd::sdf::Token;
use kinogaki_render_api::standard::{Emission, Light, Shape};
use kinogaki_render_api::{Camera, Projection as Lens};
use kinogaki_studio_api::Error;
use kinogaki_studio_api::color::{ColorService, LIN_REC709};

/// Millimetres a tenth of a stage unit of `meters_per_unit` is.
fn millimetres(meters_per_unit: f64) -> f64 {
    meters_per_unit * 100.0
}

/// `light` as the standard light: its sizes times `scale` (the project's
/// units per stage unit), its color in the working space, a dome reading
/// `image` (`op:` and its key). A sphere without a cone is a spot of 180°.
pub(crate) fn light(
    light: &UsdLight,
    scale: f64,
    image: Option<String>,
    colors: &dyn ColorService,
) -> Result<Light, Error> {
    let length = |v: f64| (v * scale) as f32;
    let shape = match light.kind {
        LightKind::Rect => Shape::Rect {
            width: length(light.width),
            height: length(light.height),
        },
        LightKind::Disk => Shape::Disk {
            radius: length(light.radius),
        },
        LightKind::Dome => Shape::Dome {
            image: image.map(Into::into),
            ground: None,
        },
        LightKind::Distant => Shape::Distant {
            angle: light.angle as f32,
        },
        LightKind::Sphere => {
            let cone = light.cone.unwrap_or(Cone {
                angle: 180.0,
                softness: 0.0,
            });
            Shape::Spot {
                radius: length(light.radius),
                cone_angle: cone.angle as f32,
                cone_softness: cone.softness as f32,
            }
        }
    };
    let emission = match (&light.kinogaki_emission, light.enable_color_temperature) {
        (Some(key), _) => Emission::of_key(key).unwrap_or_default(),
        (None, true) => Emission::Temperature,
        (None, false) => Emission::Color,
    };
    let mut color = [light.color];
    let space = light.color_space.as_ref().map_or(LIN_REC709, Token::as_str);
    colors.to_working(space, &mut color)?;
    Ok(Light {
        shape,
        emission,
        color: color[0].map(|c| c as f32),
        intensity: light.intensity as f32,
        exposure: light.exposure as f32,
        normalize: light.normalize,
        color_temperature: light.color_temperature as f32,
        ..Light::default()
    })
}

/// The standard light `light` as UsdLux: sizes over `scale` (the project's
/// units per stage unit), its color in linear Rec.709, a dome reading
/// `texture` (an asset of the scene).
pub(crate) fn usd_light(
    light: &Light,
    scale: f64,
    texture: Option<Token>,
    colors: &dyn ColorService,
) -> Result<UsdLight, Error> {
    let length = |v: f32| f64::from(v) / scale;
    let kind = match light.shape {
        Shape::Rect { .. } => LightKind::Rect,
        Shape::Disk { .. } => LightKind::Disk,
        Shape::Dome { .. } => LightKind::Dome,
        Shape::Distant { .. } => LightKind::Distant,
        Shape::Spot { .. } => LightKind::Sphere,
    };
    let mut out = UsdLight::new(kind);
    match &light.shape {
        Shape::Rect { width, height } => {
            (out.width, out.height) = (length(*width), length(*height))
        }
        Shape::Disk { radius } => out.radius = length(*radius),
        Shape::Dome { .. } => out.texture = texture,
        Shape::Distant { angle } => out.angle = f64::from(*angle),
        Shape::Spot {
            radius,
            cone_angle,
            cone_softness,
        } => {
            out.radius = length(*radius);
            out.cone = (*cone_angle < 180.0).then_some(Cone {
                angle: f64::from(*cone_angle),
                softness: f64::from(*cone_softness),
            });
        }
    }
    let mut color = [light.color.map(f64::from)];
    colors.working_to(LIN_REC709, &mut color)?;
    out.color = color[0];
    out.intensity = f64::from(light.intensity);
    out.exposure = f64::from(light.exposure);
    out.normalize = light.normalize;
    out.color_temperature = f64::from(light.color_temperature);
    match &light.emission {
        Emission::Color => {}
        Emission::Temperature => out.enable_color_temperature = true,
        illuminant => out.kinogaki_emission = Some(illuminant.key().to_owned()),
    }
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
            Shape::Rect {
                width: 2.0,
                height: 0.5,
            },
            Shape::Disk { radius: 0.25 },
            Shape::Distant { angle: 0.53 },
            Shape::Spot {
                radius: 0.1,
                cone_angle: 30.0,
                cone_softness: 0.2,
            },
            Shape::Spot {
                radius: 0.1,
                cone_angle: 180.0,
                cone_softness: 0.0,
            },
        ];
        for shape in shapes {
            let standard = Light {
                shape,
                emission: Emission::Temperature,
                color: [1.0, 0.5, 0.25],
                intensity: 40.0,
                exposure: 1.5,
                normalize: true,
                color_temperature: 3200.0,
                ..Light::default()
            };
            // A centimetre stage under a metre project.
            let usd = usd_light(&standard, 100.0, None, &Unconverted).unwrap();
            assert_eq!(light(&usd, 100.0, None, &Unconverted).unwrap(), standard);
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
