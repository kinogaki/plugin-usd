//! UsdLux lights read as the standard light and written back as UsdLux
//! writes them: every intrinsic kind, LightAPI's inputs, ShapingAPI,
//! ShadowAPI and whether the camera sees each, from minimal layers. An
//! unauthored input holds its schema's fallback for the light's kind; what
//! no standard light holds is left out, said once a light.
use crate::support::{Scratch, item, metres, read_file, write_file};
use kinogaki_render_api::standard::{DomeFormat, Light, LightShape, Pole, Shadow, Shaping};
use kinogaki_studio_api::{FileFormat, FileItem, Imported, Input, Purpose, Reading, standard};
use kinogaki_studio_api_testing::Unconverted;
use kinogaki_usd::Usd;

fn read(layer: &str) -> Imported {
    let reading = Reading::new(Purpose::Objects, metres(), &Unconverted);
    let input = Input::memory("lights.usda", layer.as_bytes());
    Usd.read(&input, &Usd.options(), &reading).unwrap()
}

fn light(read: &Imported, name: &str) -> Light {
    let held = item(read, name).1.expect("a light");
    standard::light().get(held).unwrap().clone()
}

/// A metre stage of `prims`.
fn stage(prims: &str) -> String {
    format!("#usda 1.0\n(\n    metersPerUnit = 1\n)\n{prims}")
}

#[test]
fn unauthored_intensities_are_each_kinds_fallback() {
    let read = read(&stage(
        r#"def DistantLight "Sun"
{
    float inputs:angle = 1
}
def SphereLight "Bulb"
{
}
def RectLight "Panel"
{
}
"#,
    ));
    // A DistantLight's fallback is 50,000, "to approximate sunlight".
    assert_eq!(light(&read, "Sun").intensity, 50_000.0);
    assert_eq!(light(&read, "Bulb").intensity, 1.0);
    let panel = light(&read, "Panel");
    assert_eq!(panel.intensity, 1.0);
    assert_eq!(
        panel.shape,
        LightShape::Rect {
            width: 1.0,
            height: 1.0,
            texture: None,
        }
    );
}

#[test]
fn every_intrinsic_kind_reads_as_its_standard_shape() {
    let read = read(&stage(
        r#"def RectLight "Rect"
{
    float inputs:width = 2
    float inputs:height = 0.5
}
def DiskLight "Disk"
{
    float inputs:radius = 0.25
}
def SphereLight "Point"
{
    float inputs:radius = 0.1
    bool treatAsPoint = 1
}
def CylinderLight "Tube"
{
    float inputs:length = 0.8
    float inputs:radius = 0.015
}
def CylinderLight "Line"
{
    bool treatAsLine = 1
}
def DomeLight "Old"
{
}
def DomeLight_1 "New"
{
    uniform token poleAxis = "Z"
    token inputs:texture:format = "mirroredBall"
}
def DomeLight_1 "Scene"
{
}
def DomeLight "Cross"
{
    token inputs:texture:format = "cubeMapVerticalCross"
}
"#,
    ));
    assert_eq!(
        light(&read, "Rect").shape,
        LightShape::Rect {
            width: 2.0,
            height: 0.5,
            texture: None
        }
    );
    assert_eq!(
        light(&read, "Disk").shape,
        LightShape::Disk { radius: 0.25 }
    );
    assert_eq!(
        light(&read, "Point").shape,
        LightShape::Sphere {
            radius: 0.1,
            point: true
        }
    );
    assert_eq!(
        light(&read, "Tube").shape,
        LightShape::Cylinder {
            length: 0.8,
            radius: 0.015,
            line: false
        }
    );
    assert!(matches!(
        light(&read, "Line").shape,
        LightShape::Cylinder { line: true, .. }
    ));
    // A DomeLight's pole is its own +Y; a DomeLight_1's is its poleAxis,
    // the scene's up axis by its fallback.
    let pole = |name| match light(&read, name).shape {
        LightShape::Dome { pole, format, .. } => (pole, format),
        other => panic!("{other:?}"),
    };
    assert_eq!(pole("Old"), (Pole::Y, DomeFormat::Automatic));
    assert_eq!(pole("New"), (Pole::Z, DomeFormat::MirroredBall));
    assert_eq!(pole("Scene").0, Pole::Scene);
    assert_eq!(pole("Cross").1, DomeFormat::CubeMapVerticalCross);
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
}

