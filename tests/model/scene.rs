//! The scene of a stage: meshes, shapes, curves, points, cameras and
//! lights read as authored, and written back.

use crate::common::{fixture, mesh, node, paths, round_trips, scene_of, scene_with};
use kinogaki_usd::usd::scene::{
    CurveBasis, CurveForm, CurveWrap, Interpolation, LightKind, Matrix4d, NodeKind, Projection,
    ReadOptions, ShapeForm, SubsetElement, UpAxis,
};
use kinogaki_usd::usd::sdf::{Array, Value};

/// A node's world transform: its own after its parents'.
fn world(scene: &kinogaki_usd::usd::scene::Scene, path: &str) -> Matrix4d {
    let index = (0..scene.nodes.len())
        .find(|&at| scene.path(at).text() == path)
        .unwrap();
    let mut matrix = Matrix4d::IDENTITY;
    let mut at = Some(index);
    while let Some(node) = at.map(|at| &scene.nodes[at]) {
        matrix = matrix.times(&node.transform);
        at = if node.resets_transform {
            None
        } else {
            node.parent
        };
    }
    matrix
}

#[test]
fn reads_the_tree_with_local_transforms_and_units() {
    let scene = fixture("cube.usda");
    assert_eq!((scene.up_axis, scene.meters_per_unit), (UpAxis::Z, 1.0));
    // Prim order, the Scope holding Materials among them (a library's
    // place); invisible prims stay, marked.
    let all = paths(&scene);
    let expected = [
        "/World",
        "/World/Cube",
        "/World/Hidden",
        "/World/Moving",
        "/World/Looks",
    ];
    assert_eq!(all, expected);
    assert!(!node(&scene, "/World/Hidden").visible);
    let world_node = node(&scene, "/World");
    assert_eq!(
        (world_node.type_name.as_str(), &world_node.kind),
        ("Xform", &NodeKind::Group)
    );
    // Rotate 90 about z, then translate by 10 in x.
    let moved = world(&scene, "/World/Cube").transform_point([1.0, 0.0, 0.0]);
    assert!((moved[0] - 10.0).abs() < 1e-9 && (moved[1] - 1.0).abs() < 1e-9);
    assert!(node(&scene, "/World/Cube").transform.is_identity());
    let cube = mesh(&scene, "/World/Cube");
    assert_eq!((cube.face_vertex_counts.len(), cube.points.len()), (6, 8));
    assert_eq!(cube.subdivision.scheme.as_str(), "catmullClark");
    assert_eq!(cube.subdivision.crease_indices, [0, 1]);
    assert!(mesh(&scene, "/World/Moving").left_handed);
}

#[test]
fn reads_primvars_as_authored_and_subsets_with_their_bindings() {
    let scene = fixture("cube.usda");
    let cube = mesh(&scene, "/World/Cube");
    let st = cube.primvar("st").unwrap();
    assert_eq!(
        (st.interpolation, st.type_name.as_str()),
        (Interpolation::FaceVarying, "texCoord2f[]")
    );
    assert_eq!(st.indices.as_ref().unwrap().len(), 24);
    assert!(matches!(&st.values, Value::Array(Array::Vec2f(v)) if v.len() == 4));
    assert_eq!(
        cube.primvar("displayColor").unwrap().interpolation,
        Interpolation::Constant
    );
    let red = node(&scene, "/World/Cube").material.unwrap();
    assert_eq!(scene.materials[red].path.text(), "/World/Looks/Red");
    assert_eq!(cube.subsets.len(), 1);
    assert_eq!(cube.subsets[0].element, SubsetElement::Face);
    assert_eq!(cube.subsets[0].family.as_str(), "materialBind");
    let blue = cube.subsets[0].material.unwrap();
    assert_eq!(scene.materials[blue].path.text(), "/World/Looks/Blue");
}

#[test]
fn shares_the_layers_columns() {
    let text = include_bytes!("../fixtures/cube.usda");
    let mut stage = crate::common::stage_named("cube.usda", text);
    let scene = kinogaki_usd::usd::scene::read(&mut stage, &ReadOptions::default()).unwrap();
    let Some(Value::Array(Array::Vec3f(points))) = stage.root_layer().get(
        &kinogaki_usd::usd::sdf::Path::parse("/World/Cube.points").unwrap(),
        "default",
    ) else {
        panic!("points")
    };
    assert!(std::sync::Arc::ptr_eq(
        points,
        &mesh(&scene, "/World/Cube").points
    ));
}

