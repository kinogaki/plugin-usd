//! Materials written and read back through the format: an engine's material
//! comes back as itself, with its standard form; a MaterialX material and a
//! USD Preview Surface come back as they were; every network a material
//! has is in the file (a usdz too: the owner's ruling), with a USD Preview
//! Surface made for other applications; a usdz's textures read back
//! embedded, as ranges of the package. Apple's `usdchecker --arkit` (a
//! local oracle) passes a usdz of MaterialX and USD Preview Surface
//! materials.
use crate::support::{Scratch, materials, metres, read_file, shape, square, write_file};
use kinogaki_studio_api::geometry::image::{EmbeddedFile, ImageColor, ImageFile, ImageSource};
use kinogaki_studio_api::image_files::{IMAGE_NODE, image_node, image_source};
use kinogaki_studio_api::network::{NetworkSeed, NewNode};
use kinogaki_studio_api::standard::{self, ImportedMaterial, Materials};
use kinogaki_studio_api::{FileFormat, FileItem, Input, NodeTypeId, Purpose, Reading, Value};
use kinogaki_studio_api_testing::Unconverted;
use kinogaki_usd::Usd;
use kinogaki_usd::usd::scene::{self, ReadOptions};
use kinogaki_usd::usd::{Stage, StageOptions};
use std::sync::Arc;

/// An engine's material: its own surface (an Anodized Aluminum's values),
/// a node its surface doesn't read, and its OpenPBR form.
fn anodized() -> ImportedMaterial {
    let surface = NodeTypeId::parse("kinogaki.spectre.anodized_aluminum_surface").unwrap();
    let mut inside = NetworkSeed::default();
    inside.node(
        NewNode::new(surface)
            .named("Surface")
            .with("finish", Value::Choice(Arc::from("polished")))
            .with("color", Value::Color([0.62, 0.04, 0.03]))
            .with("gloss", 0.85)
            .with("thickness", 15.0)
            .with("layers", Value::Integer(3))
            .with("note", Value::Text("hand polished".into()))
            .displayed(),
    );
    let ramp = NodeTypeId::parse("kinogaki.spectre.color_ramp").unwrap();
    inside.node(
        NewNode::new(ramp)
            .named("Unused")
            .with("keys", Value::Numbers(vec![0.0, 0.5, 1.0])),
    );
    let mut standard = NetworkSeed::default();
    standard.node(
        NewNode::materialx("ND_open_pbr_surface_surfaceshader")
            .unwrap()
            .named("OpenPBR")
            .with("base_metalness", 1.0)
            .with("coat_weight", 1.0)
            .with("coat_color", Value::Color([0.625, 0.0625, 0.03125]))
            .displayed(),
    );
    ImportedMaterial {
        key: "/Looks/Red_Anodized".into(),
        name: "Red Anodized, Polished".into(),
        inside,
        standard: Some(standard),
        double_sided: true,
    }
}

/// A MaterialX material: an OpenPBR Surface whose base color is a texture
/// held in the project, and a Standard Surface it doesn't read.
fn wood(file: &EmbeddedFile) -> ImportedMaterial {
    let mut inside = NetworkSeed::default();
    let surface = inside.node(
        NewNode::materialx("ND_open_pbr_surface_surfaceshader")
            .unwrap()
            .named("Surface")
            .with("specular_roughness", 0.25)
            .displayed(),
    );
    let base = inside.node(
        NewNode::materialx("ND_image_color3")
            .unwrap()
            .named("Base")
            .with("uaddressmode", Value::Choice(Arc::from("mirror"))),
    );
    let source = ImageSource::new(ImageFile::Embedded(file.clone()), ImageColor::Auto);
    let image = inside.node(image_node::reading(&source).named("wood"));
    inside.node(
        NewNode::materialx("ND_standard_surface_surfaceshader")
            .unwrap()
            .named("Classic")
            .with("base_color", Value::Color([0.5, 0.25, 0.125])),
    );
    inside.wire(image, image_node::IMAGE, base, "file");
    inside.wire(base, "out", surface, "base_color");
    ImportedMaterial {
        key: "/Looks/Wood".into(),
        name: "Wood".into(),
        inside,
        standard: None,
        double_sided: false,
    }
}

