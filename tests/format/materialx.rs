//! MaterialX documents a layer references, read as Import reads them: each
//! composed as the layer UsdMtlx makes of it, its materials networks of
//! MaterialX's nodes with their textures, and the instances showing them
//! keeping their bindings.
use crate::support::{Scratch, item, materials, metres, read_file};
use kinogaki_render_api::material::mtlx::SIGNATURE;
use kinogaki_studio_api::geometry::image::{ImageColor, ImageFile};
use kinogaki_studio_api::geometry::{Domain, names};
use kinogaki_studio_api::image_files::{IMAGE_NODE, image_source};
use kinogaki_studio_api::{Value, standard};

/// A textured Standard Surface through a node graph, MaterialX 1.38.
const STEEL: &str = r#"<?xml version="1.0"?>
<materialx version="1.38" colorspace="lin_rec709">
  <nodegraph name="NG_steel">
    <image name="tint" type="color3">
      <input name="file" type="filename" value="tex/tint.png" colorspace="srgb_texture" />
    </image>
    <output name="tint_out" type="color3" nodename="tint" />
  </nodegraph>
  <standard_surface name="SR_steel" type="surfaceshader">
    <input name="base_color" type="color3" nodegraph="NG_steel" output="tint_out" />
    <input name="metalness" type="float" value="1" />
  </standard_surface>
  <surfacematerial name="Steel" type="material">
    <input name="surfaceshader" type="surfaceshader" nodename="SR_steel" />
  </surfacematerial>
</materialx>
"#;

/// A piece whose look is the document's, as the Open Chess Set's are.
const PIECE: &str = r#"#usda 1.0
(
    defaultPrim = "Piece"
    metersPerUnit = 1
)
def Xform "Piece"
{
    def Mesh "Body" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
        rel material:binding = </Piece/Materials/Steel>
    }

    def Scope "Materials" (
        prepend references = @./steel.mtlx@</MaterialX/Materials>
    )
    {
    }
}
"#;

/// Two instanceable pieces and one of its own.
const BOARD: &str = r#"#usda 1.0
(
    metersPerUnit = 1
)
def Xform "Board"
{
    def Xform "Left" (
        instanceable = true
        references = @./piece.usda@
    )
    {
    }

    def Xform "Right" (
        instanceable = true
        references = @./piece.usda@
    )
    {
    }

    def Xform "Loose" (
        references = @./piece.usda@
    )
    {
    }
}
"#;

#[test]
fn a_referenced_materialx_documents_materials_import_with_their_textures() {
    let dir = Scratch::new("usd-mtlx");
    std::fs::write(dir.join("steel.mtlx"), STEEL).unwrap();
    std::fs::write(dir.join("piece.usda"), PIECE).unwrap();
    std::fs::create_dir_all(dir.join("tex")).unwrap();
    std::fs::write(dir.join("tex/tint.png"), b"a picture").unwrap();
    let path = dir.join("board.usda");
    std::fs::write(&path, BOARD).unwrap();
    let read = read_file(&path, metres(), false);
    let said: Vec<&str> = read.warnings.iter().map(|w| w.message.as_str()).collect();
    assert!(said.is_empty(), "{said:?}");
    // The instances' prototype is named by its source, the first of them.
    let looks = materials(&read);
    let keys: Vec<&str> = looks.materials.iter().map(|m| m.key.as_str()).collect();
    let shared = "/Board/Left/Materials/Steel";
    assert_eq!(keys, [shared, "/Board/Loose/Materials/Steel"]);
    for steel in &looks.materials {
        assert_eq!(steel.name, "Steel");
        let seed = &steel.inside;
        let surface = seed.nodes.iter().position(|n| n.display).unwrap();
        assert_eq!(
            seed.nodes[surface].parameters.get(SIGNATURE),
            Some(&Value::Choice("ND_standard_surface_surfaceshader".into()))
        );
        assert_eq!(
            seed.nodes[surface].parameters.get("metalness"),
            Some(&Value::Number(1.0))
        );
        // Its texture, relative to the document, read as sRGB.
        let image = seed.nodes.iter().position(|n| n.node_type == IMAGE_NODE);
        let image = image.expect("an Image node");
        let source = image_source(&seed.nodes[image].parameters)
            .unwrap()
            .unwrap();
        let tint = dir.join("tex/tint.png").canonicalize().unwrap();
        let tint = tint.display().to_string();
        assert_eq!(*source.file(), ImageFile::Path(tint.as_str().into()));
        assert_eq!(*source.color(), ImageColor::space("srgb_texture").unwrap());
        let wired = seed.wires.iter().find(|w| w.from == image).unwrap();
        assert!(wired.to != surface && wired.input == "file", "{wired:?}");
    }
    // Both instances show one prototype, which keeps its binding.
    let [left, right] = ["Left", "Right"].map(|name| {
        let held = item(&read, name).1.expect("an instance");
        standard::instances().get(held).unwrap().prototypes()[0].clone()
    });
    assert!(left.ptr_eq(&right));
    let body = standard::geometry().get(&left).unwrap();
    let bound = body.attributes().text(Domain::Primitive, names::MATERIAL);
    assert_eq!(bound.unwrap().get(0), Some(shared));
}