#[test]
fn reads_time_samples_at_a_time() {
    let text = include_bytes!("../fixtures/cube.usda");
    let at = |time| {
        let options = ReadOptions {
            time: Some(time),
            ..ReadOptions::default()
        };
        mesh(&scene_with(text, &options), "/World/Moving").points[0][2]
    };
    assert_eq!((at(0.0), at(5.0), at(20.0)), (0.0, 5.0, 10.0));
}

#[test]
fn intrinsic_shapes_read_as_authored_and_tessellate() {
    let scene = scene_of(
        br#"#usda 1.0
def Xform "Shapes"
{
    def Cube "Box"
    {
        double size = 4
        color3f[] primvars:displayColor = [(0, 1, 0)]
    }
    def Sphere "Ball"
    {
        double radius = 2
    }
    def Cylinder "Can"
    {
        uniform token axis = "X"
    }
}
"#,
    );
    let shape = |path| match &node(&scene, path).kind {
        NodeKind::Shape(shape) => shape.clone(),
        other => panic!("{other:?}"),
    };
    let cube = shape("/Shapes/Box");
    assert_eq!(cube.form, ShapeForm::Cube { size: 4.0 });
    let box_mesh = cube.mesh();
    assert_eq!(
        (box_mesh.points.len(), box_mesh.face_vertex_counts.len()),
        (8, 6)
    );
    assert_eq!(box_mesh.points[7], [2.0, 2.0, 2.0]);
    assert_eq!(box_mesh.primvars[0].name.as_str(), "displayColor");
    let ball = shape("/Shapes/Ball").mesh();
    assert!(
        (ball.points.iter()).all(
            |p| (p.map(f32::abs).iter().map(|c| c * c).sum::<f32>().sqrt() - 2.0).abs() < 1e-5
        )
    );
    let can = shape("/Shapes/Can").mesh();
    assert!(
        can.points.iter().all(|p| p[0].abs() == 1.0),
        "the cylinder lies along x"
    );
}

#[test]
fn proxy_and_guide_purposes_load_when_asked() {
    let text = br#"#usda 1.0
def Xform "World" {
    def Mesh "Render" {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
    }
    def Mesh "Proxy" {
        uniform token purpose = "proxy"
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
    }
    def Mesh "Guide" {
        uniform token purpose = "guide"
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
    }
}
"#;
    let names = |proxy, guide| {
        paths(&scene_with(
            text,
            &ReadOptions {
                proxy,
                guide,
                ..ReadOptions::default()
            },
        ))
    };
    assert_eq!(names(false, false), ["/World", "/World/Render"]);
    assert_eq!(
        names(true, true),
        ["/World", "/World/Render", "/World/Proxy", "/World/Guide"]
    );
    assert_eq!(
        names(true, false),
        ["/World", "/World/Render", "/World/Proxy"]
    );
}

#[test]
fn reads_cameras_as_authored_with_schema_fallbacks() {
    let scene = fixture("camera_light.usda");
    let camera = |path| match &node(&scene, path).kind {
        NodeKind::Camera(camera) => camera.clone(),
        other => panic!("{other:?}"),
    };
    let shot = camera("/World/Shot");
    assert_eq!(shot.projection, Projection::Perspective);
    assert_eq!(
        (
            shot.focal_length,
            shot.horizontal_aperture,
            shot.vertical_aperture
        ),
        (35.0, 36.0, 24.0)
    );
    assert_eq!(shot.clipping_range, [0.5, 5000.0]);
    assert_eq!(
        (
            shot.horizontal_aperture_offset,
            shot.vertical_aperture_offset
        ),
        (1.5, 0.0)
    );
    assert_eq!(
        (shot.focus_distance, shot.f_stop),
        (250.0, 2.799999952316284)
    );
    assert_eq!(
        world(&scene, "/World/Shot").transform_point([0.0; 3]),
        [0.0, 10.0, 500.0]
    );
    let plan = camera("/World/Plan");
    assert_eq!(plan.projection, Projection::Orthographic);
    assert_eq!(
        (
            plan.focal_length,
            plan.horizontal_aperture,
            plan.vertical_aperture
        ),
        (50.0, 20.955, 15.2908),
        "UsdGeomCamera's fallbacks"
    );
    assert_eq!(plan.clipping_range, [1.0, 1_000_000.0]);
    assert_eq!((shot.shutter, plan.shutter), ([-0.25, 0.25], [0.0, 0.0]));
}

