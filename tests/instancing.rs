//! Instancing: instanceable prims share prototypes, point instancers place
//! theirs, and both write back.

mod common;

use common::{node, paths, round_trips, scene_of};
use kinogaki_usd::usd::scene::{Matrix4d, NodeKind};

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
    assert_eq!(prototype.path(0).text(), "/Flattened_Prototype_1/Part");
    assert!(matches!(prototype.nodes[0].kind, NodeKind::Mesh(_)));
}

#[test]
fn instances_write_as_copies() {
    let scene = scene_of(INSTANCEABLE);
    for again in round_trips(&scene) {
        assert!(again.prototypes.is_empty());
        assert_eq!(
            paths(&again),
            [
                "/Asset",
                "/Asset/Part",
                "/Shelf",
                "/Shelf/One",
                "/Shelf/One/Part",
                "/Shelf/Two",
                "/Shelf/Two/Part"
            ]
        );
        let copy = common::mesh(&again, "/Shelf/Two/Part");
        let NodeKind::Mesh(part) = &scene.prototypes[0].nodes[0].kind else {
            panic!("a mesh")
        };
        assert_eq!(copy.points, part.points);
        // The copy binds what its instance bound.
        assert_eq!(node(&again, "/Shelf/Two/Part").material, Some(0));
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
