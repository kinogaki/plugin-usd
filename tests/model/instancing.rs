//! Instancing: instanceable prims share prototypes, point instancers place
//! theirs, and both write back.

use crate::common::{node, paths, round_trips, scene_of};
use kinogaki_usd::usd::scene::{self, Matrix4d, NodeKind};
use kinogaki_usd::usd::{Format, layer_bytes};

const INSTANCEABLE: &[u8] = br#"#usda 1.0
def Xform "Asset"
{
    def Mesh "Part"
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
    }
}

def Xform "Shelf"
{
    def Xform "One" (
        instanceable = true
        references = </Asset>
    )
    {
        double3 xformOp:translate = (5, 0, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }

    def Xform "Two" (
        instanceable = true
        prepend apiSchemas = ["MaterialBindingAPI"]
        references = </Asset>
    )
    {
        rel material:binding = </Looks/Red>
    }
}

def Scope "Looks"
{
    def Material "Red" {}
}
"#;

#[test]
fn instanceable_prims_share_one_prototype() {
    let scene = scene_of(INSTANCEABLE);
    assert_eq!(
        paths(&scene),
        [
            "/Asset",
            "/Asset/Part",
            "/Shelf",
            "/Shelf/One",
            "/Shelf/Two"
        ]
    );
    let (one, two) = (node(&scene, "/Shelf/One"), node(&scene, "/Shelf/Two"));
    assert_eq!(one.instance, Some(0));
    assert_eq!(two.instance, Some(0));
    assert_eq!(one.transform, Matrix4d::translation(5.0, 0.0, 0.0));
    // What the prototype's prims bind nothing for, the instance gives.
    assert_eq!((one.material, two.material), (None, Some(0)));
    let prototype = &scene.prototypes[0];
    assert_eq!(prototype.nodes.len(), 1);
    assert_eq!(prototype.path(0).text(), "/Shelf/One/Part");
    assert!(matches!(prototype.nodes[0].kind, NodeKind::Mesh(_)));
}

/// Written back, each instance references its prototype, written once
/// beneath the default prim's `Prototypes` class, and binds what it bound.
#[test]
fn instances_write_as_instances_of_their_prototype_written_once() {
    let scene = scene_of(INSTANCEABLE);
    let layer =
        String::from_utf8(layer_bytes(&scene::author(&scene).unwrap(), Format::Usda).unwrap())
            .unwrap();
    assert!(layer.contains("class \"Prototypes\""), "{layer}");
    assert_eq!(
        layer
            .matches("prepend references = </Asset/Prototypes/One>")
            .count(),
        2,
        "{layer}"
    );
    for again in round_trips(&scene) {
        assert_eq!(again.prototypes.len(), 1);
        assert_eq!(
            paths(&again),
            [
                "/Asset",
                "/Asset/Part",
                "/Shelf",
                "/Shelf/One",
                "/Shelf/Two"
            ]
        );
        let (one, two) = (node(&again, "/Shelf/One"), node(&again, "/Shelf/Two"));
        assert_eq!((one.instance, two.instance), (Some(0), Some(0)));
        assert_eq!((one.material, two.material), (None, Some(0)));
        assert_eq!(one.transform, Matrix4d::translation(5.0, 0.0, 0.0));
        let (NodeKind::Mesh(copy), NodeKind::Mesh(part)) = (
            &again.prototypes[0].nodes[0].kind,
            &scene.prototypes[0].nodes[0].kind,
        ) else {
            panic!("meshes")
        };
        assert_eq!(copy.points, part.points);
    }
}

/// A binding stronger than descendants is written stronger and read back
/// so, on an instance as anywhere.
#[test]
fn a_stronger_binding_writes_and_reads_back_stronger() {
    let mut scene = scene_of(INSTANCEABLE);
    let at = (scene.nodes.iter())
        .position(|n| n.name.as_str() == "Two")
        .unwrap();
    scene.nodes[at].stronger = true;
    for again in round_trips(&scene) {
        let two = node(&again, "/Shelf/Two");
        assert!(two.stronger && two.material == Some(0));
        assert!(!node(&again, "/Shelf/One").stronger);
    }
}

const INSTANCER: &[u8] = br#"#usda 1.0
def PointInstancer "Forest"
{
    point3f[] positions = [(0, 0, 0), (10, 0, 0), (0, 0, 10)]
    int[] protoIndices = [0, 1, 0]
    quath[] orientations = [(1, 0, 0, 0), (0.7071, 0, 0.7071, 0), (1, 0, 0, 0)]
    float3[] scales = [(1, 1, 1), (2, 2, 2), (1, 1, 1)]
    int64[] invisibleIds = [2]
    rel prototypes = [</Forest/Prototypes/Pine>, </Forest/Prototypes/Oak>]

    def Scope "Prototypes"
    {
        def Mesh "Pine"
        {
            int[] faceVertexCounts = [3]
            int[] faceVertexIndices = [0, 1, 2]
            point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
        }

        def Xform "Oak"
        {
            double3 xformOp:translate = (0, 1, 0)
            uniform token[] xformOpOrder = ["xformOp:translate"]

            def Mesh "Crown"
            {
                int[] faceVertexCounts = [3]
                int[] faceVertexIndices = [0, 1, 2]
                point3f[] points = [(0, 0, 0), (2, 0, 0), (0, 2, 0)]
            }
        }
    }
}
"#;

#[test]
fn a_point_instancer_places_its_prototypes() {
    let scene = scene_of(INSTANCER);
    // Its prototypes are its own, not drawn in place beneath it.
    assert_eq!(paths(&scene), ["/Forest"]);
    let NodeKind::Instancer(forest) = &scene.nodes[0].kind else {
        panic!("an instancer")
    };
    assert_eq!(forest.prototypes, [0, 1]);
    assert_eq!(
        scene.prototypes[0].path(0).text(),
        "/Forest/Prototypes/Pine"
    );
    assert_eq!(
        scene.prototypes[1].path(1).text(),
        "/Forest/Prototypes/Oak/Crown"
    );
    assert_eq!(
        scene.prototypes[1].nodes[0].transform,
        Matrix4d::translation(0.0, 1.0, 0.0)
    );
    assert_eq!(forest.shown(), [true, true, false]);
    // Scale, then turn (a quarter turn about y), then move.
    let placed = forest.transforms()[1].transform_point([1.0, 0.0, 0.0]);
    assert!(
        (placed[0] - 10.0).abs() < 1e-3 && placed[1].abs() < 1e-3 && (placed[2] + 2.0).abs() < 1e-3,
        "{placed:?}"
    );
}

#[test]
fn a_point_instancer_writes_and_reads_back() {
    let scene = scene_of(INSTANCER);
    for again in round_trips(&scene) {
        assert_eq!(again.nodes, scene.nodes);
        assert_eq!(again.prototypes, scene.prototypes);
    }
}

#[test]
fn a_bad_instancer_reads_as_a_group_and_says_why() {
    let scene = scene_of(
        br#"#usda 1.0
def PointInstancer "Broken"
{
    point3f[] positions = [(0, 0, 0)]
    int[] protoIndices = [3]
}
"#,
    );
    assert!(
        scene
            .warnings
            .iter()
            .any(|w| w.starts_with("/Broken: protoIndices")),
        "{:?}",
        scene.warnings
    );
}

#[test]
fn a_prototype_holding_itself_is_refused() {
    let scene = scene_of(
        br#"#usda 1.0
def PointInstancer "Loop"
{
    point3f[] positions = [(0, 0, 0)]
    int[] protoIndices = [0]
    rel prototypes = </Loop/Prototypes/Again>

    def Scope "Prototypes"
    {
        def PointInstancer "Again"
        {
            point3f[] positions = [(0, 0, 0)]
            int[] protoIndices = [0]
            rel prototypes = </Loop/Prototypes/Again>
        }
    }
}
"#,
    );
    let said = "/Loop/Prototypes/Again: its prototype /Loop/Prototypes/Again holds itself";
    assert!(
        scene.warnings.iter().any(|w| w.starts_with(said)),
        "{:?}",
        scene.warnings
    );
    assert!(kinogaki_usd::usd::scene::author(&scene).is_ok());
}

/// Two instanceable prims showing the root prim `name`, which holds two
/// quads, as an instanced stage OpenUSD flattened holds its prototypes.
fn flattened_instances(name: &str) -> String {
    format!(
        r#"#usda 1.0
over "{name}"
{{
    def Scope "Geom"
    {{
        def Mesh "Render"
        {{
            int[] faceVertexCounts = [4, 4]
            int[] faceVertexIndices = [0, 1, 2, 3, 1, 4, 5, 2]
            point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0), (2, 0, 0), (2, 1, 0)]
        }}
    }}
}}

def Xform "World"
{{
    def Xform "Left" (
        instanceable = true
        add references = </{name}>
    )
    {{
    }}

    def Xform "Right" (
        instanceable = true
        add references = </{name}>
    )
    {{
    }}
}}
"#
    )
}

#[test]
fn a_files_own_flattened_prototypes_are_its_prims_and_read_once() {
    // A prototype the reader made is told by what the flattening made, so
    // the file's own `Flattened_Prototype_1` is a prim like any other.
    for name in ["Flattened_Prototype_1", "Flattened_Prototype_7", "Proto"] {
        let scene = scene_of(flattened_instances(name).as_bytes());
        assert_eq!(paths(&scene), ["/World", "/World/Left", "/World/Right"]);
        assert_eq!(scene.prototypes.len(), 1, "{name}");
        let prototype = &scene.prototypes[0];
        // Named by its source instance, a prim of the stage.
        assert_eq!(prototype.path(1).text(), "/World/Left/Geom/Render");
        let NodeKind::Mesh(mesh) = &prototype.nodes[1].kind else {
            panic!("a mesh")
        };
        assert_eq!(mesh.face_vertex_counts.len(), 2, "{name}: two quads once");
        let instances = (scene.nodes.iter()).filter(|n| n.instance == Some(0));
        assert_eq!(instances.count(), 2);
    }
}
