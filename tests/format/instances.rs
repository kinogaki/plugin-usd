//! Instances written as USD's instancing and read back: several as a
//! point instancer, one as an instanceable prim, values several items
//! show as instances of one prototype, each prototype written once; the
//! instances' materials, maps, placements, ids and colors where they were;
//! a file's own point instancer and instanceable prims through import,
//! export and import again; a prototype's hidden prims, and bindings on
//! subsets of other families than materialBind, left out.
use crate::support::{Scratch, materials, metres, read_file, square, write_file};
use kinogaki_studio_api::geometry::{
    Attribute, AttributeType, Attributes, Column, Domain, Meaning, Text, Values, Vec3, Xform, names,
};
use kinogaki_studio_api::network::{NetworkSeed, NewNode};
use kinogaki_studio_api::standard::{self, ImportedMaterial, Instances, Materials};
use kinogaki_studio_api::{Data, FileItem, Imported};
use std::path::Path;

/// A material of no network but its OpenPBR Surface.
fn plain(key: &str, name: &str) -> ImportedMaterial {
    let mut inside = NetworkSeed::default();
    let surface = NewNode::materialx("ND_open_pbr_surface_surfaceshader").unwrap();
    inside.node(surface.displayed());
    ImportedMaterial {
        key: key.into(),
        name: name.into(),
        inside,
        standard: None,
        double_sided: true,
    }
}

/// Red and Blue, keyed `/Looks/Red` and `/Looks/Blue`.
fn looks() -> FileItem {
    let materials = vec![plain("/Looks/Red", "Red"), plain("/Looks/Blue", "Blue")];
    FileItem::top("Looks", standard::materials().wrap(Materials { materials }))
}

/// `count` placements a metre apart along X.
fn row(count: usize) -> Column<Xform> {
    (0..count)
        .map(|i| Xform::translation(Vec3::new(i as f64, 0.0, 0.0)))
        .collect()
}

/// The Instance attribute `name` naming `texts` (`""`: none).
fn texts(name: &str, texts: &[&str]) -> Attributes {
    let text = Attribute::text(Text::from_strs(texts.iter().copied()));
    Attributes::new()
        .with(Domain::Instance, name, text)
        .unwrap()
}

/// The instances item `name` holds in `read`.
fn instances<'a>(read: &'a Imported, name: &str) -> &'a Instances {
    let item = read.items.iter().find(|i| i.name == name);
    let data = item.and_then(|i| i.data.as_ref());
    data.and_then(|d| standard::instances().get(d))
        .unwrap_or_else(|| panic!("{name} holds no instances"))
}

/// The material each instance of `value` names, by its material's name.
fn named(read: &Imported, value: &Instances) -> Vec<Option<String>> {
    let text = value.attributes().text(Domain::Instance, names::MATERIAL);
    (0..value.len())
        .map(|i| {
            let key = text.and_then(|t| t.get(i))?;
            let found = materials(read).materials.iter().find(|m| m.key == key);
            found.map(|m| m.name.clone())
        })
        .collect()
}

