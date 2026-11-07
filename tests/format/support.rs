//! What the format's tests share: reading and writing through the format
//! alone (its color service converting nothing), and networks compared by
//! what they hold.
use kinogaki_studio_api::geometry::{
    Attribute, Attributes, Domain, Geometry, Meaning, Primitives, Text, Units, UpAxis, names,
};
use kinogaki_studio_api::image_files::{IMAGE_NODE, image_source};
use kinogaki_studio_api::network::NetworkSeed;
use kinogaki_studio_api::standard::{self, Materials};
use kinogaki_studio_api::{
    Data, FileFormat, FileItem, FormatOptions, Imported, Input, Purpose, Reading, Value, Writing,
};
use kinogaki_studio_api_testing::Unconverted;
pub use kinogaki_studio_api_testing::format::Scratch;
use kinogaki_usd::Usd;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The stage rooted at the layer `path` on disk, reading the files in its
/// folder and below as Import does.
pub fn stage(path: &Path) -> kinogaki_usd::usd::Stage {
    use kinogaki_studio_api::format::files::Named;
    use kinogaki_usd::usd::{Stage, StageOptions};
    let reading = Reading::new(Purpose::Objects, metres(), &Unconverted);
    let disk = Named::new(&Input::file(path), &FormatOptions::new(), &reading);
    let options = StageOptions {
        disk: Some(std::sync::Arc::new(disk)),
        ..StageOptions::default()
    };
    Stage::open(path, &options).unwrap()
}

/// Metres, Y up.
pub fn metres() -> Units {
    Units::new(1.0, UpAxis::Y).unwrap()
}

/// The file at `path` read into `units`, embedding textures when `embed`.
pub fn read_file(path: &Path, units: Units, embed: bool) -> Imported {
    let options = FormatOptions::from([("embed".to_owned(), Value::Bool(embed))]);
    read_with(path, units, &options)
}

/// The file at `path` read into `units` with `options`.
pub fn read_with(path: &Path, units: Units, options: &FormatOptions) -> Imported {
    let reading = Reading::new(Purpose::Objects, units, &Unconverted);
    Usd.read(&Input::file(path), options, &reading).unwrap()
}

/// `items` written to `path` in `units`, with what it said.
pub fn write_file(path: &Path, items: &[FileItem], units: Units) -> Vec<String> {
    let writing = Writing::new(units, &Unconverted);
    let writer = Usd.writer().unwrap();
    let said = writer
        .write(path, items, &writing, &FormatOptions::new())
        .unwrap();
    said.into_iter().map(|d| d.message).collect()
}

/// The fixture `name` (tests/fixtures).
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The materials item of a read.
pub fn materials(read: &Imported) -> &Materials {
    let item = read
        .items
        .first()
        .and_then(|i| i.data.as_ref())
        .expect("a first item");
    standard::materials()
        .get(item)
        .expect("the materials item first")
}

/// The item named `name`, and its value.
pub fn item<'a>(read: &'a Imported, name: &str) -> (&'a FileItem, Option<&'a Data>) {
    let found = read.items.iter().find(|i| i.name == name);
    let found = found.unwrap_or_else(|| panic!("no item {name} in {:?}", names_of(read)));
    (found, found.data.as_ref())
}

pub fn names_of(read: &Imported) -> Vec<&str> {
    read.items.iter().map(|i| i.name.as_str()).collect()
}

/// What a network holds, its nodes by name: each one's type, its values
/// (an Image node's by what it reads), and its wires by the names they
/// join; the shown node's name.
pub type Shape = (
    BTreeMap<String, (String, Vec<(String, String)>)>,
    BTreeSet<(String, String, String, String)>,
    Option<String>,
);

pub fn shape(seed: &NetworkSeed) -> Shape {
    let name = |at: usize| seed.nodes[at].name.clone().unwrap_or_default();
    let nodes = (seed.nodes.iter().enumerate())
        .map(|(at, n)| {
            let values: Vec<(String, String)> = match n.node_type == IMAGE_NODE {
                true => vec![(
                    "reads".into(),
                    format!("{:?}", image_source(&n.parameters).unwrap()),
                )],
                false => (n.parameters.iter())
                    .map(|(k, v)| (k.clone(), format!("{v:?}")))
                    .collect(),
            };
            (name(at), (n.node_type.to_string(), values))
        })
        .collect();
    let wires = (seed.wires.iter())
        .map(|w| (name(w.from), w.output.clone(), name(w.to), w.input.clone()))
        .collect();
    let shown = seed.nodes.iter().position(|n| n.display).map(name);
    (nodes, wires, shown)
}

/// A unit square in its XY plane, its one face showing material `key`, with
/// `uv`, `N` and `Cd`.
pub fn square(key: &str) -> Geometry {
    let positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    let table = Primitives::of_polygons(4, vec![0, 4].into(), vec![0, 1, 2, 3].into()).unwrap();
    let uv = Attribute::tuples(
        vec![[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        Meaning::TextureCoord,
    )
    .unwrap();
    let n = Attribute::tuples(vec![[0.0f32, 0.0, 1.0]; 4], Meaning::Normal).unwrap();
    let cd = Attribute::tuples(vec![[0.5f32, 0.25, 0.75]; 4], Meaning::Color).unwrap();
    let material = Attribute::text(Text::from_strs([key]));
    let attributes = (Attributes::new().with(Domain::Vertex, names::UV, uv))
        .and_then(|a| a.with(Domain::Vertex, names::N, n))
        .and_then(|a| a.with(Domain::Point, names::CD, cd))
        .and_then(|a| a.with(Domain::Primitive, names::MATERIAL, material))
        .unwrap();
    Geometry::new(positions.into(), table, attributes).unwrap()
}
