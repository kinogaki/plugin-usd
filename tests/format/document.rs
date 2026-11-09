//! File ▸ Open's read: the stage's own units kept, its items as Import
//! reads them into those units.
use super::support::Scratch;
use kinogaki_studio_api::geometry::{Units, UpAxis};
use kinogaki_studio_api::{FileFormat, Input, Purpose, Reading, standard};
use kinogaki_studio_api_testing::Unconverted;
use kinogaki_studio_api_testing::format::document;
use kinogaki_usd::Usd;

/// A centimetre, Z-up stage: a placed group holding a mesh, a light and a
/// camera.
const CENTIMETRES: &str = r#"#usda 1.0
(
    defaultPrim = "World"
    metersPerUnit = 0.01
    upAxis = "Z"
)

def Xform "World"
{
    double3 xformOp:translate = (0, 0, 100)
    uniform token[] xformOpOrder = ["xformOp:translate"]

    def Mesh "Floor"
    {
        int[] faceVertexCounts = [4]
        int[] faceVertexIndices = [0, 1, 2, 3]
        point3f[] points = [(0, 0, 0), (200, 0, 0), (200, 200, 0), (0, 200, 0)]
    }

    def SphereLight "Key"
    {
        float inputs:radius = 10
    }

    def Camera "Shot"
    {
        float focalLength = 50
    }
}
"#;

#[test]
fn a_document_read_keeps_the_stages_units() {
    let scratch = Scratch::new("usd-document");
    let path = scratch.join("centimetres.usda");
    std::fs::write(&path, CENTIMETRES).unwrap();
    let input = Input::file(&path);
    let units = document(&Usd, &input, &Usd.options()).unwrap();
    assert_eq!(units, Units::new(0.01, UpAxis::Z).unwrap());
    // Nothing scaled or turned: the floor's corner and the group's height
    // are the file's numbers.
    let reading = Reading::new(Purpose::Document, Units::MILLIMETERS, &Unconverted);
    let read = Usd.read(&input, &Usd.options(), &reading).unwrap();
    let world = read.items.iter().find(|i| i.name == "World").unwrap();
    assert_eq!(world.placement.origin().to_array(), [0.0, 0.0, 100.0]);
    let floor = (read.items.iter().filter_map(|i| i.data.as_ref()))
        .find_map(|d| standard::geometry().get(d))
        .unwrap();
    assert_eq!(floor.positions().as_slice()[2], [200.0, 200.0, 0.0]);
}

#[test]
fn a_stage_composed_of_other_files_says_it_flattens_them() {
    let scratch = Scratch::new("usd-flattened");
    let read = |name: &str| {
        let reading = Reading::new(Purpose::Document, Units::METERS, &Unconverted);
        let path = scratch.join(name);
        Usd.read(&Input::file(&path), &Usd.options(), &reading)
            .unwrap()
    };
    std::fs::write(scratch.join("part.usda"), CENTIMETRES).unwrap();
    // A sublayer and a reference to another file: their content flattened.
    let sublayer = "#usda 1.0\n(\n    subLayers = [@./part.usda@]\n)\n";
    std::fs::write(scratch.join("sublayer.usda"), sublayer).unwrap();
    let reference =
        "#usda 1.0\ndef Xform \"Shelf\" (\n    references = @./part.usda@</World>\n)\n{\n}\n";
    std::fs::write(scratch.join("reference.usda"), reference).unwrap();
    // Arcs inside the file alone: nothing of another file's.
    let internal = "#usda 1.0\ndef Xform \"Proto\"\n{\n}\ndef Xform \"Copy\" (\n    instanceable = true\n    references = </Proto>\n)\n{\n}\n";
    std::fs::write(scratch.join("internal.usda"), internal).unwrap();
    assert!(read("sublayer.usda").flattened);
    assert!(read("reference.usda").flattened);
    assert!(!read("internal.usda").flattened);
    assert!(!read("part.usda").flattened);
}
