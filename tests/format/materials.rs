//! Materials written and read back through the format: an engine's material
//! comes back as itself, with its standard form; a MaterialX material and a
//! USD Preview Surface come back as they were; every network a material
//! has is in the file (a usdz too: the owner's ruling), with a USD Preview
//! Surface made for other applications; a usdz's textures read back
//! embedded, as ranges of the package. Kinogaki's definitions (a glTF
//! texture with its whole sampler) travel as the MaterialX ones they
//! inherit and come back as themselves; a material keeps its sides
//! whatever shows it, and its lengths move into the project's units. An
//! inline MaterialX image of a float or vector reads its file as data,
//! whatever it says, as MaterialX converts colors alone.
//! Apple's `usdchecker --arkit` (a local oracle) passes a usdz of MaterialX,
//! glTF and USD Preview Surface materials.
use crate::support::{
    Doubling, Scratch, materials, metres, read_file, read_in, read_with, shape, square, stage,
    write_file, write_in,
};
use kinogaki_studio_api::color::SRGB_TEXTURE;
use kinogaki_studio_api::geometry::EmbeddedFile;
use kinogaki_studio_api::geometry::image::{ImageColor, ImageFile, ImageSource};
use kinogaki_studio_api::geometry::{
    Attribute, Attributes, Domain, Geometry, Primitives, Text, names,
};
use kinogaki_studio_api::image_files::{IMAGE_NODE, image_node, image_source};
use kinogaki_studio_api::network::{NetworkSeed, NewNode};
use kinogaki_studio_api::standard::{self, ImportedMaterial, Materials};
use kinogaki_studio_api::{
    FileFormat, FileItem, FormatOptions, Input, NodeTypeId, Purpose, Reading, Value,
};
use kinogaki_studio_api_testing::Unconverted;
use kinogaki_usd::Usd;
use kinogaki_usd::usd::scene::{self, ReadOptions};
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
    let srgb = ImageColor::space(SRGB_TEXTURE).unwrap();
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
    let looks = Materials { materials };
    let mut items = vec![FileItem::top("Looks", standard::materials().wrap(looks))];
    items.extend(squares);
    items
}

/// The render contexts each material of the file at `path` has terminals
/// in.
fn contexts(path: &std::path::Path) -> Vec<Vec<String>> {
    let mut stage = stage(path);
    let scene = scene::read(&mut stage, &ReadOptions::default()).unwrap();
    (scene.materials.iter())
        .map(|m| m.outputs.iter().map(|o| o.name.to_string()).collect())
        .collect()
}

#[test]
fn an_engines_material_comes_back_as_itself_from_a_usdz() {
    let dir = Scratch::new("engine");
    let path = dir.join("panel.usdz");
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
    let path = dir.join("table.usdz");
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
    let path = dir.join("leaf.usda");
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
    // The texture is elsewhere on disk: read when files outside are.
    let anywhere = FormatOptions::from([("outside".to_owned(), Value::Bool(true))]);
    let read = read_with(&path, metres(), &anywhere);
    let back = &materials(&read).materials[0];
    assert_eq!(shape(&back.inside), shape(&written.inside));
}

