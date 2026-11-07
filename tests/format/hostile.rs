//! The hostile files the 2026-11-07 audit found, each read as Import reads
//! it (a 2 MiB stack, `read_hostile`): an error or a warning, within a time
//! bound and, where memory was the point, within a small budget.
use crate::crafted;
use crate::support::{Scratch, materials};
use kinogaki_studio_api::format::{ReadLimits, options};
use kinogaki_studio_api::geometry::image::{ImageFile, ImageSource};
use kinogaki_studio_api::image_files::{IMAGE_NODE, image_source};
use kinogaki_studio_api::{Error, FileFormat, Imported, Input, Value};
use kinogaki_studio_api_testing::read_hostile;
use kinogaki_usd::Usd;
use std::path::Path;
use std::time::Duration;

/// What a hostile read is given: seconds enough for a debug build doing
/// linear work, and memory far below what the probes once took.
const SECONDS: u64 = 20;

fn small() -> ReadLimits {
    ReadLimits {
        allocation: 64 << 20,
        total: 256 << 20,
        ..ReadLimits::default()
    }
}

/// `bytes`, a file named `name`, read from memory within `limits`.
fn read_bytes(name: &str, bytes: &[u8], limits: ReadLimits) -> Result<Imported, Error> {
    let (read, took) = read_hostile(&Usd, &Input::memory(name, bytes), &Usd.options(), limits);
    assert!(took < Duration::from_secs(SECONDS), "{name}: {took:?}");
    read
}

/// The file at `path` read with `options` (the format's defaults over
/// them).
fn read_path(path: &Path, chosen: &[(&str, bool)]) -> Imported {
    let mut options = Usd.options();
    options.extend(
        chosen
            .iter()
            .map(|(k, v)| ((*k).to_owned(), Value::Bool(*v))),
    );
    let (read, took) = read_hostile(&Usd, &Input::file(path), &options, small());
    assert!(
        took < Duration::from_secs(SECONDS),
        "{}: {took:?}",
        path.display()
    );
    read.unwrap()
}

/// Every file the read's Image nodes and images read.
fn sources(read: &Imported) -> Vec<ImageSource> {
    let looks = materials(read);
    let nodes = looks.materials.iter().flat_map(|m| &m.inside.nodes);
    let images = nodes.filter(|n| n.node_type == IMAGE_NODE);
    let mut found: Vec<ImageSource> = images
        .filter_map(|n| image_source(&n.parameters).ok().flatten())
        .collect();
    found.extend(looks.images.iter().map(|i| i.source.clone()));
    found
}

fn said(read: &Imported) -> String {
    let lines: Vec<&str> = read.warnings.iter().map(|w| w.message.as_str()).collect();
    lines.join("\n")
}

/// A layer whose material reads each of `files` (asset paths, three at
/// most) as a texture, and whose dome reads the first.
fn textured(files: &[&str]) -> String {
    let inputs = ["diffuseColor", "emissiveColor", "specularColor"];
    let mut text = String::from("#usda 1.0\ndef Material \"M\"\n{\n");
    text.push_str("    token outputs:surface.connect = </M/Surface.outputs:surface>\n");
    text.push_str("    def Shader \"Surface\"\n    {\n        uniform token info:id = \"UsdPreviewSurface\"\n");
    for (k, input) in inputs.iter().enumerate().take(files.len()) {
        text.push_str(&format!(
            "        color3f inputs:{input}.connect = </M/T{k}.outputs:rgb>\n"
        ));
    }
    text.push_str("        token outputs:surface\n    }\n");
    for (k, file) in files.iter().enumerate() {
        text.push_str(&format!(
            "    def Shader \"T{k}\"\n    {{\n        uniform token info:id = \"UsdUVTexture\"\n        asset inputs:file = @{file}@\n        float3 outputs:rgb\n    }}\n"
        ));
    }
    text.push_str("}\n");
    text.push_str(&format!(
        "def DomeLight \"Sky\"\n{{\n    asset inputs:texture:file = @{}@\n}}\n",
        files[0]
    ));
    text
}

