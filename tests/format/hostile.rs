//! The hostile files the 2026-11-07 audit found, each read as Import reads
//! it (a 2 MiB stack, `read_hostile`): an error or a warning, within a time
//! bound and, where memory was the point, within a small budget.
use crate::crafted;
use crate::support::{Scratch, metres, write_file};
use kinogaki_render_api::standard::LightShape;
use kinogaki_studio_api::format::{ReadLimits, options};
use kinogaki_studio_api::geometry::image::{ImageFile, ImageSource};
use kinogaki_studio_api::image_files::{IMAGE_NODE, image_source};
use kinogaki_studio_api::standard;
use kinogaki_studio_api::value::ImageRef;
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

/// Every file the read's Image nodes and domes read.
fn sources(read: &Imported) -> Vec<ImageSource> {
    let looks = (read.items.first()).and_then(|i| standard::materials().get(i.data.as_ref()?));
    let materials = looks.into_iter().flat_map(|l| &l.materials);
    let nodes = materials.flat_map(|m| &m.inside.nodes);
    let images = nodes.filter(|n| n.node_type == IMAGE_NODE);
    let mut found: Vec<ImageSource> = images
        .filter_map(|n| image_source(&n.parameters).ok().flatten())
        .collect();
    let lights = read
        .items
        .iter()
        .filter_map(|i| standard::light().get(i.data.as_ref()?));
    found.extend(lights.filter_map(|l| match &l.shape {
        LightShape::Dome {
            image: Some(ImageRef::File(source)),
            ..
        } => Some(source.clone()),
        _ => None,
    }));
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
    std::fs::write(dir.join("letter.docx"), zip).unwrap();
    let scene = dir.join("scene.usda");
    let layer = textured(&["/dev/zero[sky.exr]", "letter.docx[word/document.xml]"]);
    std::fs::write(&scene, layer).unwrap();
    // Neither is a usdz: no entry is read; what stays a path names no file.
    let read = read_path(&scene, &[]);
    for source in sources(&read) {
        match source.file() {
            ImageFile::Embedded(file) => panic!("{} read in", file.name()),
            ImageFile::Path(path) => assert!(path.ends_with(']'), "{path}"),
        }
    }
}

#[test]
fn layers_and_textures_outside_the_folder_are_read() {
    let dir = Scratch::new("hostile-reach");
    std::fs::create_dir_all(dir.join("scene")).unwrap();
    let secret = "#usda 1.0\ndef \"Secret\"\n{\n    def Cube \"Key\"\n    {\n    }\n}\n";
    std::fs::write(dir.join("secret.usda"), secret).unwrap();
    std::fs::write(dir.join("key.png"), b"\x89PNG\r\n\x1a\nkey").unwrap();
    let absolute = dir.join("secret.usda").display().to_string();
    let layer = format!(
        "#usda 1.0\n(\n    subLayers = [@../secret.usda@]\n)\ndef \"Mine\" (\n    references = @{absolute}@</Secret>\n)\n{{\n}}\n{}",
        &textured(&["../key.png", "//server/share/a.png", "/dev/zero"])["#usda 1.0\n".len()..]
    );
    let scene = dir.join("scene/scene.usda");
    std::fs::write(&scene, layer).unwrap();
    let read = read_path(&scene, &[(options::EMBED, true)]);
    // The layers compose and the picture is read in, wherever they are.
    let names: Vec<&str> = read.items.iter().map(|i| i.name.as_str()).collect();
    assert!(
        names.contains(&"Key") && names.contains(&"Secret"),
        "{names:?}"
    );
    let key = sources(&read).into_iter().find_map(|s| match s.file() {
        ImageFile::Embedded(file) => Some(file.bytes().to_vec()),
        ImageFile::Path(_) => None,
    });
    assert_eq!(key.as_deref(), Some(&b"\x89PNG\r\n\x1a\nkey"[..]));
    // A device never, and what isn't there is said.
    let warnings = said(&read);
    assert!(warnings.contains("never read: /dev/zero"), "{warnings}");
    assert!(warnings.contains("/server/share/a.png"), "{warnings}");
}

