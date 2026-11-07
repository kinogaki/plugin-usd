//! The format's budgets (render-agnostic PLAN, wave 9), measured in release
//! on the benchmark grid of 700,569 quads with texture coordinates (the
//! plugin's README's): read from a usdc in at most 40 ms and from a usda in
//! at most 80 ms into the standard values, 100 MB of textures in a usdz
//! adding at most 20 ms, written from the standard values as a usdz in at
//! most 60 ms; Pixar's Kitchen Set (not redistributable: the path in
//! `KINOGAKI_KITCHEN_SET`) imported in at most 250 ms. Best of ten, on
//! workers a thread a core, as Studio installs them.
use crate::support::{Scratch, metres, read_file, write_file};
use kinogaki_studio_api::FileItem;
use kinogaki_studio_api::geometry::image::{EmbeddedFile, ImageFile, ImageSource};
use kinogaki_studio_api::geometry::parallel::{self, Workers};
use kinogaki_studio_api::geometry::{
    Attribute, Attributes, Domain, Geometry, Meaning, Primitives, Text, names,
};
use kinogaki_studio_api::image_files::image_node;
use kinogaki_studio_api::network::{NetworkSeed, NewNode};
use kinogaki_studio_api::standard::{self, ImportedMaterial, Materials};
use std::ops::Range;
use std::panic::Location;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// The workers a host installs: the calling thread and scoped helpers, one
/// a core, taking ranges from a shared counter.
struct Pool(usize);

impl Workers for Pool {
    fn run(
        &self,
        _: &'static Location<'static>,
        len: usize,
        minimum: usize,
        work: &(dyn Fn(Range<usize>) -> bool + Sync),
    ) {
        let step = minimum.max(1);
        let threads = self.0.min(len.div_ceil(step));
        if threads <= 1 {
            if len > 0 {
                work(0..len);
            }
            return;
        }
        let (next, stop) = (AtomicUsize::new(0), AtomicBool::new(false));
        let take = || {
            while !stop.load(Ordering::Relaxed) {
                let start = next.fetch_add(step, Ordering::Relaxed);
                if start >= len {
                    break;
                }
                if !work(start..(start + step).min(len)) {
                    stop.store(true, Ordering::Relaxed);
                }
            }
        };
        std::thread::scope(|scope| {
            (1..threads).for_each(|_| _ = scope.spawn(take));
            take();
        });
    }
}

/// The host's workers, installed once.
fn workers() {
    let threads = std::thread::available_parallelism().map_or(1, usize::from);
    parallel::install(Box::leak(Box::new(Pool(threads))));
}

/// Quads along each side of the grid: 837² = 700,569 faces.
const SIDE: usize = 837;

