//! Files read through the format into the standard values: the fixtures'
//! prims, primvars, bindings, lights and cameras where the project puts
//! them; instances of shared prototypes; damaged files, cancellation.
use crate::support::{Scratch, fixture, item, materials, metres, read_file};
use kinogaki_render_api::standard::LightShape;
use kinogaki_studio_api::geometry::image::ImageFile;
use kinogaki_studio_api::geometry::{Domain, Units, UpAxis, Vec3, names};
use kinogaki_studio_api::image_files::{IMAGE_NODE, OP, image_source};
use kinogaki_studio_api::standard;
use kinogaki_studio_api::{
    CancelToken, Error, FileFormat, FormatOptions, Input, Purpose, Reading, Value,
};
use kinogaki_studio_api_testing::Unconverted;
use kinogaki_usd::Usd;

#[test]
fn a_z_up_stage_lands_in_a_y_up_project_with_its_bindings_and_primvars() {
    let read = read_file(&fixture("cube.usda"), metres(), false);
    let looks = materials(&read);
    let keys: Vec<&str> = looks.materials.iter().map(|m| m.key.as_str()).collect();
    assert_eq!(keys, ["/World/Looks/Red", "/World/Looks/Blue"]);
    // World moves 10 along the stage's X: the project's X too.
    let (world, _) = item(&read, "World");
    assert_eq!(world.placement.origin(), Vec3::new(10.0, 0.0, 0.0));
    // The cube's corner (-1, -1, 1) is up the stage's Z: the project's Y.
    let (cube, value) = item(&read, "Cube");
    assert_eq!(read.items[cube.parent.unwrap()].name, "World");
    let cube = standard::geometry().get(value.unwrap()).unwrap();
    assert_eq!(cube.positions()[0], [-1.0, 1.0, 1.0]);
    let attributes = cube.attributes();
    // `st` through its indices, one per corner; displayColor once.
    let uv = attributes
        .tuples::<[f32; 2]>(Domain::Vertex, names::UV)
        .unwrap();
    assert_eq!((uv.len(), uv[2]), (24, [1.0, 1.0]));
    assert!(attributes.get(Domain::Detail, names::CD).is_some());
    // The top face's subset binds Blue, the others the mesh's Red; the
    // subset is a group too.
    let material = attributes.text(Domain::Primitive, names::MATERIAL).unwrap();
    assert_eq!(material.get(0), Some("/World/Looks/Blue"));
    assert_eq!(material.get(1), Some("/World/Looks/Red"));
    assert!(attributes.group(Domain::Primitive, "top").is_some());
    // An invisible prim starts hidden.
    assert!(!item(&read, "Hidden").0.visible);
    // Blue's texture names a file beside the layer: a path, absolute.
    let blue = &looks.materials[1];
    let image = blue
        .inside
        .nodes
        .iter()
        .find(|n| n.node_type == IMAGE_NODE)
        .unwrap();
    let source = image_source(&image.parameters).unwrap().unwrap();
    let beside = fixture("textures/blue.png").display().to_string();
    assert_eq!(*source.file(), ImageFile::Path(beside.as_str().into()));
}

#[test]
fn a_left_handed_mesh_winds_its_faces_the_other_way() {
    let read = read_file(&fixture("cube.usda"), metres(), false);
    let moving = standard::geometry()
        .get(item(&read, "Moving").1.unwrap())
        .unwrap();
    assert_eq!(moving.primitives().primitive(0), [0, 2, 1]);
}

#[test]
fn a_light_or_camera_out_of_range_is_left_out_and_said() {
    let scratch = Scratch::new("out-of-range");
    let path = scratch.path().join("lights.usda");
    let text = r#"#usda 1.0
def SphereLight "Dim"
{
    float inputs:intensity = -5
}
def DiskLight "Flat"
{
    float inputs:radius = 0
}
def Camera "Blind"
{
    float focalLength = 0
}
def SphereLight "Bulb"
{
    float inputs:intensity = 5
}
"#;
    std::fs::write(&path, text).unwrap();
    let read = read_file(&path, metres(), false);
    for name in ["Dim", "Flat", "Blind"] {
        let held = read.items.iter().find(|i| i.name == name);
        assert!(
            held.is_none_or(|i| i.data.is_none()),
            "{name} isn't a value"
        );
        let said = read.warnings.iter().map(|w| &w.message);
        assert!(
            said.clone()
                .any(|w| w.contains(name) && w.contains("left out")),
            "{name}: {:?}",
            read.warnings
        );
    }
    let bulb = standard::light().get(item(&read, "Bulb").1.unwrap());
    assert_eq!(bulb.map(|l| l.intensity), Some(5.0));
}