/// The layer written at `path`, as text.
fn layer(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn several_instances_are_a_point_instancer_of_one_prototype_written_once() {
    let dir = Scratch::new("instancer");
    let path = dir.join("row.usda");
    let leaf = standard::geometry().wrap(square(""));
    // The third turned a quarter about Z and scaled (1, 2, 0.5).
    let mut at = row(3).to_vec();
    let (x, y, z) = (
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(-2.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 0.5),
    );
    at[2] = Xform::from_axes(x, y, z, Vec3::new(2.0, 0.0, 0.0));
    let attributes = texts(
        names::MATERIAL,
        &["/Looks/Red", "/Looks/Blue", "/Looks/Red"],
    );
    let ids = Attribute::tuples(vec![7_i64, 8, 9], Meaning::None).unwrap();
    let cd = Attribute::tuples(vec![[0.25_f32, 0.5, 0.75]; 3], Meaning::Color).unwrap();
    let attributes = (attributes.with(Domain::Instance, names::ID, ids))
        .and_then(|a| a.with(Domain::Instance, names::CD, cd))
        .unwrap();
    let value = Instances::new(vec![leaf], at.clone().into(), None, attributes).unwrap();
    let items = [
        looks(),
        FileItem::top("Row", standard::instances().wrap(value)),
    ];
    let said = write_file(&path, &items, metres());
    assert!(said.is_empty(), "{said:?}");
    let text = layer(&path);
    assert_eq!(text.matches("def PointInstancer").count(), 1, "{text}");
    assert_eq!(
        text.matches("point3f[] points").count(),
        1,
        "the square once"
    );
    assert!(text.contains("class \"Prototypes\""), "{text}");
    assert!(text.contains("quatf[] orientationsf"), "{text}");
    assert_eq!(
        text.matches("instanceable = true").count(),
        2,
        "a prim for each binding"
    );
    let read = read_file(&path, metres(), false);
    let value = instances(&read, "Row");
    assert_eq!((value.len(), value.prototypes().len()), (3, 1));
    let red = Some("Red".to_owned());
    assert_eq!(
        named(&read, value),
        [red.clone(), Some("Blue".to_owned()), red]
    );
    for (got, want) in value.xforms().iter().zip(&at) {
        assert!(close(got, want), "{got:?} {want:?}");
    }
    let ids = value
        .attributes()
        .tuples::<i64>(Domain::Instance, names::ID);
    assert_eq!(ids.unwrap(), [7, 8, 9]);
    let cd = value
        .attributes()
        .tuples::<[f32; 3]>(Domain::Instance, names::CD);
    assert_eq!(cd.unwrap()[1], [0.25, 0.5, 0.75]);
}

/// An instance whose `material_map` renders its prototype's Red as Blue
/// shows a prototype written in Blue; the others share the one in Red.
#[test]
fn a_material_map_shows_a_prototype_written_in_its_materials() {
    let dir = Scratch::new("mapped");
    let path = dir.join("mapped.usda");
    let leaf = standard::geometry().wrap(square("/Looks/Red"));
    let ty = AttributeType {
        array: true,
        ..AttributeType::TEXT
    };
    let pairs = Values::Text(Text::from_strs(["/Looks/Red", "/Looks/Blue"]));
    let map = Attribute::arrays(ty, vec![0, 0, 2, 2].into(), pairs).unwrap();
    let attributes = (Attributes::new())
        .with(Domain::Instance, names::MATERIAL_MAP, map)
        .unwrap();
    let value = Instances::new(vec![leaf], row(3), None, attributes).unwrap();
    let items = [
        looks(),
        FileItem::top("Row", standard::instances().wrap(value)),
    ];
    write_file(&path, &items, metres());
    let read = read_file(&path, metres(), false);
    let value = instances(&read, "Row");
    assert_eq!((value.len(), value.prototypes().len()), (3, 2));
    let shown: Vec<String> = (0..3)
        .map(|i| {
            let prototype = &value.prototypes()[value.prototype_index(i)];
            let polygons = standard::geometry().get(prototype).unwrap();
            let key = polygons
                .attributes()
                .text(Domain::Primitive, names::MATERIAL);
            let key = key.unwrap().get(0).unwrap();
            let found = materials(&read).materials.iter().find(|m| m.key == key);
            found.unwrap().name.clone()
        })
        .collect();
    assert_eq!(shown, ["Red", "Blue", "Red"]);
}

/// One instance is an instanceable prim; a value two items show is written
/// once and both are instances of it, as two instances of one prototype
/// are; an instance's material binds stronger than its prototype's own.
#[test]
fn one_instance_and_shared_values_are_instanceable_prims_of_one_prototype() {
    let dir = Scratch::new("instanceable");
    let path = dir.join("pieces.usda");
    let red = standard::geometry().wrap(square("/Looks/Red"));
    let one = |material: &str| {
        let attributes = texts(names::MATERIAL, &[material]);
        let value = Instances::new(vec![red.clone()], row(1), None, attributes).unwrap();
        standard::instances().wrap(value)
    };
    let plain = standard::geometry().wrap(square(""));
    let mut copy = FileItem::top("Copy", plain.clone());
    copy.placement = Xform::translation(Vec3::new(0.0, 3.0, 0.0));
    let items = [
        looks(),
        FileItem::top("BishopL", one("/Looks/Blue")),
        FileItem::top("BishopR", one("")),
        FileItem::top("Original", plain),
        copy,
    ];
    write_file(&path, &items, metres());
    let text = layer(&path);
    assert_eq!(text.matches("instanceable = true").count(), 4, "{text}");
    assert_eq!(
        text.matches("point3f[] points").count(),
        2,
        "each value once"
    );
    assert_eq!(text.matches("strongerThanDescendants").count(), 1, "{text}");
    let read = read_file(&path, metres(), false);
    let (left, right) = (instances(&read, "BishopL"), instances(&read, "BishopR"));
    assert!(left.prototypes()[0].ptr_eq(&right.prototypes()[0]));
    assert_eq!(named(&read, left), [Some("Blue".to_owned())]);
    assert_eq!(named(&read, right), [None]);
    let (original, copy) = (instances(&read, "Original"), instances(&read, "Copy"));
    assert!(original.prototypes()[0].ptr_eq(&copy.prototypes()[0]));
    let placed = read.items.iter().find(|i| i.name == "Copy").unwrap();
    assert_eq!(placed.placement.origin(), Vec3::new(0.0, 3.0, 0.0));
}

/// Instances a point instancer can't place (one sheared) are each an
/// instanceable prim of their own, placed by its matrix.
#[test]
fn a_shear_leaves_its_instances_instanceable_prims_of_their_own() {
    let dir = Scratch::new("sheared");
    let path = dir.join("sheared.usda");
    let leaf = standard::geometry().wrap(square(""));
    let sheared = Xform::from_rows([
        [1.0, 0.5, 0.0, 2.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ]);
    let at = vec![Xform::IDENTITY, sheared];
    let value = Instances::new(vec![leaf], at.into(), None, Attributes::new()).unwrap();
    let items = [FileItem::top("Row", standard::instances().wrap(value))];
    write_file(&path, &items, metres());
    let text = layer(&path);
    assert!(!text.contains("PointInstancer"), "{text}");
    let read = read_file(&path, metres(), false);
    let held: Vec<(&FileItem, &Instances)> = (read.items.iter())
        .filter_map(|i| Some((i, standard::instances().get(i.data.as_ref()?)?)))
        .collect();
    assert_eq!(held.len(), 2);
    assert!(held[0].1.prototypes()[0].ptr_eq(&held[1].1.prototypes()[0]));
    assert!(close(&held[1].0.placement, &sheared));
}

/// Whether `a` and `b` are the same placement to single precision.
fn close(a: &Xform, b: &Xform) -> bool {
    let rows = a.rows().iter().zip(b.rows());
    rows.flat_map(|(a, b)| a.iter().zip(b))
        .all(|(a, b)| (a - b).abs() < 1e-6)
}

/// A file's point instancer of eight and two instanceable prims of one
/// prototype come back from an export as they were imported: as many
/// instances and prototypes, the prototypes' points once each.
#[test]
fn a_files_instancing_survives_import_export_and_import() {
    let dir = Scratch::new("round");
    let source = dir.join("set.usda");
    std::fs::write(&source, SET).unwrap();
    let first = read_file(&source, metres(), false);
    let exported = dir.join("exported.usda");
    write_file(&exported, &first.items, metres());
    let again = read_file(&exported, metres(), false);
    for read in [&first, &again] {
        let pawns = instances(read, "Pawns");
        assert_eq!((pawns.len(), pawns.prototypes().len()), (8, 1));
        let (left, right) = (instances(read, "BishopL"), instances(read, "BishopR"));
        assert!(left.prototypes()[0].ptr_eq(&right.prototypes()[0]));
        assert_eq!(named(read, right), [Some("Black".to_owned())]);
        assert_eq!(held_points(read), 4 + 3);
    }
    let text = layer(&exported);
    assert_eq!(text.matches("point3f[] points").count(), 2, "{text}");
}

/// The points every value of `read` holds, each prototype once.
fn held_points(read: &Imported) -> usize {
    fn points(data: &Data, seen: &mut Vec<Data>) -> usize {
        if seen.iter().any(|s| s.ptr_eq(data)) {
            return 0;
        }
        seen.push(data.clone());
        match (
            standard::geometry().get(data),
            standard::instances().get(data),
        ) {
            (Some(g), _) => g.positions().len(),
            (_, Some(i)) => i.prototypes().iter().map(|p| points(p, seen)).sum(),
            _ => 0,
        }
    }
    let mut seen = Vec::new();
    let held = read.items.iter().filter_map(|i| i.data.as_ref());
    held.map(|d| points(d, &mut seen)).sum()
}

/// A pawn placed eight times by a point instancer, and a bishop two
/// instanceable prims reference, bound to Black on the instance.
const SET: &str = r#"#usda 1.0
(
    defaultPrim = "Set"
    metersPerUnit = 1
    upAxis = "Y"
)

def Xform "Set"
{
    def PointInstancer "Pawns"
    {
        point3f[] positions = [(0, 0, 0), (1, 0, 0), (2, 0, 0), (3, 0, 0), (4, 0, 0), (5, 0, 0), (6, 0, 0), (7, 0, 0)]
        int[] protoIndices = [0, 0, 0, 0, 0, 0, 0, 0]
        rel prototypes = </Set/Pawns/Pawn>

        def Xform "Pawn"
        {
            def Mesh "Body"
            {
                int[] faceVertexCounts = [4]
                int[] faceVertexIndices = [0, 1, 2, 3]
                point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)]
                uniform token subdivisionScheme = "none"
            }
        }
    }

    def Xform "BishopL" (
        instanceable = true
        prepend references = </Assets/Bishop>
    )
    {
    }

    def Xform "BishopR" (
        prepend apiSchemas = ["MaterialBindingAPI"]
        instanceable = true
        prepend references = </Assets/Bishop>
    )
    {
        rel material:binding = </Set/Looks/Black>
    }

    def Scope "Looks"
    {
        def Material "Black"
        {
        }
    }
}