/// A USD Preview Surface material: a texture on disk through texture
/// coordinates, a cutout, the specular workflow.
fn preview() -> ImportedMaterial {
    let mut inside = NetworkSeed::default();
    let surface = inside.node(
        NewNode::materialx("ND_UsdPreviewSurface_surfaceshader")
            .unwrap()
            .named("Preview")
            .with("roughness", 0.25)
            .with("useSpecularWorkflow", Value::Integer(1))
            .with("specularColor", Value::Color([0.5, 0.5, 0.25]))
            .with("opacityMode", Value::Integer(1))
            .with("opacityThreshold", 0.5)
            .displayed(),
    );
    let texture = inside.node(
        NewNode::materialx("ND_UsdUVTexture_23")
            .unwrap()
            .named("Texture")
            .with("wrapS", Value::Choice(Arc::from("periodic")))
            .with("wrapT", Value::Choice(Arc::from("mirror")))
            .with("scale", Value::Numbers(vec![0.5, 0.5, 0.5, 1.0])),
    );
    let reader = inside.node(
        NewNode::materialx("ND_UsdPrimvarReader_vector2")
            .unwrap()
            .named("Reader")
            .with("varname", Value::Text("uv".into())),
    );
    let srgb = ImageColor::Space(Arc::from("srgb_texture"));
    let source = ImageSource::new(ImageFile::Path(Arc::from("/maps/leaf.png")), srgb);
    let image = inside.node(image_node::reading(&source).named("leaf"));
    inside.wire(image, image_node::IMAGE, texture, "file");
    inside.wire(reader, "out", texture, "st");
    inside.wire(texture, "rgb", surface, "diffuseColor");
    inside.wire(texture, "a", surface, "opacity");
    ImportedMaterial {
        key: "/Looks/Leaf".into(),
        name: "Leaf".into(),
        inside,
        standard: None,
        double_sided: true,
    }
}

/// `materials` and a square showing each, as a file's items.
fn items(materials: Vec<ImportedMaterial>) -> Vec<FileItem> {
    let squares: Vec<FileItem> = (materials.iter())
        .map(|m| FileItem::top(m.name.clone(), standard::geometry().wrap(square(&m.key))))
        .collect();
    let looks = Materials {
        materials,
        images: Vec::new(),
    };
    let mut items = vec![FileItem::top("Looks", standard::materials().wrap(looks))];
    items.extend(squares);
    items
}

/// The render contexts each material of the file at `path` has terminals
/// in.
fn contexts(path: &std::path::Path) -> Vec<Vec<String>> {
    let mut stage = Stage::open_with(path, &StageOptions::default()).unwrap();
    let scene = scene::read(&mut stage, &ReadOptions::default()).unwrap();
    (scene.materials.iter())
        .map(|m| m.outputs.iter().map(|o| o.name.to_string()).collect())
        .collect()
}

#[test]
fn an_engines_material_comes_back_as_itself_from_a_usdz() {
    let dir = Scratch::new("engine");
    let path = dir.path().join("panel.usdz");
    let written = anodized();
    write_file(&path, &items(vec![written.clone()]), metres());
    // Every network it has: the engine's, its standard form, and a USD
    // Preview Surface for other applications.
    assert_eq!(
        contexts(&path),
        [["kinogaki:surface", "mtlx:surface", "surface"]]
    );
    let read = read_file(&path, metres(), false);
    let back = &materials(&read).materials[0];
    assert_eq!(
        (back.name.as_str(), back.double_sided),
        ("Red Anodized, Polished", true)
    );
    assert_eq!(shape(&back.inside), shape(&written.inside));
    assert_eq!(
        shape(back.standard.as_ref().unwrap()),
        shape(written.standard.as_ref().unwrap())
    );
    // The square names it by its key in the file.
    let (_, square) = crate::support::item(&read, "Red_Anodized__Polished");
    let polygons = standard::geometry().get(square.unwrap()).unwrap();
    let bound = polygons
        .attributes()
        .text(kinogaki_studio_api::geometry::Domain::Primitive, "material");
    assert_eq!(bound.unwrap().get(0), Some(back.key.as_str()));
}

#[test]
fn a_materialx_material_and_its_packaged_texture_come_back_as_they_were() {
    let dir = Scratch::new("mtlx");
    let path = dir.path().join("table.usdz");
    let bytes: Vec<u8> = (0..4096u32).map(|i| (i * 7 % 251) as u8).collect();
    let file = EmbeddedFile::new("wood.png", bytes);
    let written = wood(&file);
    write_file(&path, &items(vec![written.clone()]), metres());
    assert_eq!(contexts(&path), [["mtlx:surface", "surface"]]);
    let read = read_file(&path, metres(), false);
    let back = &materials(&read).materials[0];
    assert_eq!(shape(&back.inside), shape(&written.inside));
    assert!(back.standard.is_none());
    // Single-sided: the meshes showing it say so.
    assert!(!back.double_sided);
}