#[test]
fn units_scale_points_and_lengths_but_no_placement() {
    let millimetres = Units::new(0.001, UpAxis::Z).unwrap();
    let read = read_file(&fixture("camera_light.usda"), millimetres, false);
    // A centimetre stage, Y up, into millimetres, Z up: World rises 10 cm.
    let (world, _) = item(&read, "World");
    assert_eq!(world.placement.origin(), Vec3::new(0.0, 0.0, 100.0));
    let key = standard::light()
        .get(item(&read, "Key").1.unwrap())
        .unwrap();
    assert_eq!(
        key.shape,
        LightShape::Rect {
            width: 20.0,
            height: 30.0
        }
    );
    let shot = standard::camera()
        .get(item(&read, "Shot").1.unwrap())
        .unwrap();
    // Tenths of a centimetre are millimetres; distances are project units.
    assert_eq!((shot.focal_length, shot.horizontal_aperture), (35.0, 36.0));
    assert_eq!(shot.clipping_range, [5.0, 50_000.0]);
    assert_eq!(shot.focus_distance, 2500.0);
    let bulb = standard::light()
        .get(item(&read, "Bulb").1.unwrap())
        .unwrap();
    let LightShape::Spot { cone_angle, .. } = bulb.shape else {
        panic!("a sphere light is a spot");
    };
    assert_eq!(cone_angle, 180.0);
}

#[test]
fn instances_share_one_prototype_and_point_instancers_place_theirs() {
    let layer = br#"#usda 1.0
(
    metersPerUnit = 1
)
def Xform "Asset"
{
    def Mesh "Leaf" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
    }
}
def Material "Green"
{
}
def Xform "One" (
    instanceable = true
    prepend apiSchemas = ["MaterialBindingAPI"]
    references = </Asset>
)
{
    rel material:binding = </Green>
    double3 xformOp:translate = (5, 0, 0)
    uniform token[] xformOpOrder = ["xformOp:translate"]
}
def Xform "Two" (
    instanceable = true
    references = </Asset>
)
{
}
def PointInstancer "Field"
{
    rel prototypes = </Field/Prototypes/Leaf>
    int[] protoIndices = [0, 0, 0]
    point3f[] positions = [(0, 0, 0), (2, 0, 0), (4, 0, 0)]
    int64[] invisibleIds = [1]
    def Scope "Prototypes"
    {
        def Mesh "Leaf"
        {
            int[] faceVertexCounts = [3]
            int[] faceVertexIndices = [0, 1, 2]
            point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
        }
    }
}
"#;
    let reading = Reading::new(Purpose::Objects, metres(), &Unconverted);
    let options = Usd.options();
    let read = Usd
        .read(&Input::memory("leaves.usda", layer), &options, &reading)
        .unwrap();
    let one = standard::instances()
        .get(item(&read, "One").1.unwrap())
        .unwrap();
    let two = standard::instances()
        .get(item(&read, "Two").1.unwrap())
        .unwrap();
    assert!(
        one.prototypes()[0].ptr_eq(&two.prototypes()[0]),
        "one prototype, held once"
    );
    assert_eq!(
        item(&read, "One").0.placement.origin(),
        Vec3::new(5.0, 0.0, 0.0)
    );
    // One binds Green, which the prototype's leaf leaves to it.
    let bound = one
        .attributes()
        .text(Domain::Instance, names::MATERIAL)
        .unwrap();
    assert_eq!(bound.get(0), Some("/Green"));
    assert!(
        two.attributes()
            .text(Domain::Instance, names::MATERIAL)
            .is_none()
    );
    // The field shows two of its three: the second is invisible.
    let field = standard::instances()
        .get(item(&read, "Field").1.unwrap())
        .unwrap();
    let at: Vec<Vec3> = field.xforms().iter().map(|x| x.origin()).collect();
    assert_eq!(at, [Vec3::new(0.0, 0.0, 0.0), Vec3::new(4.0, 0.0, 0.0)]);
}