class "Assets"
{
    def Xform "Bishop"
    {
        def Mesh "Body"
        {
            int[] faceVertexCounts = [3]
            int[] faceVertexIndices = [0, 1, 2]
            point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
            uniform token subdivisionScheme = "none"
        }
    }
}
"#;

/// An invisible prim inside a prototype shows nothing, as in USD; a
/// subset's binding counts in the materialBind family alone.
#[test]
fn a_prototypes_invisible_prims_and_other_families_bindings_show_nothing() {
    let dir = Scratch::new("hidden");
    let source = dir.join("hidden.usda");
    std::fs::write(&source, HIDDEN).unwrap();
    let read = read_file(&source, metres(), false);
    let value = instances(&read, "Shown");
    let polygons = standard::geometry().get(&value.prototypes()[0]).unwrap();
    assert_eq!(polygons.positions().len(), 3, "the hidden mesh left out");
    let named = polygons
        .attributes()
        .text(Domain::Primitive, names::MATERIAL);
    assert!(
        named.is_none_or(|t| t.get(0).is_none()),
        "a partition's binding ignored"
    );
}

/// An instance of an asset holding a hidden mesh and a mesh whose one
/// face a subset of another family than materialBind binds.
const HIDDEN: &str = r#"#usda 1.0
(
    defaultPrim = "Set"
    metersPerUnit = 1
    upAxis = "Y"
)

def Xform "Set"
{
    def Xform "Shown" (
        instanceable = true
        prepend references = </Assets/Asset>
    )
    {
    }

    def Scope "Looks"
    {
        def Material "Red"
        {
        }
    }
}

class "Assets"
{
    def Xform "Asset"
    {
        def Mesh "Seen"
        {
            int[] faceVertexCounts = [3]
            int[] faceVertexIndices = [0, 1, 2]
            point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
            uniform token subdivisionScheme = "none"

            def GeomSubset "front" (
                prepend apiSchemas = ["MaterialBindingAPI"]
            )
            {
                uniform token elementType = "face"
                uniform token familyName = "parts"
                int[] indices = [0]
                rel material:binding = </Set/Looks/Red>
            }
        }

        def Mesh "Hidden"
        {
            int[] faceVertexCounts = [4]
            int[] faceVertexIndices = [0, 1, 2, 3]
            point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)]
            uniform token subdivisionScheme = "none"
            token visibility = "invisible"
        }
    }
}
"#;