#[test]
fn other_applications_see_the_textured_base_color_in_the_preview() {
    let dir = Scratch::new("other");
    let path = dir.join("table.usdz");
    let file = EmbeddedFile::new("wood.png", vec![1, 2, 3, 4]);
    write_file(&path, &items(vec![wood(&file)]), metres());
    let mut stage = stage(&path);
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
fn an_inline_materialx_image_of_no_color_reads_its_file_as_data() {
    let dir = Scratch::new("data-images");
    let path = dir.join("look.usda");
    for file in ["rough.jpg", "normal.jpg", "color.jpg"] {
        std::fs::write(dir.join(file), [1, 2, 3]).unwrap();
    }
    let text = r#"#usda 1.0
def Material "Look"
{
    token outputs:mtlx:surface.connect = </Look/Surface.outputs:out>
    def Shader "Surface"
    {
        uniform token info:id = "ND_standard_surface_surfaceshader"
        color3f inputs:base_color.connect = </Look/Color.outputs:out>
        float inputs:specular_roughness.connect = </Look/Rough.outputs:out>
        float3 inputs:coat_normal.connect = </Look/Normal.outputs:out>
        token outputs:out
    }
    def Shader "Rough"
    {
        uniform token info:id = "ND_image_float"
        asset inputs:file = @rough.jpg@
        float outputs:out
    }
    def Shader "Normal"
    {
        uniform token info:id = "ND_image_vector3"
        asset inputs:file = @normal.jpg@ (colorSpace = "srgb_texture")
        float3 outputs:out
    }
    def Shader "Color"
    {
        uniform token info:id = "ND_image_color3"
        asset inputs:file = @color.jpg@
        color3f outputs:out
    }
}
"#;
    std::fs::write(&path, text).unwrap();
    let read = read_file(&path, metres(), false);
    let inside = &materials(&read).materials[0].inside;
    let color_of = |file: &str| {
        let sources = (inside.nodes.iter())
            .filter(|n| n.node_type == IMAGE_NODE)
            .filter_map(|n| image_source(&n.parameters).unwrap());
        let source = sources.into_iter().find(|s| s.name().ends_with(file));
        source.map(|s| s.color().clone())
    };
    assert_eq!(color_of("rough.jpg"), Some(ImageColor::Data));
    assert_eq!(color_of("normal.jpg"), Some(ImageColor::Data));
    assert_eq!(color_of("color.jpg"), Some(ImageColor::Auto));
}

#[test]
fn a_usdz_texture_reads_back_embedded_as_a_range_of_the_package() {
    let dir = Scratch::new("packaged");
    let path = dir.join("table.usdz");
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

/// A glTF material as glTF's reader makes one: glTF PBR whose base color is
/// a texture of Kinogaki's (glTF's whole sampler: a minification filter, an
/// undefined magnification one), and an occlusion texture holding no
/// filter of its own, the one file read as color and as data; a volume's
/// lengths.
fn gltf(file: &EmbeddedFile) -> ImportedMaterial {
    let mut inside = NetworkSeed::default();
    let surface = inside.node(
        NewNode::materialx("ND_gltf_pbr_surfaceshader")
            .unwrap()
            .named("Surface")
            .with("thickness", 2.0)
            .with("attenuation_distance", 0.5)
            .displayed(),
    );
    let base = inside.node(
        NewNode::materialx("ND_kinogaki_gltf_colorimage")
            .unwrap()
            .named("Base")
            .with("filtertype", Value::Choice(Arc::from("")))
            .with(
                "minfilter",
                Value::Choice(Arc::from("nearest_mipmap_linear")),
            ),
    );
    let occlusion = inside.node(
        NewNode::materialx("ND_kinogaki_gltf_image_float")
            .unwrap()
            .named("Occlusion"),
    );
    // Kinogaki's color4 image places in glTF's order, MaterialX's in the
    // other by default.
    inside.node(
        NewNode::materialx("ND_kinogaki_gltf_image_color4")
            .unwrap()
            .named("Spare"),
    );
    for (color, texture) in [(ImageColor::Auto, base), (ImageColor::Data, occlusion)] {
        let source = ImageSource::new(ImageFile::Embedded(file.clone()), color);
        let image = inside.node(image_node::reading(&source).named("box"));
        inside.wire(image, image_node::IMAGE, texture, "file");
    }
    inside.wire(base, "outcolor", surface, "base_color");
    inside.wire(occlusion, "out", surface, "occlusion");
    ImportedMaterial {
        key: "/Looks/Box".into(),
        name: "Box".into(),
        inside,
        standard: None,
        double_sided: false,
    }
}

#[test]
fn kinogakis_textures_travel_as_materialx_ones_and_come_back_as_themselves() {
    let dir = Scratch::new("gltf");
    let path = dir.join("box.usdz");
    let file = EmbeddedFile::new("box.png", vec![1, 2, 3, 4]);
    let written = gltf(&file);
    write_file(&path, &items(vec![written.clone()]), metres());
    let mut stage = stage(&path);
    let scene = scene::read(&mut stage, &ReadOptions::default()).unwrap();
    let material = &scene.materials[0];
    // Other applications know every shader's id: Kinogaki's textures are
    // MaterialX's glTF ones, their sampler's filters beside.
    let ids: Vec<&str> = (material.nodes.iter())
        .filter_map(|n| n.id().map(|id| id.as_str()))
        .collect();
    assert!(ids.contains(&"ND_gltf_colorimage"), "{ids:?}");
    assert!(ids.contains(&"ND_gltf_image_float_float_1_0"), "{ids:?}");
    assert!(!ids.iter().any(|id| id.contains("kinogaki")), "{ids:?}");
    let mtlx = material.surface("mtlx").unwrap();
    let base = &material.nodes[mtlx.input("base_color").unwrap().sources[0].node.unwrap()];
    assert!(base.input("minfilter").is_some());
    let spare = (material.nodes.iter())
        .find(|n| n.path.name() == "Spare")
        .unwrap();
    let order = spare.input("operationorder").and_then(|p| p.value.clone());
    assert_eq!(order, Some(kinogaki_usd::usd::sdf::Value::Int(0)));
    // Their USD Preview Surface shows the base color's texture.
    let preview = material.surface("").unwrap();
    let diffuse = preview
        .input("diffuseColor")
        .expect("a textured diffuse color");
    let texture = &material.nodes[diffuse.sources[0].node.unwrap()];
    assert_eq!(texture.id().unwrap().as_str(), "UsdUVTexture");
    // Read back, each texture is Kinogaki's again, holding what it held
    // and, the occlusion, its filter at its default.
    let read = read_file(&path, metres(), false);
    let back = &materials(&read).materials[0];
    let mut expected = written.inside.clone();
    let none = Value::Choice(Arc::from(""));
    expected.nodes[2] = (expected.nodes[2].clone()).with("minfilter", none.clone());
    expected.nodes[3] = (expected.nodes[3].clone())
        .with("minfilter", none)
        .with("operationorder", Value::Integer(0));
    assert_eq!(shape(&back.inside), shape(&expected));
}

/// Two triangles, the first showing material `a`, the second `b`.
fn two_faces(a: &str, b: &str) -> Geometry {
    let positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    let faces = Primitives::of_polygons(4, vec![0, 3, 6].into(), vec![0, 1, 2, 0, 2, 3].into());
    let material = Attribute::text(Text::from_strs([a, b]));
    let attributes = Attributes::new().with(Domain::Primitive, names::MATERIAL, material);
    Geometry::new(positions.into(), faces.unwrap(), attributes.unwrap()).unwrap()
}

#[test]
fn a_material_keeps_its_sides_whatever_shows_it() {
    let dir = Scratch::new("sides");
    let path = dir.join("sides.usda");
    let file = EmbeddedFile::new("box.png", vec![1, 2, 3, 4]);
    // A single-sided material no mesh shows (one of a glTF's variants), and
    // a double-sided one on a mesh beside a single-sided one.
    let spare = ImportedMaterial {
        key: "/Looks/Spare".into(),
        name: "Spare".into(),
        ..gltf(&file)
    };
    let double = ImportedMaterial {
        double_sided: true,
        ..wood(&file)
    };
    let single = ImportedMaterial {
        key: "/Looks/Single".into(),
        name: "Single".into(),
        ..wood(&file)
    };
    let shared = two_faces(&double.key, &single.key);
    let looks = Materials {
        materials: vec![spare, double, single],
    };
    let items = vec![
        FileItem::top("Looks", standard::materials().wrap(looks)),
        FileItem::top("Shared", standard::geometry().wrap(shared)),
    ];
    write_file(&path, &items, metres());
    let read = read_file(&path, metres(), false);
    let mut sides: Vec<(&str, bool)> = (materials(&read).materials.iter())
        .map(|m| (m.name.as_str(), m.double_sided))
        .collect();
    sides.sort();
    assert_eq!(sides, [("Single", false), ("Spare", false), ("Wood", true)]);
}

#[test]
fn a_materials_lengths_move_into_the_projects_units() {
    use kinogaki_studio_api::geometry::{Units, UpAxis};
    let dir = Scratch::new("lengths");
    let path = dir.join("glass.usda");
    let file = EmbeddedFile::new("box.png", vec![1, 2, 3, 4]);
    let millimetres = Units::new(0.001, UpAxis::Y).unwrap();
    let centimetres = Units::new(0.01, UpAxis::Y).unwrap();
    write_file(&path, &items(vec![gltf(&file)]), millimetres);
    let read = read_file(&path, centimetres, false);
    let back = &materials(&read).materials[0];
    let surface = back.inside.nodes.iter().find(|n| n.display).unwrap();
    // 2 mm and 0.5 mm, in centimetres.
    let held = |name: &str| match surface.parameters.get(name) {
        Some(Value::Number(n)) => *n,
        other => panic!("{name}: {other:?}"),
    };
    assert!(
        (held("thickness") - 0.2).abs() < 1e-9,
        "{}",
        held("thickness")
    );
    assert!((held("attenuation_distance") - 0.05).abs() < 1e-9);
    // In the units it was written in, it reads as it was.
    let read = read_file(&path, millimetres, false);
    let back = &materials(&read).materials[0];
    assert_eq!(
        shape(&back.inside).0["Surface"],
        shape(&gltf(&file).inside).0["Surface"]
    );
}

/// A raw normal map decoded by its texture's scale and bias, as McUsd's
/// are, beside a surface color.
const NORMAL_MAP: &str = r#"#usda 1.0
(
    metersPerUnit = 1
)
def Mesh "Block" (
    prepend apiSchemas = ["MaterialBindingAPI"]
)
{
    int[] faceVertexCounts = [3]
    int[] faceVertexIndices = [0, 1, 2]
    point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
    rel material:binding = </Looks/Block>
}

def Scope "Looks"
{
    def Material "Block"
    {
        token outputs:surface.connect = </Looks/Block/PreviewSurface.outputs:surface>

        def Shader "PreviewSurface"
        {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor = (0.25, 0.25, 0.25)
            normal3f inputs:normal.connect = </Looks/Block/normal_texture.outputs:rgb>
            token outputs:surface
        }

        def Shader "normal_texture"
        {
            uniform token info:id = "UsdUVTexture"
            float4 inputs:bias = (-1, 1, -1, -1)
            float4 inputs:fallback = (0.5, 0.5, 1, 1)
            asset inputs:file = @normal.png@
            float4 inputs:scale = (2, -2, 2, 2)
            token inputs:sourceColorSpace = "raw"
            float3 outputs:rgb
        }
    }
}
"#;

#[test]
fn a_textures_scale_bias_and_fallback_are_numbers_read_and_written() {
    let dir = Scratch::new("scale-bias");
    let path = dir.join("block.usda");
    std::fs::write(&path, NORMAL_MAP).unwrap();
    std::fs::write(dir.join("normal.png"), b"a picture").unwrap();
    // Read where colors convert: the surface's color does, the texture's
    // numbers don't.
    let read = read_in(&path, &Doubling);
    let seed = &materials(&read).materials[0].inside;
    let named = |name: &str| {
        let node = seed.nodes.iter().find(|n| n.name.as_deref() == Some(name));
        &node.unwrap().parameters
    };
    let texture = named("normal_texture");
    let numbers = |n: [f64; 4]| Some(Value::Numbers(n.to_vec()));
    assert_eq!(
        texture.get("scale").cloned(),
        numbers([2.0, -2.0, 2.0, 2.0])
    );
    assert_eq!(
        texture.get("bias").cloned(),
        numbers([-1.0, 1.0, -1.0, -1.0])
    );
    assert_eq!(
        texture.get("fallback").cloned(),
        numbers([0.5, 0.5, 1.0, 1.0])
    );
    let surface = named("PreviewSurface");
    assert_eq!(surface.get("diffuseColor"), Some(&Value::Color([0.5; 3])));
    // Written, each is as it was authored.
    let again = dir.join("again.usda");
    write_in(&again, &read.items, &Doubling);
    let text = std::fs::read_to_string(&again).unwrap();
    for spelled in [
        "float4 inputs:scale = (2, -2, 2, 2)",
        "float4 inputs:bias = (-1, 1, -1, -1)",
        "float4 inputs:fallback = (0.5, 0.5, 1, 1)",
        "color3f inputs:diffuseColor = (0.25, 0.25, 0.25)",
    ] {
        assert!(text.contains(spelled), "{spelled} in\n{text}");
    }
}

#[test]
#[ignore = "hand: Apple's usdchecker, a local oracle"]
fn hand_run_usdchecker_arkit_passes_materialx_and_preview_surface_usdz() {
    let dir = Scratch::new("arkit");
    let path = dir.join("table.usdz");
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
    let gltf = gltf(&EmbeddedFile::new("box.png", png.to_vec()));
    write_file(&path, &items(vec![wood, leaf, gltf]), metres());
    let checked = std::process::Command::new("usdchecker")
        .arg("--arkit")
        .arg(&path)
        .output()
        .expect("usdchecker runs");
    let said = String::from_utf8_lossy(&checked.stdout).into_owned()
        + &String::from_utf8_lossy(&checked.stderr);
    assert!(checked.status.success(), "{said}");
}