#[test]
fn package_paths_open_usdz_files_alone_and_never_devices() {
    let dir = Scratch::new("hostile-packages");
    let zip = kinogaki_usd::usd::package::zip::write_stored(
        &[("word/document.xml", b"secret".as_slice())],
        64,
    )
    .unwrap();
    std::fs::write(dir.path().join("letter.docx"), zip).unwrap();
    let scene = dir.path().join("scene.usda");
    let layer = textured(&["/dev/zero[sky.exr]", "letter.docx[word/document.xml]"]);
    std::fs::write(&scene, layer).unwrap();
    for outside in [false, true] {
        // Neither is a usdz: no entry is read, whatever the option says;
        // what stays a path names no file.
        let read = read_path(&scene, &[(options::OUTSIDE, outside)]);
        for source in sources(&read) {
            match source.file() {
                ImageFile::Embedded(file) => panic!("{} read in", file.name()),
                ImageFile::Path(path) => assert!(path.ends_with(']'), "{path}"),
            }
        }
    }
}

#[test]
fn layers_and_textures_outside_the_folder_are_left_unresolved() {
    let dir = Scratch::new("hostile-reach");
    std::fs::create_dir_all(dir.path().join("scene")).unwrap();
    let secret = "#usda 1.0\ndef \"Secret\"\n{\n    def Cube \"Key\"\n    {\n    }\n}\n";
    std::fs::write(dir.path().join("secret.usda"), secret).unwrap();
    std::fs::write(dir.path().join("key.png"), b"\x89PNG\r\n\x1a\nkey").unwrap();
    let absolute = dir.path().join("secret.usda").display().to_string();
    let layer = format!(
        "#usda 1.0\n(\n    subLayers = [@../secret.usda@]\n)\ndef \"Mine\" (\n    references = @{absolute}@</Secret>\n)\n{{\n}}\n{}",
        &textured(&["../key.png", "//server/share/a.png", "/net/host/a.png"])
            ["#usda 1.0\n".len()..]
    );
    let scene = dir.path().join("scene/scene.usda");
    std::fs::write(&scene, layer).unwrap();
    let read = read_path(&scene, &[(options::EMBED, true)]);
    let names: Vec<&str> = read.items.iter().map(|i| i.name.as_str()).collect();
    assert!(
        !names.contains(&"Key") && !names.contains(&"Secret"),
        "{names:?}"
    );
    assert!(sources(&read).is_empty(), "{:?}", sources(&read));
    let warnings = said(&read);
    assert!(
        warnings.contains("are left unresolved (Read Files Outside the Folder reads them)"),
        "{warnings}"
    );
    for name in [
        "/key.png",
        "/secret.usda",
        "/server/share/a.png",
        "/net/host/a.png",
    ] {
        assert!(warnings.contains(name), "{name} in {warnings}");
    }
    // Asked, the layers compose and the picture is read in.
    let anywhere = read_path(&scene, &[(options::EMBED, true), (options::OUTSIDE, true)]);
    let names: Vec<&str> = anywhere.items.iter().map(|i| i.name.as_str()).collect();
    assert!(
        names.contains(&"Key") && names.contains(&"Secret"),
        "{names:?}"
    );
    let key = sources(&anywhere).into_iter().find_map(|s| match s.file() {
        ImageFile::Embedded(file) => Some(file.bytes().to_vec()),
        ImageFile::Path(_) => None,
    });
    assert_eq!(key.as_deref(), Some(&b"\x89PNG\r\n\x1a\nkey"[..]));
}

#[test]
fn a_usdz_names_nothing_outside_itself_on_disk() {
    let dir = Scratch::new("hostile-usdz");
    std::fs::write(dir.path().join("hosts"), b"127.0.0.1 localhost").unwrap();
    let hosts = dir.path().join("hosts").display().to_string();
    let layer = kinogaki_usd::usd::text::read_usda(
        textured(&[hosts.as_str()]).as_bytes(),
        &Default::default(),
    )
    .unwrap();
    let package = kinogaki_usd::usd::package::write_usdz(&layer, "root.usdc", &[]).unwrap();
    // Held in memory, it has no folder: nothing on disk is read.
    let read = read_bytes("scene.usdz", &package, small()).unwrap();
    assert!(sources(&read).is_empty(), "{:?}", sources(&read));
    assert!(said(&read).contains("is held in memory"), "{}", said(&read));
}

#[test]
fn a_dictionary_shared_by_its_entries_decodes_once() {
    // 30 levels: 2^30 dictionaries decoded one reference at a time.
    let dag = crafted::dictionary_dag(30);
    assert!(dag.len() < 2048, "{} bytes", dag.len());
    read_bytes("dag.usdc", &dag, small()).unwrap();
}

