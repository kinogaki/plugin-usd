//! Polygons split into prims by their `name` paths, the hierarchy they came
//! from, each prim holding its faces alone; off, one Mesh.
use crate::support::{Scratch, metres, names_of, read_file, write_file};
use kinogaki_studio_api::geometry::{
    Attribute, Attributes, Domain, Geometry, Primitives, Text, names,
};
use kinogaki_studio_api::{FileFormat, FileItem, FormatOptions, Value, standard};
use kinogaki_usd::Usd;

/// Three squares named by `paths`, with a material each on the last.
fn squares(paths: [&str; 3]) -> FileItem {
    let mut positions = Vec::new();
    let mut corners = Vec::new();
    for k in 0..3u32 {
        let x = 3.0 * k as f64;
        positions.extend([
            [x, 0.0, 0.0],
            [x + 1.0, 0.0, 0.0],
            [x + 1.0, 1.0, 0.0],
            [x, 1.0, 0.0],
        ]);
        corners.extend([4 * k, 4 * k + 1, 4 * k + 2, 4 * k + 3]);
    }
    let table = Primitives::of_polygons(12, vec![0, 4, 8, 12].into(), corners.into()).unwrap();
    let attributes = Attributes::new()
        .with(
            Domain::Primitive,
            names::NAME,
            Attribute::text(Text::from_strs(paths)),
        )
        .unwrap();
    let value = Geometry::new(positions.into(), table, attributes).unwrap();
    FileItem::top("Camera", standard::geometry().wrap(value))
}

#[test]
fn polygons_split_into_prims_by_their_name_paths() {
    let scratch = Scratch::new("usd-split");
    let path = scratch.join("split.usda");
    let said = write_file(
        &path,
        &[squares(["/Lens/Ring 2", "/Lens/Ring 1", "/Body"])],
        metres(),
    );
    assert_eq!(said, Vec::<String>::new());
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("def Mesh").count(), 3, "{text}");
    for prim in [
        "def Xform \"Lens\"",
        "def Mesh \"Ring_2\"",
        "def Mesh \"Ring_1\"",
        "def Mesh \"Body\"",
    ] {
        assert!(text.contains(prim), "no {prim}: {text}");
    }
    assert!(
        !text.contains("primvars:name"),
        "the path is the prim: {text}"
    );
    let read = read_file(&path, metres(), false);
    let names = names_of(&read);
    assert!(
        names.contains(&"Ring_2") && names.contains(&"Body"),
        "{names:?}"
    );
    let ring = read.items.iter().find(|i| i.name == "Ring_2").unwrap();
    let parent = &read.items[ring.parent.expect("under Lens")];
    assert_eq!(parent.name, "Lens");
    let held = standard::geometry()
        .get(ring.data.as_ref().unwrap())
        .unwrap();
    assert_eq!(held.primitives().len(), 1, "its faces alone");
}

#[test]
fn one_name_stays_one_prim_and_the_option_turns_it_off() {
    let scratch = Scratch::new("usd-split-off");
    let one = scratch.join("one.usda");
    write_file(&one, &[squares(["/Body", "/Body", "/Body"])], metres());
    let text = std::fs::read_to_string(&one).unwrap();
    assert_eq!(text.matches("def Mesh").count(), 1, "{text}");
    let off = scratch.join("off.usda");
    let writing =
        kinogaki_studio_api::Writing::new(metres(), &kinogaki_studio_api_testing::Unconverted);
    let options = FormatOptions::from([("split_by_name".to_owned(), Value::Bool(false))]);
    Usd.writer()
        .unwrap()
        .write(&off, &[squares(["/a/b", "/a/c", "/d"])], &writing, &options)
        .unwrap();
    let text = std::fs::read_to_string(&off).unwrap();
    assert_eq!(text.matches("def Mesh").count(), 1, "{text}");
}