#[test]
fn a_usdz_held_in_memory_reads_nothing_on_disk() {
    let dir = Scratch::new("hostile-usdz");
    std::fs::write(dir.join("hosts"), b"127.0.0.1 localhost").unwrap();
    let hosts = dir.join("hosts").display().to_string();
    let layer = kinogaki_usd::usd::text::read_usda(
        textured(&[hosts.as_str()]).as_bytes(),
        &Default::default(),
    )
    .unwrap();
    let package = kinogaki_usd::usd::package::write_usdz(&layer, "root.usdc", &[]).unwrap();
    // Held in memory, it has no folder: nothing on disk is read, and its
    // textures keep the names it gives, as paths.
    let read = read_bytes("scene.usdz", &package, small()).unwrap();
    let sources = sources(&read);
    assert!(!sources.is_empty());
    for source in sources {
        assert_eq!(*source.file(), ImageFile::Path(hosts.as_str().into()));
    }
}

#[test]
fn a_dictionary_shared_by_its_entries_decodes_once() {
    // 30 levels: 2^30 dictionaries decoded one reference at a time.
    let dag = crafted::dictionary_dag(30);
    assert!(dag.len() < 2048, "{} bytes", dag.len());
    let read = read_bytes("dag.usdc", &dag, small()).unwrap();
    // As text, the layer's metadata, it would hold all of them: left out.
    let said = "customLayerData: more than the read's budget holds; left out";
    assert!(
        read.warnings.iter().any(|w| w.message == said),
        "{:?}",
        read.warnings
    );
    assert!(read.metadata.is_empty());
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
        std::fs::write(dir.join(format!("l{level}.usda")), text).unwrap();
    }
    std::fs::write(dir.join("l8.usda"), "#usda 1.0\ndef Cube \"Deep\"\n{\n}\n").unwrap();
    let read = read_path(&dir.join("l0.usda"), &[]);
    assert!(read.items.iter().any(|i| i.name == "Deep"));
}

/// A layer of a cube `Many` whose `field` list op prepends `item(k)` for
/// each `k` of `count`, and `targets` prims `P<k>`.
fn listing(field: &str, count: usize, item: impl Fn(usize) -> String, targets: usize) -> String {
    let items: Vec<String> = (0..count).map(item).collect();
    let mut text = format!(
        "#usda 1.0\ndef Cube \"Many\" (\n    prepend {field} = [{}]\n)\n{{\n}}\n",
        items.join(", ")
    );
    for k in 0..targets {
        text.push_str(&format!("def Xform \"P{k}\"\n{{\n}}\n"));
    }
    text
}

#[test]
fn list_ops_of_many_items_apply_in_linear_time() {
    // 300,000 API schemas on a light (which looks for its shaping): each
    // prepended item once looked through all.
    let schemas = listing("apiSchemas", 300_000, |k| format!("\"S{k}\""), 0);
    let schemas = schemas.replacen("def Cube", "def SphereLight", 1);
    let read = read_bytes("schemas.usda", schemas.as_bytes(), small()).unwrap();
    assert!(read.items.iter().any(|i| i.name == "Many"));
    // 150,000 references, each with custom data (a dictionary of floats,
    // which can't be hashed) and an offset, into a layer that isn't there.
    let references = listing(
        "references",
        150_000,
        |k| format!("@./gone.usda@</P{k}> (offset = {k}; customData = {{ double weight = 0.5 }})"),
        0,
    );
    let read = read_bytes("references.usda", references.as_bytes(), small()).unwrap();
    assert!(read.items.iter().any(|i| i.name == "Many"));
}

