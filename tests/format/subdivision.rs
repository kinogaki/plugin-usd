//! A mesh's subdivision read, written and read back: the scheme (USD's
//! fallback, Catmull-Clark, where none is authored; none as no setting),
//! the level (ours, Arnold's, Omniverse's), the boundary, face-varying and
//! triangle rules, creases by chain and by edge, corners and holes; normals
//! written only where the mesh doesn't subdivide.
use crate::support::{Scratch, item, metres, read_file, write_file};
use kinogaki_studio_api::geometry::{
    Boundary, Domain, FaceVarying, Geometry, Scheme, Subdivision, Triangles, names,
};
use kinogaki_studio_api::{Imported, standard};

/// Two quads side by side over six points, `extra` authored on the mesh.
fn mesh(name: &str, extra: &str) -> String {
    format!(
        r#"    def Mesh "{name}"
    {{
        int[] faceVertexCounts = [4, 4]
        int[] faceVertexIndices = [0, 3, 4, 1, 1, 4, 5, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (2, 0, 0), (0, 0, 1), (1, 0, 1), (2, 0, 1)]
{extra}
    }}
"#
    )
}

fn layer(meshes: &[String]) -> String {
    let mut text = String::from(
        "#usda 1.0\n(\n    defaultPrim = \"World\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n)\n\ndef Xform \"World\"\n{\n",
    );
    for m in meshes {
        text.push_str(m);
    }
    text.push_str("}\n");
    text
}

fn value<'a>(read: &'a Imported, name: &str) -> &'a Geometry {
    standard::geometry()
        .get(item(read, name).1.unwrap())
        .unwrap()
}

fn crease(value: &Geometry, domain: Domain) -> Vec<f32> {
    let values = value.attributes().tuples::<f32>(domain, names::CREASE);
    values.expect("creases").to_vec()
}

/// The sharpness of `value`'s edge between `a` and `b`.
fn edge(value: &Geometry, a: u32, b: u32) -> f32 {
    let e = value.primitives().edge_of(a, b).expect("the edge");
    crease(value, Domain::Edge)[e as usize]
}

/// Every mesh of the fixture, as Import reads it.
fn fixture(dir: &Scratch) -> Imported {
    let path = dir.join("subdivision.usda");
    let every = r#"        uniform token subdivisionScheme = "catmullClark"
        custom uniform int kinogaki:subdivisionLevel = 3
        token interpolateBoundary = "edgeOnly"
        token faceVaryingLinearInterpolation = "all"
        token triangleSubdivisionRule = "smooth"
        int[] creaseIndices = [0, 1, 2, 3, 4]
        int[] creaseLengths = [3, 2]
        float[] creaseSharpnesses = [2.5, 10]
        int[] cornerIndices = [5]
        float[] cornerSharpnesses = [4]
        int[] holeIndices = [1]"#;
    let per_edge = r#"        uniform token subdivisionScheme = "loop"
        int[] creaseIndices = [0, 1, 2]
        int[] creaseLengths = [3]
        float[] creaseSharpnesses = [1, 2]"#;
    let arnold = r#"        uchar primvars:arnold:subdiv_iterations = 1 (
            interpolation = "constant"
        )"#;
    let omniverse = r#"        custom bool refinementEnableOverride = 1
        custom int refinementLevel = 4"#;
    let not_enabled = r#"        custom int refinementLevel = 4"#;
    let none = r#"        uniform token subdivisionScheme = "none"
        normal3f[] normals = [(0, 1, 0), (0, 1, 0), (0, 1, 0), (0, 1, 0), (0, 1, 0), (0, 1, 0)] (
            interpolation = "vertex"
        )"#;
    let text = layer(&[
        mesh("Every", every),
        mesh("PerEdge", per_edge),
        mesh("Unauthored", ""),
        mesh("Arnold", arnold),
        mesh("Omniverse", omniverse),
        mesh("NotEnabled", not_enabled),
        mesh("Polygons", none),
    ]);
    std::fs::write(&path, text).unwrap();
    read_file(&path, metres(), false)
}

/// What `read` holds of the fixture's meshes, as the format must read them.
fn check(read: &Imported) {
    let every = value(read, "Every");
    let setting = Subdivision::of(every).expect("Catmull-Clark");
    assert_eq!(
        setting,
        Subdivision {
            boundary: Boundary::EdgeOnly,
            face_varying: FaceVarying::All,
            triangles: Triangles::Smooth,
            ..Subdivision::new(Scheme::CatmullClark).with_level(3)
        }
    );
    // Two faces held, the second a hole.
    assert_eq!(every.primitives().len(), 2);
    let holes = every
        .attributes()
        .group(Domain::Primitive, names::SUBDIVISION_HOLE);
    assert_eq!(holes.unwrap().iter().collect::<Vec<_>>(), [1]);
    assert_eq!(
        (edge(every, 0, 1), edge(every, 1, 2), edge(every, 3, 4)),
        (2.5, 2.5, 10.0)
    );
    assert_eq!(edge(every, 1, 4), 0.0);
    assert_eq!(crease(every, Domain::Point), [0.0, 0.0, 0.0, 0.0, 0.0, 4.0]);
    let per_edge = value(read, "PerEdge");
    assert_eq!(Subdivision::of(per_edge).unwrap().scheme, Scheme::Loop);
    assert_eq!((edge(per_edge, 0, 1), edge(per_edge, 1, 2)), (1.0, 2.0));
    let unauthored = Subdivision::of(value(read, "Unauthored")).expect("USD's fallback");
    assert_eq!(unauthored, Subdivision::new(Scheme::CatmullClark));
    let arnold = value(read, "Arnold");
    assert_eq!(Subdivision::of(arnold).unwrap().level, 1);
    assert!(
        arnold
            .attributes()
            .iter()
            .all(|(_, name, _)| !name.contains("subdiv_iterations"))
    );
    assert_eq!(Subdivision::of(value(read, "Omniverse")).unwrap().level, 4);
    let not_enabled = Subdivision::of(value(read, "NotEnabled")).unwrap();
    assert_eq!(not_enabled.level, Subdivision::DEFAULT_LEVEL);
    let polygons = value(read, "Polygons");
    assert_eq!(Subdivision::of(polygons), None);
    assert!(polygons.attributes().get(Domain::Point, names::N).is_some());
}

#[test]
fn every_subdivision_attribute_reads_writes_and_reads_back() {
    let dir = Scratch::new("usd-subdivision");
    let read = fixture(&dir);
    check(&read);
    let written = dir.join("written.usda");
    write_file(&written, &read.items, metres());
    let text = std::fs::read_to_string(&written).unwrap();
    for line in [
        "uniform token subdivisionScheme = \"catmullClark\"",
        "custom uniform int kinogaki:subdivisionLevel = 3",
        "token interpolateBoundary = \"edgeOnly\"",
        "token faceVaryingLinearInterpolation = \"all\"",
        "token triangleSubdivisionRule = \"smooth\"",
        "int[] cornerIndices = [5]",
        "int[] holeIndices = [1]",
        "uniform token subdivisionScheme = \"loop\"",
        "uniform token subdivisionScheme = \"none\"",
    ] {
        assert!(text.contains(line), "{line} in\n{text}");
    }
    // Normals only on the mesh that doesn't subdivide; no primvar of a tag.
    assert_eq!(text.matches("normals").count(), 1, "{text}");
    assert!(!text.contains("primvars:crease") && !text.contains("primvars:subdivision"));
    assert!(!text.contains("GeomSubset \"subdivision_hole\""));
    let back = read_file(&written, metres(), false);
    check(&back);
}
