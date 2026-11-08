//! Primvars read as attributes: a standard attribute's name takes the
//! standard's whole type, its values converted and moved into the project;
//! one no attribute can hold is left out with its prim named, and never
//! fails a read.
use crate::support::{item, metres};
use kinogaki_studio_api::geometry::{Domain, Geometry, names};
use kinogaki_studio_api::{FileFormat, Imported, Input, Purpose, Reading, standard};
use kinogaki_studio_api_testing::Unconverted;
use kinogaki_usd::Usd;

/// A quad of a centimetre stage holding `primvars`.
fn quad(primvars: &str) -> String {
    format!(
        r#"#usda 1.0
(
    metersPerUnit = 0.01
)
def Xform "World"
{{
    def Mesh "Quad"
    {{
        int[] faceVertexCounts = [4]
        int[] faceVertexIndices = [0, 1, 2, 3]
        point3f[] points = [(0, 0, 0), (100, 0, 0), (100, 100, 0), (0, 100, 0)]
{primvars}
    }}
}}
"#
    )
}

fn read(layer: &str) -> (Imported, Geometry) {
    let reading = Reading::new(Purpose::Objects, metres(), &Unconverted);
    let input = Input::memory("quad.usda", layer.as_bytes());
    let read = Usd.read(&input, &Usd.options(), &reading).unwrap();
    let held = item(&read, "Quad").1.unwrap();
    let quad = standard::geometry().get(held).unwrap().clone();
    (read, quad)
}

#[test]
fn a_rest_primvar_reads_as_rest_positions_moved_into_the_project() {
    // Houdini writes rest positions as single floats, indexed.
    let (read, quad) = read(&quad(
        r#"        float3[] primvars:rest = [(0, 0, 0), (100, 0, 0), (100, 100, 0), (0, 100, 0)] (
            interpolation = "vertex"
        )
        int[] primvars:rest:indices = [0, 1, 2, 3]
        int[] primvars:id = [7, 8, 9, 10] (
            interpolation = "vertex"
        )"#,
    ));
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    let attributes = quad.attributes();
    let rest = attributes.tuples::<[f64; 3]>(Domain::Point, names::REST);
    let rest = rest.expect("rest positions, as precise as positions");
    assert_eq!(rest.len(), 4);
    assert_eq!((rest[1], rest[2]), ([1.0, 0.0, 0.0], [1.0, 1.0, 0.0]));
    let id = attributes.tuples::<[i64; 1]>(Domain::Point, names::ID);
    assert_eq!(id.expect("ids, 64-bit")[..], [[7], [8], [9], [10]]);
}

#[test]
fn a_transform_primvar_reads_as_placements_in_the_project() {
    // A USD matrix of row vectors, its translation 100 cm along X.
    let moved = "1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 100, 0, 0, 1";
    let (read, quad) = read(&quad(&format!(
        r#"        double[] primvars:transform = [{moved}, {moved}, {moved}, {moved}] (
            elementSize = 16
            interpolation = "vertex"
        )"#
    )));
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    let transform = quad
        .attributes()
        .tuples::<[f64; 16]>(Domain::Point, names::TRANSFORM);
    let rows = [
        1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    assert_eq!(transform.expect("placements")[3], rows);
}

#[test]
fn a_primvar_no_attribute_holds_is_left_out_with_its_prim_named() {
    let (read, quad) = read(&quad(
        r#"        float2[] primvars:rest = [(0, 0), (1, 0), (1, 1), (0, 1)] (
            interpolation = "vertex"
        )
        string[] primvars:scale = ["a", "b", "c", "d"] (
            interpolation = "vertex"
        )
        float[] primvars:pscale = [1] (
            interpolation = "constant"
        )
        float[] primvars:weight = [0.5, 0.25, 1, 0] (
            interpolation = "vertex"
        )"#,
    ));
    let said: Vec<&str> = read.warnings.iter().map(|w| w.message.as_str()).collect();
    assert_eq!(
        said,
        [
            "/World/Quad: primvar pscale is constant; pscale isn't held there; left out",
            "/World/Quad: primvar rest is no rest; left out",
            "/World/Quad: primvar scale is no scale; left out",
        ]
    );
    // The rest of the mesh reads.
    let weight = quad
        .attributes()
        .tuples::<[f32; 1]>(Domain::Point, "weight");
    assert_eq!(weight.unwrap()[1], [0.25]);
}
