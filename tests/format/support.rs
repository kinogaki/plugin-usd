//! What the format's tests share: reading and writing through the format
//! alone (its color service converting nothing), and networks compared by
//! what they hold.
use kinogaki_studio_api::color::ColorService;
use kinogaki_studio_api::geometry::{
    Attribute, Attributes, Domain, Geometry, Meaning, Primitives, Text, Units, UpAxis, names,
};
use kinogaki_studio_api::image_files::{IMAGE_NODE, image_source};
use kinogaki_studio_api::network::NetworkSeed;
use kinogaki_studio_api::standard::{self, Materials};
use kinogaki_studio_api::{
    Data, Error, FileFormat, FileItem, FormatOptions, Imported, Input, Purpose, Reading, Value,
    Writing,
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

/// A color service whose working space, `ACEScg`, holds any other space's
/// colors at twice their numbers: what tells colors, which convert, from
/// numbers, which don't.
#[derive(Debug)]
pub struct Doubling;

impl ColorService for Doubling {
    fn working(&self) -> &str {
        "ACEScg"
    }
    fn convert(&self, from: &str, to: &str, rgb: &mut [[f64; 3]]) -> Result<(), Error> {
        let k = match (from == self.working(), to == self.working()) {
            (false, true) => 2.0,
            (true, false) => 0.5,
            _ => 1.0,
        };
        rgb.iter_mut().flatten().for_each(|c| *c *= k);
        Ok(())
    }
    fn revision(&self) -> u64 {
        0
    }
}

/// The file at `path` read into metres with `colors`.
pub fn read_in(path: &Path, colors: &dyn ColorService) -> Imported {
    let reading = Reading::new(Purpose::Objects, metres(), colors);
    Usd.read(&Input::file(path), &FormatOptions::new(), &reading)
        .unwrap()
}

/// `items` written to `path` in metres with `colors`, with what it said.
pub fn write_in(path: &Path, items: &[FileItem], colors: &dyn ColorService) -> Vec<String> {
    let writing = Writing::new(metres(), colors);
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

/// Every material of a read, its libraries' in their items' order.
pub fn materials(read: &Imported) -> Materials {
    let held = (read.items.iter())
        .filter_map(|i| standard::materials().get(i.data.as_ref()?))
        .flat_map(|m| m.materials.iter().cloned());
    Materials {
        materials: held.collect(),
    }
}

/// Each library of a read: its path among the items, and its materials'
/// names.
pub fn libraries(read: &Imported) -> Vec<(String, Vec<String>)> {
    let mut paths: Vec<String> = Vec::with_capacity(read.items.len());
    let mut out = Vec::new();
    for item in &read.items {
        let above = item.parent.map_or("", |p| paths[p].as_str());
        paths.push(format!("{above}/{}", item.name));
        if let Some(held) = item
            .data
            .as_ref()
            .and_then(|d| standard::materials().get(d))
        {
            let names = held.materials.iter().map(|m| m.name.clone()).collect();
            out.push((paths.last().expect("its path").clone(), names));
        }
    }
    out
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

/// What a network holds, its nodes by name (a name met again numbered:
/// `box 2`): each one's type, its values (an Image node's by what it reads),
/// and its wires by the names they join; the shown node's name.
pub type Shape = (
    BTreeMap<String, (String, Vec<(String, String)>)>,
    BTreeSet<(String, String, String, String)>,
    Option<String>,
);

pub fn shape(seed: &NetworkSeed) -> Shape {
    let given = |at: usize| seed.nodes[at].name.clone().unwrap_or_default();
    let name = |at: usize| match (0..at).filter(|&k| given(k) == given(at)).count() {
        0 => given(at),
        n => format!("{} {}", given(at), n + 1),
    };
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