#[test]
fn a_dome_reads_its_image_by_op_and_key() {
    let layer = br#"#usda 1.0
def DomeLight "Sky"
{
    asset inputs:texture:file = @sky.exr@
}
"#;
    let dir = crate::support::Scratch::new("dome");
    let path = dir.join("sky.usda");
    std::fs::write(&path, layer).unwrap();
    std::fs::write(dir.join("sky.exr"), b"not really an exr").unwrap();
    let read = read_file(&path, metres(), true);
    let image = &materials(&read).images[0];
    assert_eq!(image.name, "sky");
    let ImageFile::Embedded(file) = image.source.file() else {
        panic!("Embed Textures reads it in");
    };
    assert_eq!(file.bytes(), b"not really an exr");
    let sky = standard::light()
        .get(item(&read, "Sky").1.unwrap())
        .unwrap();
    let LightShape::Dome {
        image: Some(named), ..
    } = &sky.shape
    else {
        panic!("a dome");
    };
    assert_eq!(**named, format!("{OP}{}", image.key));
}

#[test]
fn damaged_files_fail_and_a_cancelled_read_stops() {
    let usdc = {
        let dir = crate::support::Scratch::new("damaged");
        let path = dir.join("cube.usdc");
        let read = read_file(&fixture("cube.usda"), metres(), false);
        crate::support::write_file(&path, &read.items, metres());
        std::fs::read(path).unwrap()
    };
    let reading = Reading::new(Purpose::Objects, metres(), &Unconverted);
    let options = Usd.options();
    for cut in (0..usdc.len()).step_by(97) {
        let input = Input::memory("cut.usdc", &usdc[..cut]);
        // An error naming the input, never a panic; a cut that leaves a
        // whole layer may read.
        if let Err(e) = Usd.read(&input, &options, &reading) {
            assert!(e.message().contains("cut.usdc"), "{e}");
        }
    }
    let stop = CancelToken::default();
    stop.cancel();
    let cancelled = Reading {
        budget: kinogaki_studio_api::format::ReadBudget::default().cancelled_by(stop),
        ..Reading::new(Purpose::Objects, metres(), &Unconverted)
    };
    let input = Input::memory("cube.usdc", &usdc);
    assert_eq!(
        Usd.read(&input, &options, &cancelled).unwrap_err(),
        Error::cancelled()
    );
}

#[test]
fn the_import_dialog_offers_embed_textures_off() {
    let options = Usd.options();
    assert_eq!(options.get("embed"), Some(&Value::Bool(false)));
    let schema = Usd.schema(&FormatOptions::new(), Purpose::Objects);
    assert_eq!(
        schema
            .parameters()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["embed"]
    );
}

/// A camera and a light keep what they look at in a project of another up
/// axis: their values look down their own −Z whatever the space, so their
/// placements turn with the stage and are never turned back.
#[test]
fn cameras_and_lights_look_where_the_stage_has_them_look_in_another_up_axis() {
    let dir = crate::support::Scratch::new("frames");
    let path = dir.join("frames.usda");
    let stage = r#"#usda 1.0
(
    metersPerUnit = 1
    upAxis = "Z"
)
def Xform "World"
{
    double3 xformOp:translate = (0, 0, 5)
    uniform token[] xformOpOrder = ["xformOp:translate"]

    def Camera "Down"
    {
    }

    def RectLight "Lamp"
    {
        float inputs:width = 1
        float inputs:height = 1
    }
}
"#;
    std::fs::write(&path, stage).unwrap();
    let read = read_file(&path, metres(), false);
    let (world, _) = item(&read, "World");
    assert_eq!(
        world.placement.origin(),
        Vec3::new(0.0, 5.0, 0.0),
        "Z up is Y up"
    );
    for name in ["Down", "Lamp"] {
        let (it, data) = item(&read, name);
        assert!(data.is_some(), "{name}");
        // At the stage's identity each looks down the stage's −Z, the
        // ground: the project's −Y.
        let placed = world.placement.compose(&it.placement);
        let looks = placed.apply_vector(-Vec3::Z);
        assert!(
            (looks - -Vec3::Y).length() < 1e-12,
            "{name} looks along {looks:?}"
        );
        assert!(
            (placed.apply_vector(Vec3::Y) - -Vec3::Z).length() < 1e-12,
            "{name}"
        );
        assert_eq!(placed.origin(), Vec3::new(0.0, 5.0, 0.0));
    }
}