#[test]
fn light_api_shaping_and_shadows_read_on_any_shape() {
    let read = read(&stage(
        r#"def DiskLight "Spot" (
    prepend apiSchemas = ["ShapingAPI", "ShadowAPI"]
)
{
    float inputs:radius = 0.1
    bool inputs:normalize = 1
    float inputs:diffuse = 0.5
    float inputs:specular = 0.25
    float inputs:shaping:cone:angle = 25
    float inputs:shaping:cone:softness = 0.4
    float inputs:shaping:focus = 2
    color3f inputs:shaping:focusTint = (1, 0, 0)
    asset inputs:shaping:ies:file = @lamp.ies@
    float inputs:shaping:ies:angleScale = 0.5
    bool inputs:shaping:ies:normalize = 1
    color3f inputs:shadow:color = (0, 0, 1)
    float inputs:shadow:distance = 2
    float inputs:shadow:falloff = 0.5
    float inputs:shadow:falloffGamma = 2
}
def SphereLight "Hemisphere" (
    prepend apiSchemas = ["ShapingAPI"]
)
{
}
def CylinderLight "Shadowless" (
    prepend apiSchemas = ["ShadowAPI"]
)
{
    bool inputs:shadow:enable = 0
}
"#,
    ));
    let spot = light(&read, "Spot");
    assert!(spot.normalize);
    assert_eq!((spot.diffuse, spot.specular), (0.5, 0.25));
    let shaping = spot.shaping.unwrap();
    assert_eq!(
        (
            shaping.cone_angle,
            shaping.cone_softness,
            shaping.focus,
            shaping.focus_tint
        ),
        (25.0, 0.4, 2.0, [1.0, 0.0, 0.0])
    );
    // The profile is a file the light reads, its values as authored.
    assert!(shaping.ies.is_some());
    assert_eq!(
        (shaping.ies_angle_scale, shaping.ies_normalize),
        (0.5, true)
    );
    assert_eq!(
        spot.shadow,
        Shadow {
            enable: true,
            color: [0.0, 0.0, 1.0],
            distance: 2.0,
            falloff: 0.5,
            falloff_gamma: 2.0,
        }
    );
    // ShapingAPI applied with nothing authored is its fallbacks: a 90° cone.
    assert_eq!(light(&read, "Hemisphere").shaping, Some(Shaping::default()));
    assert!(!light(&read, "Shadowless").shadow.enable);
}

#[test]
fn what_no_standard_light_holds_is_said_once_a_light() {
    let read = read(&stage(
        r#"def DistantLight "Sun" (
    prepend apiSchemas = ["ShapingAPI"]
)
{
    float inputs:shaping:cone:angle = 10
}
def DomeLight "Sky" (
    prepend apiSchemas = ["ShapingAPI"]
)
{
    asset inputs:shaping:ies:file = @sky.ies@
}
def RectLight "Filtered"
{
    rel light:filters = </Barn>
}
def LightFilter "Barn"
{
}
def PluginLight "Custom"
{
}
def SphereLight "Linked"
{
    rel collection:lightLink:excludes = </Barn>
}
def SphereLight "Unshadowing"
{
    uniform bool collection:shadowLink:includeRoot = 0
}
def SphereLight "Everything"
{
    rel collection:lightLink:includes = </Barn>
}
"#,
    ));
    let said: Vec<&str> = read.warnings.iter().map(|w| w.message.as_str()).collect();
    for line in [
        "/Sun: ShapingAPI on a DistantLight is left out",
        "/Sky: ShapingAPI on a DomeLight (IES profile sky.ies) is left out",
        "/Filtered: its light filters are a renderer's own; left out",
        "/Barn: LightFilter is a renderer's own light filter; read as a group",
        "/Custom: PluginLight names a renderer's own shader, which no engine has; read as a group",
        "/Linked: its light and shadow links are left out; it lights and shadows everything",
        "/Unshadowing: its light and shadow links are left out; it lights and shadows everything",
    ] {
        assert!(said.contains(&line), "{line} in {said:?}");
    }
    // Includes beside the root (on by default) narrow nothing.
    assert!(
        !said.iter().any(|w| w.starts_with("/Everything")),
        "{said:?}"
    );
    // The sun lights as a sun, the filtered rect as a rect.
    assert!(light(&read, "Sun").shaping.is_none());
    assert!(matches!(
        light(&read, "Filtered").shape,
        LightShape::Rect { .. }
    ));
}