#[test]
fn many_relocates_are_checked_in_linear_time() {
    // 100,000 relocates, each checked against the others: the ones that
    // conflict (a target that is another's source, two sources with one
    // target, a source under another's) go, the rest stay.
    let count = 100_000;
    let mut pairs: Vec<String> = (0..count)
        .map(|k| format!("</Root/A{k}>: </Root/B{k}>"))
        .collect();
    pairs.push("</Root/B0>: </Root/C0>".into());
    pairs.push("</Root/D0>: </Root/B1>".into());
    pairs.push("</Root/A2/E>: </Root/F>".into());
    let text = format!(
        "#usda 1.0\n(\n    relocates = {{\n        {}\n    }}\n)\ndef Cube \"Root\"\n{{\n}}\n",
        pairs.join(",\n        ")
    );
    let read = read_bytes("relocates.usda", text.as_bytes(), small()).unwrap();
    assert!(read.items.iter().any(|i| i.name == "Root"));
    // A0 → B0 and B0 → C0, A1 → B1 and D0 → B1, and A2/E under A2 go.
    let said = said(&read);
    assert!(said.contains("Conflicting relocation (5 times)"), "{said}");
}

/// A layer of `count` children a reference brings under `Root`, each
/// relocated to a name of its own.
fn relocated_children(count: usize) -> String {
    let pairs: Vec<String> = (0..count)
        .map(|k| format!("</Root/A{k}>: </Root/B{k}>"))
        .collect();
    let mut text = format!(
        "#usda 1.0\n(\n    relocates = {{\n        {}\n    }}\n)\n",
        pairs.join(",\n        ")
    );
    text.push_str("def Xform \"Root\" (\n    references = </Model>\n)\n{\n}\n");
    text.push_str("over \"Model\"\n{\n");
    for k in 0..count {
        text.push_str(&format!("    def Cube \"A{k}\"\n    {{\n    }}\n"));
    }
    text.push_str("}\n");
    text
}

#[test]
fn many_relocated_children_compose_in_linear_time_and_memory() {
    // Each child's index maps through the relocations under Root: one set
    // of 40,000 pairs, shared, not a copy each (64 GB).
    let count = 40_000;
    let read = read_bytes(
        "relocated.usda",
        relocated_children(count).as_bytes(),
        small(),
    );
    let read = read.unwrap();
    let moved = (read.items.iter()).filter(|i| i.name.starts_with('B'));
    assert_eq!(moved.count(), count);
}

#[test]
fn a_prim_order_of_many_names_applies_in_linear_time() {
    // 100,000 children a reference brings, ordered last to first.
    let count = 100_000;
    let order: Vec<String> = (0..count).rev().map(|k| format!("\"C{k}\"")).collect();
    let mut text = format!(
        "#usda 1.0\ndef Cube \"Root\" (\n    references = </Model>\n)\n{{\n    reorder nameChildren = [{}]\n}}\ndef \"Model\"\n{{\n",
        order.join(", ")
    );
    for k in 0..count {
        text.push_str(&format!("    def \"C{k}\"\n    {{\n    }}\n"));
    }
    text.push_str("}\n");
    let read = read_bytes("ordered.usda", text.as_bytes(), small()).unwrap();
    assert!(read.items.iter().any(|i| i.name == "Root"));
}