#[test]
fn reads_every_light_kind_old_and_new_spellings() {
    let scene = fixture("camera_light.usda");
    let lights: Vec<_> = (scene.nodes.iter())
        .filter_map(|n| match &n.kind {
            NodeKind::Light(light) => Some(light.clone()),
            _ => None,
        })
        .collect();
    use LightKind::*;
    assert_eq!(
        lights.iter().map(|l| l.kind).collect::<Vec<_>>(),
        [Rect, Disk, Sphere, Distant, Dome, Sphere, Cylinder]
    );
    let key = &lights[0];
    assert_eq!(
        (key.intensity, key.exposure, key.color),
        (20.0, 1.5, [1.0, 0.5, 0.25])
    );
    assert!(key.enable_color_temperature && key.normalize);
    assert_eq!(
        (key.color_temperature, key.width, key.height),
        (3200.0, 2.0, 3.0)
    );
    // Inputs written before UsdLux's `inputs:` prefix.
    assert_eq!((lights[1].intensity, lights[1].radius), (7.0, 4.0));
    assert_eq!(
        (lights[2].radius as f32, lights[2].shaping.is_none()),
        (0.1, true)
    );
    assert_eq!(lights[3].angle, 1.0);
    // The dome's image is one of the scene's assets, anchored at the layer.
    let sky = &lights[4];
    let texture = sky.texture.as_ref().unwrap();
    assert!(
        texture.ends_with("/tests/fixtures/sky/noon.exr"),
        "{texture}"
    );
    let asset = scene.asset(texture).unwrap();
    assert_eq!(asset.authored.as_str(), "sky/noon.exr");
    let spot = &lights[5];
    let shaping = spot.shaping.as_ref().unwrap();
    assert_eq!(
        (spot.radius, shaping.cone_angle, shaping.cone_softness),
        (0.25, 30.0, 0.5)
    );
    // A tube along X, two long.
    assert_eq!(lights[6].length, 2.0);
    assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
}

#[test]
fn reads_basis_and_nurbs_curves_as_authored() {
    let scene = fixture("curves.usda");
    let curves = |path| match &node(&scene, path).kind {
        NodeKind::Curves(curves) => curves.clone(),
        other => panic!("{path}: {other:?}"),
    };
    let wire = curves("/World/Wire");
    assert_eq!(wire.form, CurveForm::Linear { periodic: true });
    assert_eq!(
        (wire.vertex_counts.as_slice(), wire.points.len()),
        (&[3, 4][..], 7)
    );
    let widths = wire
        .primvars
        .iter()
        .find(|p| p.name.as_str() == "widths")
        .unwrap();
    assert_eq!(widths.interpolation, Interpolation::Uniform);
    assert!(
        wire.primvars
            .iter()
            .any(|p| p.name.as_str() == "displayColor")
    );
    let hair = curves("/World/Hair");
    assert_eq!(
        hair.form,
        CurveForm::Cubic {
            basis: CurveBasis::BSpline,
            wrap: CurveWrap::Pinned
        }
    );
    let widths = hair
        .primvars
        .iter()
        .find(|p| p.name.as_str() == "widths")
        .unwrap();
    assert_eq!(widths.interpolation, Interpolation::Vertex);
    assert_eq!(hair.elements(Interpolation::Varying), 5);
    let path = curves("/World/Path");
    assert_eq!(
        path.form,
        CurveForm::Cubic {
            basis: CurveBasis::Bezier,
            wrap: CurveWrap::Nonperiodic
        }
    );
    let arc = curves("/World/Arc");
    let CurveForm::Nurbs {
        orders,
        knots,
        ranges,
        weights,
    } = &arc.form
    else {
        panic!("Arc is NURBS")
    };
    assert_eq!((orders.as_slice(), knots.len()), (&[3][..], 6));
    assert_eq!(ranges, &[[0.0, 1.0]]);
    assert_eq!(weights[1], std::f64::consts::FRAC_1_SQRT_2);
    assert_eq!(
        world(&scene, "/World/Arc").transform_point([0.0; 3]),
        [0.0, 0.0, 5.0]
    );
    // Curves that don't add up read as groups, each with a note.
    let notes: Vec<&String> = scene
        .warnings
        .iter()
        .filter(|w| w.ends_with("read as a group"))
        .collect();
    assert_eq!(notes.len(), 3, "{notes:?}");
    assert!(notes[0].starts_with("/World/Short: curveVertexCounts"));
    assert!(notes[1].starts_with("/World/BadKnots: knots"));
    assert!(notes[2].starts_with("/World/Thin: primvar widths"));
}

#[test]
fn points_read_with_widths_and_ids() {
    let scene = scene_of(
        br#"#usda 1.0
def Points "Dust"
{
    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
    float[] widths = [0.1, 0.2, 0.3]
    int64[] ids = [7, 8, 9]
}
"#,
    );
    let NodeKind::Points(points) = &node(&scene, "/Dust").kind else {
        panic!("points")
    };
    assert_eq!(points.points.len(), 3);
    assert_eq!(
        points.ids.as_deref().map(Vec::as_slice),
        Some(&[7, 8, 9][..])
    );
    assert_eq!(points.primvars[0].name.as_str(), "widths");
    assert_eq!(points.primvars[0].interpolation, Interpolation::Vertex);
}

