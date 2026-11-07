//! Packages: textures held as the package's bytes, files given in memory,
//! and scenes written as usdz with their assets, or beside a loose layer.

use crate::common::{node, scene_of};
use kinogaki_usd::usd::package::{Bytes, Package};
use kinogaki_usd::usd::scene::{self, Asset, AssetSource, NodeKind, ReadOptions};
use kinogaki_usd::usd::sdf::{Token, Value};
use kinogaki_usd::usd::{Stage, StageOptions};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really a picture";
const EXR: &[u8] = b"v/1\x01not really an image either";

const TEXTURED: &[u8] = br#"#usda 1.0
(
    defaultPrim = "Lamp"
)
def Xform "Lamp"
{
    def Material "Shade"
    {
        token outputs:surface.connect = </Lamp/Shade/Preview.outputs:surface>
        def Shader "Preview"
        {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor.connect = </Lamp/Shade/Paper.outputs:rgb>
            token outputs:surface
        }
        def Shader "Paper"
        {
            uniform token info:id = "UsdUVTexture"
            asset inputs:file = @textures/paper.png@
            float3 outputs:rgb
        }
    }
    def DomeLight "Sky"
    {
        asset inputs:texture:file = @./sky.exr@
    }
}
"#;

/// A usdz of `TEXTURED` carrying its two files.
fn package() -> Vec<u8> {
    let layer = kinogaki_usd::usd::text::read_usda(TEXTURED).unwrap();
    let files = [("textures/paper.png", PNG), ("sky.exr", EXR)];
    kinogaki_usd::usd::package::write_usdz(&layer, "lamp.usdc", &files).unwrap()
}

fn held(asset: &Asset) -> &Bytes {
    match &asset.source {
        AssetSource::Bytes(bytes) => bytes,
        other => panic!("{}: {other:?}", asset.path),
    }
}

#[test]
fn a_packages_textures_are_views_of_its_bytes() {
    let bytes = Bytes::from(package());
    let mut stage =
        Stage::open_memory("lamp.usdz", bytes.clone(), &StageOptions::default()).unwrap();
    let scene = scene::read(&mut stage, &ReadOptions::default()).unwrap();
    assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
    let paths: Vec<&str> = scene.assets.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(
        paths,
        ["/lamp.usdz[textures/paper.png]", "/lamp.usdz[sky.exr]"]
    );
    let (paper, sky) = (held(&scene.assets[0]), held(&scene.assets[1]));
    assert_eq!((&paper[..], &sky[..]), (PNG, EXR));
    // No copy: the entry is a range of the package's own buffer.
    assert!(std::sync::Arc::ptr_eq(paper.buffer().0, bytes.buffer().0));
    let NodeKind::Light(dome) = &node(&scene, "/Lamp/Sky").kind else {
        panic!("a light")
    };
    assert_eq!(dome.texture.as_deref(), Some("/lamp.usdz[sky.exr]"));
}

#[test]
fn a_file_in_memory_reads_nothing_from_disk() {
    // A sibling of the same name on disk is never read: the file has no place.
    let mut stage = crate::common::stage_named(
        "Cargo.toml.usda",
        b"#usda 1.0\ndef \"A\" (references = @./Cargo.toml@) {}\ndef Material \"M\" {\n    asset inputs:file = @README.md@\n}\n",
    );
    let scene = scene::read(&mut stage, &ReadOptions::default()).unwrap();
    assert!(
        stage.warnings().iter().any(|w| w.contains("Cargo.toml")),
        "{:?}",
        stage.warnings()
    );
    assert_eq!(scene.assets[0].source, AssetSource::Missing);
}

#[test]
fn a_scene_writes_as_a_usdz_with_its_assets_under_textures() {
    let scene = scene_of(&package());
    let directory = scratch("usdz");
    let path = directory.join("lamp copy.usdz");
    scene::write_file(&scene, &path).unwrap();
    let written = Bytes::from(std::fs::read(&path).unwrap());
    let package = Package::open(written.clone()).unwrap();
    let names: Vec<&str> = package.names().collect();
    assert_eq!(
        names,
        ["lamp copy.usdc", "textures/paper.png", "textures/sky.exr"]
    );
    assert_eq!(&package.entry("textures/paper.png").unwrap()[..], PNG);
    // Read back, the material names the packaged file.
    let again = scene_of(&written);
    let paper = again.materials[0]
        .nodes
        .iter()
        .find(|n| n.path.name() == "Paper")
        .unwrap();
    let Some(Value::Asset(file)) = &paper.input("file").unwrap().value else {
        panic!("an asset")
    };
    assert_eq!(
        again.asset(file).unwrap().authored.as_str(),
        "textures/paper.png"
    );
    assert_eq!(&held(again.asset(file).unwrap())[..], PNG);
    assert_eq!(again.nodes, scene_of(&std::fs::read(&path).unwrap()).nodes);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_loose_layer_writes_held_assets_beside_it() {
    let scene = scene_of(&package());
    let directory = scratch("loose");
    let path = directory.join("lamp.usda");
    scene::write_file(&scene, &path).unwrap();
    assert_eq!(
        std::fs::read(directory.join("lamp_textures/paper.png")).unwrap(),
        PNG
    );
    assert_eq!(
        std::fs::read(directory.join("lamp_textures/sky.exr")).unwrap(),
        EXR
    );
    let text = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
    assert!(text.contains("@lamp_textures/paper.png@"), "{text}");
    // Read from disk, the files are found beside it.
    let again = scene::read(&mut Stage::open(&path).unwrap(), &ReadOptions::default()).unwrap();
    assert!(
        again
            .assets
            .iter()
            .all(|a| matches!(a.source, AssetSource::File(_))),
        "{:?}",
        again.assets
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_package_inside_a_package_is_read_through() {
    let inner = package();
    let outer = kinogaki_usd::usd::package::write_usdz(
        &kinogaki_usd::usd::text::read_usda(
            b"#usda 1.0\ndef \"Room\" (references = @lamp.usdz@) {}\n",
        )
        .unwrap(),
        "room.usda",
        &[("lamp.usdz", &inner)],
    )
    .unwrap();
    let scene = scene_of(&outer);
    assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
    let paper = scene
        .asset("/scene.usdz[lamp.usdz[textures/paper.png]]")
        .unwrap();
    assert_eq!(&held(paper)[..], PNG);
    assert_eq!(scene.nodes[0].name, Token::new("Room"));
}

/// A new empty directory for one test's files.
fn scratch(name: &str) -> std::path::PathBuf {
    let directory =
        std::env::temp_dir().join(format!("kinogaki-usd-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}