/// The grid, its faces naming material `key`.
fn grid(key: &str) -> Geometry {
    let points = SIDE + 1;
    let positions: Vec<[f64; 3]> = (0..points * points)
        .map(|i| [(i % points) as f64 * 0.01, (i / points) as f64 * 0.01, 0.0])
        .collect();
    let mut vertices = Vec::with_capacity(SIDE * SIDE * 4);
    for row in 0..SIDE {
        for column in 0..SIDE {
            let corner = (row * points + column) as u32;
            let up = points as u32;
            vertices.extend([corner, corner + 1, corner + up + 1, corner + up]);
        }
    }
    let offsets: Vec<u32> = (0..=SIDE * SIDE).map(|f| (f * 4) as u32).collect();
    let table = Primitives::of_polygons(positions.len(), offsets.into(), vertices.into()).unwrap();
    let corners = [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let uv: Vec<[f32; 2]> = (0..SIDE * SIDE * 4).map(|c| corners[c % 4]).collect();
    let material = Text::new(vec![0; SIDE * SIDE].into(), [key.into()].into()).unwrap();
    let attributes = Attributes::new()
        .with(
            Domain::Vertex,
            names::UV,
            Attribute::tuples(uv, Meaning::TextureCoord).unwrap(),
        )
        .and_then(|a| {
            a.with(
                Domain::Primitive,
                names::MATERIAL,
                Attribute::text(material),
            )
        })
        .unwrap();
    Geometry::new(positions.into(), table, attributes).unwrap()
}

/// The grid's items, its material textured with `megabytes` of bytes.
fn items(megabytes: usize) -> Vec<FileItem> {
    let mut inside = NetworkSeed::default();
    let surface = inside.node(
        NewNode::materialx("ND_UsdPreviewSurface_surfaceshader")
            .unwrap()
            .displayed(),
    );
    let texture = inside.node(
        NewNode::materialx("ND_UsdUVTexture_23")
            .unwrap()
            .named("Texture"),
    );
    let bytes: Vec<u8> = (0..megabytes << 20)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let file = ImageFile::Embedded(EmbeddedFile::new("checker.png", bytes));
    let image = inside
        .node(image_node::reading(&ImageSource::new(file, Default::default())).named("checker"));
    inside.wire(image, image_node::IMAGE, texture, "file");
    inside.wire(texture, "rgb", surface, "diffuseColor");
    let material = ImportedMaterial {
        key: "/Looks/Checker".into(),
        name: "Checker".into(),
        inside,
        standard: None,
        double_sided: true,
    };
    let looks = Materials {
        materials: vec![material],
        images: Vec::new(),
    };
    vec![
        FileItem::top("Looks", standard::materials().wrap(looks)),
        FileItem::top("Grid", standard::geometry().wrap(grid("/Looks/Checker"))),
    ]
}

/// The best of ten runs of `run`.
fn best(mut run: impl FnMut()) -> Duration {
    (0..10)
        .map(|_| {
            let start = Instant::now();
            run();
            start.elapsed()
        })
        .min()
        .unwrap()
}

/// The best time `path` reads into the standard values.
fn read_time(path: &Path) -> Duration {
    best(|| {
        let read = read_file(path, metres(), false);
        assert!(read.items.len() >= 2);
    })
}

/// The benchmark grid as the format's own model authors it (the plugin's
/// README's: `st` indexed, four values), textured with `megabytes` of bytes
/// in a usdz.
fn authored(path: &Path, megabytes: usize) {
    use kinogaki_usd::usd::package::Bytes;
    use kinogaki_usd::usd::scene::{
        Asset, AssetSource, Interpolation, Mesh, Node, NodeKind, Primvar, Scene,
    };
    use kinogaki_usd::usd::sdf::{Array, Path as Prim, Token, Value as Usd};
    use kinogaki_usd::usd::shade::{Material, Port, ShadeKind, ShadeNode, Source};
    use std::sync::Arc;
    let points = SIDE + 1;
    let at =
        (0..points * points).map(|i| [(i % points) as f32 * 0.01, (i / points) as f32 * 0.01, 0.0]);
    let mut indices = Vec::with_capacity(SIDE * SIDE * 4);
    for row in 0..SIDE {
        for column in 0..SIDE {
            let corner = (row * points + column) as i32;
            indices.extend([
                corner,
                corner + 1,
                corner + points as i32 + 1,
                corner + points as i32,
            ]);
        }
    }
    let corners = vec![[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let st = Primvar {
        name: Token::new("st"),
        type_name: Token::new("texCoord2f[]"),
        interpolation: Interpolation::FaceVarying,
        element_size: 1,
        values: Usd::Array(Array::Vec2f(Arc::new(corners))),
        indices: Some(Arc::new(
            (0..SIDE * SIDE * 4).map(|i| (i % 4) as i32).collect(),
        )),
        color_space: None,
    };
    let mesh = Mesh {
        points: Arc::new(at.collect()),
        face_vertex_counts: Arc::new(vec![4; SIDE * SIDE]),
        face_vertex_indices: Arc::new(indices),
        left_handed: false,
        primvars: vec![st],
        subdivision: Default::default(),
        subsets: Vec::new(),
        double_sided: false,
    };
    let mut scene = Scene::default();
    scene.nodes.push(Node::group("World", None));
    let looks = Prim::parse("/World/Looks/Checker").unwrap();
    let mut material = Material::new(looks.clone());
    let file = Token::new("checker.png");
    let id = |id: &str| {
        let mut port = Port::new("id", "token", Some(Usd::Token(Token::new(id))));
        port.uniform = true;
        port
    };
    let source = |node: usize, port: &str| Source {
        node: Some(node),
        port: Token::new(port),
        output: true,
    };
    let mut diffuse = Port::new("diffuseColor", "color3f", None);
    diffuse.sources.push(source(0, "rgb"));
    let shader = |name: &str, info: Port, inputs, outputs| ShadeNode {
        path: looks.child(name),
        kind: ShadeKind::Shader,
        parent: None,
        info: vec![info],
        inputs,
        outputs,
    };
    material.nodes = vec![
        shader(
            "Texture",
            id("UsdUVTexture"),
            vec![Port::new("file", "asset", Some(Usd::Asset(file.clone())))],
            vec![Port::new("rgb", "float3", None)],
        ),
        shader(
            "Preview",
            id("UsdPreviewSurface"),
            vec![diffuse],
            vec![Port::new("surface", "token", None)],
        ),
    ];
    let mut surface = Port::new("surface", "token", None);
    surface.sources.push(source(1, "surface"));
    material.outputs.push(surface);
    scene.materials.push(material);
    let bytes: Vec<u8> = (0..megabytes << 20)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    scene.assets.push(Asset {
        path: file.clone(),
        authored: file,
        source: AssetSource::Bytes(Bytes::from(bytes)),
    });
    let mut plane = Node::group("Grid", Some(0));
    plane.kind = NodeKind::Mesh(Box::new(mesh));
    plane.material = Some(0);
    scene.nodes.push(plane);
    kinogaki_usd::usd::scene::write_file(&scene, path).unwrap();
}

#[test]
#[ignore = "hand: a release measurement of the grid"]
fn hand_run_the_grid_reads_and_writes_within_its_budgets() {
    workers();
    let dir = Scratch::new("measure");
    let file = |name: &str| dir.path().join(name);
    authored(&file("grid.usdc"), 0);
    authored(&file("grid.usda"), 0);
    authored(&file("grid.usdz"), 0);
    authored(&file("textured.usdz"), 100);
    let (c, a) = (read_time(&file("grid.usdc")), read_time(&file("grid.usda")));
    let (z, t) = (
        read_time(&file("grid.usdz")),
        read_time(&file("textured.usdz")),
    );
    // From the standard values: the grid, its material and texture.
    let plain = items(0);
    let write = best(|| _ = write_file(&file("written.usdz"), &plain, metres()));
    // The grid as this format writes it (`st` one value a corner) and reads.
    write_file(&file("written.usda"), &plain, metres());
    let unindexed = read_time(&file("written.usda"));
    println!(
        "MEASURE usd grid (700,569 quads): usdc {c:.1?}, usda {a:.1?}, usdz {z:.1?}, \
         usdz + 100 MB {t:.1?}, written as usdz {write:.1?}; its written usda {unindexed:.1?}"
    );
    assert!(c <= Duration::from_millis(40), "usdc {c:?}");
    assert!(a <= Duration::from_millis(80), "usda {a:?}");
    assert!(
        t.saturating_sub(z) <= Duration::from_millis(20),
        "100 MB of textures {t:?} against {z:?}"
    );
    assert!(write <= Duration::from_millis(60), "written {write:?}");
}

#[test]
#[ignore = "hand: Pixar's Kitchen Set, not redistributable (KINOGAKI_KITCHEN_SET)"]
fn hand_run_the_kitchen_set_imports_within_its_budget() {
    workers();
    let path =
        std::env::var("KINOGAKI_KITCHEN_SET").expect("KINOGAKI_KITCHEN_SET names Kitchen_set.usd");
    let mut prims = 0;
    let time = best(|| prims = read_file(Path::new(&path), metres(), false).items.len());
    println!("MEASURE usd Kitchen Set: {prims} items in {time:.1?}");
    assert!(prims > 1000, "{prims} items");
    assert!(time <= Duration::from_millis(250), "{time:?}");
}

#[test]
fn the_grid_writes_and_reads_back_whole() {
    let dir = Scratch::new("grid");
    let path = dir.path().join("grid.usdc");
    write_file(&path, &items(0), metres());
    let read = read_file(&path, metres(), false);
    let grid = standard::geometry()
        .get(read.items[2].data.as_ref().unwrap())
        .unwrap();
    assert_eq!(grid.primitives().len(), SIDE * SIDE);
    let uv = grid
        .attributes()
        .tuples::<[f32; 2]>(Domain::Vertex, names::UV)
        .unwrap();
    assert_eq!((uv.len(), uv[6]), (SIDE * SIDE * 4, [1.0, 1.0]));
    let looks = &crate::support::materials(&read).materials[0];
    let material = grid
        .attributes()
        .text(Domain::Primitive, names::MATERIAL)
        .unwrap();
    assert_eq!(material.get(SIDE), Some(looks.key.as_str()));
}