#[test]
fn colors_say_the_space_they_are_authored_in() {
    let scene = scene_of(
        br#"#usda 1.0
def Xform "Looks" (
    prepend apiSchemas = ["ColorSpaceAPI"]
)
{
    uniform token colorSpace:name = "lin_ap1_scene"
    def Material "Red"
    {
        token outputs:surface.connect = </Looks/Red/Surface.outputs:surface>
        def Shader "Surface"
        {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor = (0.8, 0.1, 0.1)
            color3f inputs:emissiveColor = (1, 1, 1) (
                colorSpace = "srgb_rec709_scene"
            )
            token outputs:surface
        }
    }
}
def RectLight "Key"
{
    color3f inputs:color = (1, 0.5, 0.25) (
        colorSpace = "lin_ap1_scene"
    )
}
def DiskLight "Fill"
{
    color3f inputs:color = (1, 1, 1)
}
"#,
    );
    let surface = scene.materials[0].surface("").unwrap();
    let space = |name| {
        surface
            .input(name)
            .unwrap()
            .color_space
            .as_ref()
            .map(|s| s.to_string())
    };
    // Inherited from the ColorSpaceAPI above; the attribute's own wins.
    assert_eq!(space("diffuseColor").as_deref(), Some("lin_ap1_scene"));
    assert_eq!(space("emissiveColor").as_deref(), Some("srgb_rec709_scene"));
    let light_space = |path| match &node(&scene, path).kind {
        NodeKind::Light(light) => light.color_space.as_ref().map(|s| s.to_string()),
        other => panic!("{other:?}"),
    };
    assert_eq!(light_space("/Key").as_deref(), Some("lin_ap1_scene"));
    assert_eq!(light_space("/Fill"), None, "unsaid: linear Rec.709");
}

#[test]
fn scenes_write_and_read_back_unchanged() {
    for name in ["cube.usda", "camera_light.usda", "curves.usda"] {
        // The schema's float attributes round the fallbacks (20.955) once;
        // from then on a round trip is exact.
        let [first, _] = round_trips(&fixture(name));
        for again in round_trips(&first) {
            assert_eq!(again.nodes, first.nodes, "{name}");
            assert_eq!(again.materials, first.materials, "{name}");
            assert_eq!(again.prototypes, first.prototypes, "{name}");
            assert_eq!(
                (again.up_axis, again.meters_per_unit),
                (first.up_axis, first.meters_per_unit)
            );
        }
    }
}

#[test]
fn a_reset_transform_and_a_root_level_mesh_write_as_they_were() {
    let text = br#"#usda 1.0
def Mesh "Sheet"
{
    int[] faceVertexCounts = [3]
    int[] faceVertexIndices = [0, 1, 2]
    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
}
def Xform "Rig"
{
    double3 xformOp:translate = (1, 2, 3)
    uniform token[] xformOpOrder = ["xformOp:translate"]
    def Mesh "Pinned"
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
        double3 xformOp:translate = (0, 0, 4)
        uniform token[] xformOpOrder = ["!resetXformStack!", "xformOp:translate"]
    }
}
"#;
    let scene = scene_of(text);
    assert!(node(&scene, "/Rig/Pinned").resets_transform);
    let layer = kinogaki_usd::usd::scene::author(&scene).unwrap();
    let written = String::from_utf8(kinogaki_usd::usd::text::write_usda(&layer).unwrap()).unwrap();
    assert!(written.contains("def Mesh \"Sheet\""), "{written}");
    assert!(written.contains("\"!resetXformStack!\""), "{written}");
    let [again, _] = round_trips(&scene);
    assert_eq!(again.nodes, scene.nodes);
    assert_eq!(
        world(&again, "/Rig/Pinned").transform_point([0.0; 3]),
        [0.0, 0.0, 4.0]
    );
}

#[test]
fn a_name_that_is_not_an_identifier_is_refused() {
    let mut scene = kinogaki_usd::usd::scene::Scene::default();
    scene
        .nodes
        .push(kinogaki_usd::usd::scene::Node::group("A", None));
    scene
        .nodes
        .push(kinogaki_usd::usd::scene::Node::group("not a name", Some(0)));
    let error = kinogaki_usd::usd::scene::author(&scene).unwrap_err();
    assert!(error.to_string().contains("not a name"), "{error}");
}
