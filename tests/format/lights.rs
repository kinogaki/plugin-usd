//! UsdLux lights read as the standard light: an unauthored input holds its
//! schema's fallback for the light's kind, and what of ShapingAPI the
//! standard light can't hold is left out, said once a light.
use crate::support::{item, metres};
use kinogaki_render_api::standard::{Light, LightShape};
use kinogaki_studio_api::{FileFormat, Imported, Input, Purpose, Reading, standard};
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

#[test]
fn unauthored_intensities_are_each_kinds_fallback() {
    let read = read(
        r#"#usda 1.0
(
    metersPerUnit = 1
)
def DistantLight "Sun"
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
    );
    // A DistantLight's fallback is 50,000, "to approximate sunlight".
    assert_eq!(light(&read, "Sun").intensity, 50_000.0);
    assert_eq!(light(&read, "Bulb").intensity, 1.0);
    let panel = light(&read, "Panel");
    assert_eq!(panel.intensity, 1.0);
    assert_eq!(
        panel.shape,
        LightShape::Rect {
            width: 1.0,
            height: 1.0
        }
    );
}

#[test]
fn shaping_the_standard_light_cant_hold_is_said_once_a_light() {
    let read = read(
        r#"#usda 1.0
(
    metersPerUnit = 1
)
def DiskLight "Spot" (
    prepend apiSchemas = ["ShapingAPI"]
)
{
    float inputs:radius = 0.1
    float inputs:shaping:cone:angle = 25
    float inputs:shaping:cone:softness = 0.4
}
def SphereLight "Bulb" (
    prepend apiSchemas = ["ShapingAPI"]
)
{
    float inputs:shaping:cone:angle = 30
    float inputs:shaping:focus = 2
    color3f inputs:shaping:focusTint = (1, 0, 0)
    asset inputs:shaping:ies:file = @bulb.ies@
}
def RectLight "Plain" (
    prepend apiSchemas = ["ShapingAPI"]
)
{
    float inputs:shaping:cone:angle = 90
    float inputs:shaping:focus = 0
    color3f inputs:shaping:focusTint = (1, 0, 0)
}
def DistantLight "Sun"
{
    float inputs:shaping:cone:angle = 10
}
"#,
    );
    let said: Vec<&str> = read.warnings.iter().map(|w| w.message.as_str()).collect();
    assert_eq!(
        said,
        [
            "/Spot: ShapingAPI's cone on a DiskLight is left out",
            "/Bulb: ShapingAPI's focus and IES profile bulb.ies are left out",
        ]
    );
    // The disk lights as a disk; the sphere's cone is its spot's.
    assert_eq!(light(&read, "Spot").shape, LightShape::Disk { radius: 0.1 });
    let LightShape::Spot { cone_angle, .. } = light(&read, "Bulb").shape else {
        panic!("a sphere with a cone is a spot");
    };
    assert_eq!(cone_angle, 30.0);
}