#[test]
fn camera_visibility_reads_ours_then_the_renderers_and_is_off_without() {
    let read = read(&stage(
        r#"def RectLight "Plain"
{
}
def RectLight "Ours"
{
    custom bool kinogaki:visibility:camera = 1
}
def RectLight "Proposed"
{
    bool visibility:camera = 1
}
def RectLight "Karma"
{
    bool inputs:karma:light:renderlightgeo = 1
}
def RectLight "RenderMan"
{
    int primvars:ri:attributes:visibility:camera = 1
}
def RectLight "Omniverse"
{
    bool visibleInPrimaryRay = 1
}
def DomeLight "Arnold"
{
    float primvars:arnold:camera = 0
}
def RectLight "OursFirst"
{
    custom bool kinogaki:visibility:camera = 0
    bool inputs:karma:light:renderlightgeo = 1
}
"#,
    ));
    for (name, seen) in [
        ("Plain", false),
        ("Ours", true),
        ("Proposed", true),
        ("Karma", true),
        ("RenderMan", true),
        ("Omniverse", true),
        ("Arnold", false),
        ("OursFirst", false),
    ] {
        assert_eq!(light(&read, name).camera, seen, "{name}");
    }
}

#[test]
fn every_kind_and_api_writes_back_as_usdlux_writes_them() {
    let shaping = Shaping {
        cone_angle: 25.0,
        cone_softness: 0.4,
        focus: 2.0,
        focus_tint: [1.0, 0.0, 0.0],
        ..Shaping::default()
    };
    let shadow = Shadow {
        color: [0.0, 0.0, 1.0],
        distance: 2.0,
        falloff: 0.5,
        falloff_gamma: 2.0,
        ..Shadow::default()
    };
    let lights = [
        (
            "Rect",
            LightShape::Rect {
                width: 2.0,
                height: 0.5,
                texture: None,
            },
        ),
        ("Disk", LightShape::Disk { radius: 0.25 }),
        (
            "Point",
            LightShape::Sphere {
                radius: 0.1,
                point: true,
            },
        ),
        (
            "Tube",
            LightShape::Cylinder {
                length: 0.8,
                radius: 0.015,
                line: false,
            },
        ),
        ("Sun", LightShape::Distant { angle: 0.53 }),
        (
            "Sky",
            LightShape::Dome {
                image: None,
                format: DomeFormat::Angular,
                pole: Pole::Z,
                ground: Some([0.2, 0.1, 0.05]),
            },
        ),
    ]
    .map(|(name, shape)| {
        let shaped = !matches!(shape, LightShape::Distant { .. } | LightShape::Dome { .. });
        let light = Light {
            shape,
            diffuse: 0.5,
            specular: 2.0,
            shaping: shaped.then(|| shaping.clone()),
            shadow: shadow.clone(),
            camera: name == "Rect",
            ..Light::default()
        };
        (name, light)
    });
    let items: Vec<FileItem> = (lights.iter())
        .map(|(name, l)| FileItem::top(*name, standard::light().wrap(l.clone()).unwrap()))
        .collect();
    let dir = Scratch::new("lights");
    let path = dir.join("lights.usda");
    write_file(&path, &items, metres());
    let back = read_file(&path, metres(), false);
    for (name, l) in &lights {
        assert_eq!(&light(&back, name), l, "{name}");
    }
    let text = std::fs::read_to_string(&path).unwrap();
    for line in [
        "def CylinderLight \"Tube\"",
        "bool treatAsPoint = 1",
        "def DomeLight_1 \"Sky\"",
        "uniform token poleAxis = \"Z\"",
        "prepend apiSchemas = [\"ShapingAPI\", \"ShadowAPI\"]",
        "float inputs:shaping:cone:angle = 25",
        "color3f inputs:shadow:color = (0, 0, 1)",
        "custom bool kinogaki:visibility:camera = 1",
        "custom bool kinogaki:visibility:camera = 0",
    ] {
        assert!(text.contains(line), "{line} in\n{text}");
    }
}