#[test]
fn a_usd_preview_surface_comes_back_as_it_was_and_alone() {
    let dir = Scratch::new("preview");
    let path = dir.path().join("leaf.usda");
    let written = preview();
    write_file(&path, &items(vec![written.clone()]), metres());
    assert_eq!(contexts(&path), [["surface"]]);
    let text = std::fs::read_to_string(&path).unwrap();
    for spelled in [
        r#"uniform token info:id = "UsdPreviewSurface""#,
        r#"uniform token info:id = "UsdPrimvarReader_float2""#,
        r#"token inputs:wrapS = "repeat""#,
        r#"token inputs:opacityMode = "presence""#,
        r#"uniform token inputs:sourceColorSpace = "sRGB""#,
        r#"string inputs:varname = "st""#,
        "texCoord2f[] primvars:st",
    ] {
        assert!(text.contains(spelled), "{spelled} in\n{text}");
    }
    let read = read_file(&path, metres(), false);
    let back = &materials(&read).materials[0];
    assert_eq!(shape(&back.inside), shape(&written.inside));
}

#[test]
fn other_applications_see_the_textured_base_color_in_the_preview() {
    let dir = Scratch::new("other");
    let path = dir.path().join("table.usdz");
    let file = EmbeddedFile::new("wood.png", vec![1, 2, 3, 4]);
    write_file(&path, &items(vec![wood(&file)]), metres());
    let mut stage = Stage::open(&path).unwrap();
    let scene = scene::read(&mut stage, &ReadOptions::default()).unwrap();
    let material = &scene.materials[0];
    let preview = material.surface("").unwrap();
    assert_eq!(preview.id().unwrap().as_str(), "UsdPreviewSurface");
    let diffuse = preview.input("diffuseColor").unwrap();
    let texture = &material.nodes[diffuse.sources[0].node.unwrap()];
    assert_eq!(texture.id().unwrap().as_str(), "UsdUVTexture");
    // The same packaged file as the MaterialX network's texture.
    let mtlx = material.surface("mtlx").unwrap();
    let base = &material.nodes[mtlx.input("base_color").unwrap().sources[0].node.unwrap()];
    assert_eq!(
        texture.input("file").unwrap().value,
        base.input("file").unwrap().value
    );
    assert_eq!(
        texture
            .input("wrapT")
            .unwrap()
            .value
            .as_ref()
            .and_then(|v| v.as_token())
            .unwrap()
            .as_str(),
        "repeat"
    );
}

#[test]
fn a_usdz_texture_reads_back_embedded_as_a_range_of_the_package() {
    let dir = Scratch::new("packaged");
    let path = dir.path().join("table.usdz");
    let bytes: Vec<u8> = (0..100_000u32).map(|i| (i % 253) as u8).collect();
    let file = EmbeddedFile::new("wood.png", bytes.clone());
    write_file(&path, &items(vec![wood(&file)]), metres());
    let package = EmbeddedFile::new("table.usdz", std::fs::read(&path).unwrap());
    let reading = Reading::new(Purpose::Objects, metres(), &Unconverted);
    let read = Usd
        .read(&Input::embedded(&package), &Usd.options(), &reading)
        .unwrap();
    let back = &materials(&read).materials[0];
    let image = back
        .inside
        .nodes
        .iter()
        .find(|n| n.node_type == IMAGE_NODE)
        .unwrap();
    let source = image_source(&image.parameters).unwrap().unwrap();
    let ImageFile::Embedded(read_back) = source.file() else {
        panic!("a package's texture is embedded");
    };
    assert_eq!(
        (read_back.name(), read_back.bytes()),
        ("wood.png", &bytes[..])
    );
    let held = package.bytes().as_ptr_range();
    assert!(
        held.contains(&read_back.bytes().as_ptr()),
        "a range of the package, not a copy"
    );
}

#[test]
#[ignore = "hand: Apple's usdchecker, a local oracle"]
fn hand_run_usdchecker_arkit_passes_materialx_and_preview_surface_usdz() {
    let dir = Scratch::new("arkit");
    let path = dir.path().join("table.usdz");
    // A PNG of one pixel: what usdchecker's texture check reads.
    let png = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let wood = wood(&EmbeddedFile::new("wood.png", png.to_vec()));
    let mut leaf = preview();
    for node in &mut leaf.inside.nodes {
        if node.node_type == IMAGE_NODE {
            let packed = ImageSource::new(
                ImageFile::Embedded(EmbeddedFile::new("leaf.png", png.to_vec())),
                ImageColor::Auto,
            );
            *node = image_node::reading(&packed).named("leaf");
        }
    }
    write_file(&path, &items(vec![wood, leaf]), metres());
    let checked = std::process::Command::new("usdchecker")
        .arg("--arkit")
        .arg(&path)
        .output()
        .expect("usdchecker runs");
    let said = String::from_utf8_lossy(&checked.stdout).into_owned()
        + &String::from_utf8_lossy(&checked.stderr);
    assert!(checked.status.success(), "{said}");
}