#[test]
fn a_layer_named_many_ways_is_read_once() {
    let dir = Scratch::new("hostile-spellings");
    // A layer of 64 KiB, and 64 links naming it.
    let mut layer = String::from("#usda 1.0\ndef Cube \"Big\"\n{\n");
    while layer.len() < 64 << 10 {
        layer.push_str("    custom double padding = 0.000000000000000000000000000001\n");
    }
    layer.push_str("}\n");
    std::fs::write(dir.join("big.usda"), &layer).unwrap();
    let links: Vec<String> = (0..64).map(|k| format!("@./link{k}.usda@")).collect();
    for k in 0..64 {
        std::os::unix::fs::symlink("big.usda", dir.join(format!("link{k}.usda"))).unwrap();
    }
    let root = format!("#usda 1.0\n(\n    subLayers = [{}]\n)\n", links.join(", "));
    std::fs::write(dir.join("root.usda"), root).unwrap();
    // The files read may take the layer twice over, not 64 times.
    let limits = ReadLimits {
        bytes: 2 * layer.len() as u64,
        ..small()
    };
    let (read, _) = read_hostile(
        &Usd,
        &Input::file(&dir.join("root.usda")),
        &Usd.options(),
        limits,
    );
    let read = read.unwrap();
    assert!(read.items.iter().any(|i| i.name == "Big"));
    assert!(read.warnings.is_empty(), "{}", said(&read));
}

#[test]
fn many_alike_names_export_in_linear_time() {
    use kinogaki_studio_api::{FileItem, Xform};
    let dir = Scratch::new("hostile-names");
    let group = FileItem {
        name: "Instance".into(),
        parent: None,
        placement: Xform::IDENTITY,
        visible: true,
        data: None,
    };
    let groups = vec![group; 20_000];
    let start = std::time::Instant::now();
    write_file(&dir.join("many.usda"), &groups, metres());
    assert!(
        start.elapsed() < Duration::from_secs(SECONDS),
        "{:?}",
        start.elapsed()
    );
    let text = std::fs::read_to_string(dir.join("many.usda")).unwrap();
    assert!(text.contains("\"Instance_19999\""));
}

#[test]
fn a_usdz_packs_pictures_and_nothing_else() {
    use kinogaki_studio_api::image_files::image_node;
    use kinogaki_studio_api::network::{NetworkSeed, NewNode};
    use kinogaki_studio_api::standard::{self, ImportedMaterial, Materials};
    let dir = Scratch::new("hostile-pack");
    let png = dir.join("wood.png");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\nwood").unwrap();
    let key = dir.join("id_rsa.png");
    std::fs::write(&key, b"-----BEGIN OPENSSH PRIVATE KEY-----").unwrap();
    let mut inside = NetworkSeed::default();
    let surface = NewNode::materialx("ND_UsdPreviewSurface_surfaceshader").unwrap();
    let surface = inside.node(surface.displayed());
    for (k, file) in [&png, &key, Path::new("/etc/hosts")]
        .into_iter()
        .enumerate()
    {
        let texture = inside.node(NewNode::materialx("ND_UsdUVTexture_23").unwrap());
        let source = ImageSource::path(&file.display().to_string());
        let image = inside.node(image_node::reading(&source).named(format!("I{k}")));
        inside.wire(image, image_node::IMAGE, texture, "file");
        if k == 0 {
            inside.wire(texture, "rgb", surface, "diffuseColor");
        }
    }
    let material = ImportedMaterial {
        key: "/Looks/M".into(),
        name: "M".into(),
        inside,
        standard: None,
        double_sided: true,
    };
    let looks = Materials {
        materials: vec![material],
    };
    let square = crate::support::square("/Looks/M");
    let items = vec![
        kinogaki_studio_api::FileItem::top("Looks", standard::materials().wrap(looks)),
        kinogaki_studio_api::FileItem::top("Square", standard::geometry().wrap(square)),
    ];
    let path = dir.join("packed.usdz");
    let said = write_file(&path, &items, metres());
    let bytes =
        kinogaki_usd::usd::package::Package::open(std::fs::read(&path).unwrap().into()).unwrap();
    let names: Vec<&str> = bytes.names().collect();
    assert_eq!(names[1..], ["textures/wood.png"], "{names:?}");
    let left = said.join("\n");
    assert!(
        left.contains("id_rsa.png: no picture a usdz holds"),
        "{left}"
    );
    assert!(
        left.contains("/etc/hosts: no picture a usdz holds"),
        "{left}"
    );
}