#[test]
fn a_path_tree_too_deep_is_refused_on_the_import_stack() {
    let deep = crafted::deep_paths(100_000);
    let error = read_bytes("deep.usdc", &deep, ReadLimits::default()).unwrap_err();
    assert!(error.message().contains("deeper than 4096"), "{error}");
}

#[test]
fn token_sections_claiming_more_than_they_hold_are_refused() {
    // Claimed: 16 GiB from 4 KiB, more than LZ4 can give.
    let claim = crafted::token_bomb(4096, Some(16 << 30));
    let error = read_bytes("claim.usdc", &claim, ReadLimits::default()).unwrap_err();
    assert!(
        error
            .message()
            .contains("more than its compressed bytes hold"),
        "{error}"
    );
    // Given: 1 MiB of LZ4 decoding to 255 MiB of empty tokens, past the budget.
    let bomb = crafted::token_bomb(1 << 20, None);
    let error = read_bytes("bomb.usdc", &bomb, small()).unwrap_err();
    assert!(
        error.message().contains("over the read's budget"),
        "{error}"
    );
}

#[test]
fn an_array_shared_by_many_fields_decodes_once() {
    // 2,000 prims sharing one 100,000-double array: 1.6 GB decoded each
    // time, 800 KB once.
    let shared = crafted::shared_array(2000, 100_000);
    read_bytes("shared.usdc", &shared, small()).unwrap();
}

#[test]
fn variables_doubling_a_string_fail_without_the_doubling() {
    let mut text = String::from("#usda 1.0\n(\n    expressionVariables = {\n");
    for level in 0..40 {
        let next = level + 1;
        text.push_str(&format!(
            "        string V{level} = \"`'${{V{next}}}${{V{next}}}'`\"\n"
        ));
    }
    text.push_str("        string V40 = \"ab\"\n    }\n    subLayers = [@`\"${V0}.usda\"`@]\n)\n");
    text.push_str("def Cube \"A\"\n{\n}\n");
    let read = read_bytes("doubling.usda", text.as_bytes(), small()).unwrap();
    assert!(read.items.iter().any(|i| i.name == "A"));
}

/// A layer of `levels` levels of reference diamonds: `A<k>` and `B<k>`
/// each reference both `A<k+1>` and `B<k+1>`.
fn diamonds(levels: usize) -> String {
    let mut text = String::from("#usda 1.0\n(\n    defaultPrim = \"A0\"\n)\n");
    for level in 0..levels {
        let next = level + 1;
        for prim in ["A", "B"] {
            text.push_str(&format!(
                "def \"{prim}{level}\" (\n    references = [</A{next}>, </B{next}>]\n)\n{{\n}}\n"
            ));
        }
    }
    text.push_str(&format!(
        "def \"A{levels}\"\n{{\n    custom int x = 1\n}}\n"
    ));
    text.push_str(&format!("def \"B{levels}\"\n{{\n}}\n"));
    text
}

#[test]
fn reference_diamonds_compose_in_time_and_fail_past_the_node_limit() {
    // 14 levels: 32,767 nodes in A0's index.
    let read = read_bytes(
        "diamonds.usda",
        diamonds(14).as_bytes(),
        ReadLimits::default(),
    );
    assert!(read.is_ok(), "{read:?}");
    // 20 levels: two million nodes, refused.
    let error = read_bytes(
        "diamonds.usda",
        diamonds(20).as_bytes(),
        ReadLimits::default(),
    );
    let error = error.unwrap_err();
    assert!(
        error
            .message()
            .contains("prim index of more than 65536 nodes"),
        "{error}"
    );
}

#[test]
fn sublayers_named_over_and_over_stack_once() {
    let dir = Scratch::new("hostile-sublayers");
    // 8 levels, each naming the next 8 times: 16 million stack entries
    // one by one, 9 layers once.
    for level in 0..8 {
        let next = format!("@./l{}.usda@", level + 1);
        let list = [next.as_str(); 8].join(", ");
        let text = format!("#usda 1.0\n(\n    subLayers = [{list}]\n)\n");
        std::fs::write(dir.path().join(format!("l{level}.usda")), text).unwrap();
    }
    std::fs::write(
        dir.path().join("l8.usda"),
        "#usda 1.0\ndef Cube \"Deep\"\n{\n}\n",
    )
    .unwrap();
    let read = read_path(&dir.path().join("l0.usda"), &[]);
    assert!(read.items.iter().any(|i| i.name == "Deep"));
}
