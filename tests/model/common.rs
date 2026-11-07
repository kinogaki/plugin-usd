//! What the tests share: stages from text and from disk, nodes by path,
//! round trips.
use kinogaki_studio_api::format::files::Named;
use kinogaki_studio_api::{FormatOptions, Input, Purpose, Reading, Units};
use kinogaki_studio_api_testing::Unconverted;
use kinogaki_usd::usd::package::Bytes;
use kinogaki_usd::usd::scene::{self, Mesh, Node, NodeKind, ReadOptions, Scene};
use kinogaki_usd::usd::{Format, Result, Stage, StageOptions, layer_bytes};
use std::sync::Arc;

/// The stage rooted at the layer `path` on disk, reading the files in its
/// folder and below as Import does.
pub fn open(path: impl AsRef<std::path::Path>) -> Result<Stage> {
    open_with(path, &StageOptions::default())
}

/// The stage rooted at the layer `path` on disk with `options`, reading
/// the files in its folder and below as Import does.
pub fn open_with(path: impl AsRef<std::path::Path>, options: &StageOptions) -> Result<Stage> {
    let path = path.as_ref();
    let reading = Reading::new(Purpose::Objects, Units::default(), &Unconverted);
    let disk = Named::new(&Input::file(path), &FormatOptions::new(), &reading);
    let options = StageOptions {
        disk: Some(Arc::new(disk)),
        ..options.clone()
    };
    Stage::open(path, &options)
}

/// The stage of `bytes`, a file in memory named `name`.
pub fn stage_named(name: &str, bytes: &[u8]) -> Stage {
    Stage::open_memory(name, Bytes::from(bytes.to_vec()), &StageOptions::default()).unwrap()
}

/// The scene of a usda, usdc or usdz file in memory.
pub fn scene_of(bytes: &[u8]) -> Scene {
    scene_with(bytes, &ReadOptions::default())
}

/// The scene of a file in memory, read with `options`.
pub fn scene_with(bytes: &[u8], options: &ReadOptions) -> Scene {
    let name = match Format::detect(bytes) {
        Some(Format::Usdz) => "scene.usdz",
        Some(Format::Usdc) => "scene.usdc",
        _ => "scene.usda",
    };
    scene::read(&mut stage_named(name, bytes), options).unwrap()
}

/// The scene of the fixture `name` (tests/fixtures), read from disk.
pub fn fixture(name: &str) -> Scene {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    scene::read(&mut open(path).unwrap(), &ReadOptions::default()).unwrap()
}

/// The node at `path`.
pub fn node<'a>(scene: &'a Scene, path: &str) -> &'a Node {
    let index = (0..scene.nodes.len())
        .find(|&at| scene.path(at).text() == path)
        .unwrap_or_else(|| panic!("no node {path}"));
    &scene.nodes[index]
}

/// The mesh at `path`.
pub fn mesh<'a>(scene: &'a Scene, path: &str) -> &'a Mesh {
    match &node(scene, path).kind {
        NodeKind::Mesh(mesh) => mesh,
        other => panic!("{path} is {other:?}"),
    }
}

/// The paths of the nodes, in order.
pub fn paths(scene: &Scene) -> Vec<String> {
    (0..scene.nodes.len())
        .map(|at| scene.path(at).text())
        .collect()
}

/// `scene` written as a usda and as a usdc layer, and each read back.
pub fn round_trips(scene: &Scene) -> [Scene; 2] {
    let layer = scene::author(scene).unwrap();
    [Format::Usda, Format::Usdc].map(|format| scene_of(&layer_bytes(&layer, format).unwrap()))
}
