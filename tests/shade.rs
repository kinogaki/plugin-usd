//! Materials as UsdShade networks: the three render contexts, connections,
//! interface inputs, bindings and MaterialX documents, read as authored
//! and written back unchanged.

mod common;

use common::{fixture, node, round_trips, scene_of};
use kinogaki_usd::usd::package::Bytes;
use kinogaki_usd::usd::scene::{self, AssetSource, ReadOptions};
use kinogaki_usd::usd::sdf::{Path, Token, Value};
use kinogaki_usd::usd::shade::{ShadeKind, Source};
use kinogaki_usd::usd::{Stage, StageOptions};

/// DESIGN §8.2's material: an engine's own surface, its OpenPBR form and
/// its preview, each behind its context's terminal.
const THREE_CONTEXTS: &[u8] = br#"#usda 1.0
def Xform "Kitchen"
{
    def Mesh "Kettle" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
        rel material:binding = </Kitchen/Materials/Red_Anodized>
    }

    def Scope "Materials"
    {
        def Material "Red_Anodized" (
            customData = {
                string "kinogaki:name" = "Red Anodized, Polished"
            }
        )
        {
            token outputs:kinogaki:surface.connect = </Kitchen/Materials/Red_Anodized/Surface.outputs:out>
            token outputs:mtlx:surface.connect = </Kitchen/Materials/Red_Anodized/OpenPBR.outputs:out>
            token outputs:surface.connect = </Kitchen/Materials/Red_Anodized/Preview.outputs:surface>

            def Shader "Surface"
            {
                uniform token info:id = "kinogaki.spectre.anodized_aluminum_surface"
                color3d inputs:color = (0.62, 0.04, 0.03) (
                    colorSpace = "ACEScg"
                )
                token inputs:finish = "polished"
                double inputs:gloss = 0.85
                token outputs:out
            }

            def Shader "OpenPBR"
            {
                uniform token info:id = "ND_open_pbr_surface_surfaceshader"
                float inputs:base_metalness = 1
                color3f inputs:coat_color.connect = </Kitchen/Materials/Red_Anodized/Tint.outputs:out>
                token outputs:out
            }

            def NodeGraph "Tint"
            {
                color3f inputs:tint = (0.62, 0.04, 0.03)
                color3f outputs:out.connect = </Kitchen/Materials/Red_Anodized/Tint/Multiply.outputs:out>

                def Shader "Multiply"
                {
                    uniform token info:id = "ND_multiply_color3"
                    color3f inputs:in1.connect = </Kitchen/Materials/Red_Anodized/Tint.inputs:tint>
                    color3f inputs:in2 = (1, 1, 1)
                    color3f outputs:out
                }
            }

            def Shader "Preview"
            {
                uniform token info:id = "UsdPreviewSurface"
                color3f inputs:diffuseColor.connect = </Kitchen/Materials/Red_Anodized/Wood.outputs:rgb>
                float inputs:metallic = 1
                float inputs:roughness = 0.08
                token outputs:surface
            }

            def Shader "Wood"
            {
                uniform token info:id = "UsdUVTexture"
                asset inputs:file = @textures/wood.png@
                token inputs:sourceColorSpace = "sRGB"
                token inputs:wrapS = "repeat"
                float2 inputs:st.connect = </Kitchen/Materials/Red_Anodized/Coordinates.outputs:result>
                float3 outputs:rgb
            }

            def Shader "Coordinates"
            {
                uniform token info:id = "UsdPrimvarReader_float2"
                string inputs:varname = "st"
                float2 outputs:result
            }
        }
    }
}
"#;

#[test]
fn a_material_holds_every_render_context_as_authored() {
    let scene = scene_of(THREE_CONTEXTS);
    assert_eq!(scene.materials.len(), 1);
    let material = &scene.materials[0];
    assert_eq!(material.path.text(), "/Kitchen/Materials/Red_Anodized");
    assert_eq!(
        material.custom_data.get("kinogaki:name"),
        Some(&Value::String(Token::new("Red Anodized, Polished")))
    );
    let id = |context| material.surface(context).unwrap().id().unwrap().to_string();
    assert_eq!(id("kinogaki"), "kinogaki.spectre.anodized_aluminum_surface");
    assert_eq!(id("mtlx"), "ND_open_pbr_surface_surfaceshader");
    assert_eq!(id(""), "UsdPreviewSurface");
    let engine = material.surface("kinogaki").unwrap();
    let color = engine.input("color").unwrap();
    assert_eq!(
        (color.type_name.as_str(), color.color_space.as_deref()),
        ("color3d", Some("ACEScg"))
    );
    assert_eq!(
        engine.input("finish").unwrap().value,
        Some(Value::Token(Token::new("polished")))
    );
    // The kettle binds it.
    assert_eq!(node(&scene, "/Kitchen/Kettle").material, Some(0));
}

#[test]
fn connections_resolve_to_nodes_and_node_graph_interfaces() {
    let scene = scene_of(THREE_CONTEXTS);
    let material = &scene.materials[0];
    let at = |path: &str| material.node(&Path::parse(path).unwrap()).unwrap();
    let tint = at("/Kitchen/Materials/Red_Anodized/Tint");
    let multiply = at("/Kitchen/Materials/Red_Anodized/Tint/Multiply");
    assert_eq!(material.nodes[tint].kind, ShadeKind::NodeGraph);
    assert_eq!(material.nodes[multiply].parent, Some(tint));
    // The graph's output is its inner node's; the node reads the graph's input.
    let output = &material.nodes[tint].outputs[0];
    assert_eq!(
        output.sources,
        [Source {
            node: Some(multiply),
            port: "out".into(),
            output: true
        }]
    );
    let in1 = material.nodes[multiply].input("in1").unwrap();
    assert_eq!(
        in1.sources,
        [Source {
            node: Some(tint),
            port: "tint".into(),
            output: false
        }]
    );
    let preview = material.surface("").unwrap();
    let wood = at("/Kitchen/Materials/Red_Anodized/Wood");
    let diffuse = preview.input("diffuseColor").unwrap();
    assert_eq!(
        diffuse.sources,
        [Source {
            node: Some(wood),
            port: "rgb".into(),
            output: true
        }]
    );
    assert!(material.nodes[at("/Kitchen/Materials/Red_Anodized/Preview")].info[0].uniform);
}

#[test]
fn texture_files_are_the_scenes_assets() {
    let scene = scene_of(THREE_CONTEXTS);
    let wood = scene.materials[0]
        .nodes
        .iter()
        .find(|n| n.path.name() == "Wood")
        .unwrap();
    let Some(Value::Asset(file)) = &wood.input("file").unwrap().value else {
        panic!("an asset")
    };
    let asset = scene.asset(file).unwrap();
    assert_eq!(asset.authored.as_str(), "textures/wood.png");
    // A file in memory names nothing beside it.
    assert_eq!(asset.source, AssetSource::Missing);
    assert!(
        scene
            .warnings
            .iter()
            .any(|w| w == "textures/wood.png: no such file"),
        "{:?}",
        scene.warnings
    );
}

#[test]
fn networks_write_and_read_back_unchanged() {
    // A file read from disk names its textures by absolute paths; once
    // written and read from memory, a round trip is exact.
    let [from_disk, _] = round_trips(&fixture("cube.usda"));
    for scene in [scene_of(THREE_CONTEXTS), from_disk] {
        for again in round_trips(&scene) {
            assert_eq!(again.materials, scene.materials);
            assert_eq!(again.nodes, scene.nodes);
        }
    }
}

#[test]
fn bindings_follow_usd_shade() {
    let scene = scene_of(
        br#"#usda 1.0
def Xform "World" (
    prepend apiSchemas = ["MaterialBindingAPI"]
)
{
    rel material:binding = </Looks/Plain>

    def Mesh "Inherits"
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
    }

    def Mesh "Full" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
        rel material:binding = </Looks/Plain>
        rel material:binding:full = </Looks/Rich>
    }

    def Xform "Strong" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        rel material:binding = </Looks/Rich> (
            bindMaterialAs = "strongerThanDescendants"
        )

        def Mesh "Overridden" (
            prepend apiSchemas = ["MaterialBindingAPI"]
        )
        {
            int[] faceVertexCounts = [3]
            int[] faceVertexIndices = [0, 1, 2]
            point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
            rel material:binding = </Looks/Plain>
        }
    }
}

def Scope "Looks"
{
    def Material "Plain" {}
    def Material "Rich" {}
    def Material "Unbound" {}
}
"#,
    );
    let bound = |path| {
        let index = node(&scene, path).material.unwrap();
        scene.materials[index].path.name().to_owned()
    };
    assert_eq!(bound("/World/Inherits"), "Plain");
    assert_eq!(bound("/World/Full"), "Rich", "the full purpose wins");
    assert_eq!(
        bound("/World/Strong/Overridden"),
        "Rich",
        "stronger than descendants"
    );
    let names: Vec<&str> = scene.materials.iter().map(|m| m.path.name()).collect();
    assert_eq!(
        names,
        ["Plain", "Rich", "Unbound"],
        "unbound materials are read too"
    );
}

#[test]
fn a_connection_leaving_its_material_is_said() {
    let scene = scene_of(
        br#"#usda 1.0
def Material "A"
{
    token outputs:surface.connect = </B/Surface.outputs:surface>
}
def Material "B"
{
    def Shader "Surface"
    {
        uniform token info:id = "UsdPreviewSurface"
        token outputs:surface
    }
}
"#,
    );
    assert!(scene.materials[0].outputs[0].sources.is_empty());
    assert!(
        scene
            .warnings
            .iter()
            .any(|w| w.contains("leaves the material /A")),
        "{:?}",
        scene.warnings
    );
}

/// A MaterialX document, as UsdMtlx lays its materials out.
const STEEL_MTLX: &[u8] = br#"<?xml version="1.0"?>
<materialx version="1.39">
  <open_pbr_surface name="SR_steel" type="surfaceshader">
    <input name="base_metalness" type="float" value="1" />
  </open_pbr_surface>
  <surfacematerial name="Steel" type="material">
    <input name="surfaceshader" type="surfaceshader" nodename="SR_steel" />
  </surfacematerial>
</materialx>
"#;

#[test]
fn referenced_materialx_documents_are_kept_for_their_reader() {
    let layer = br#"#usda 1.0
(
    defaultPrim = "Root"
)
def Xform "Root"
{
    def "MaterialX" (
        references = @./steel.mtlx@</MaterialX>
    )
    {
    }

    def Mesh "Beam" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
        rel material:binding = </Root/MaterialX/Materials/Steel>
    }

    def Material "Chrome" (
        references = @./steel.mtlx@</MaterialX/Materials/Steel>
    )
    {
    }
}
"#;
    // In a package, the document travels with the layer.
    let package = kinogaki_usd::usd::package::write_usdz(
        &kinogaki_usd::usd::text::read_usda(layer).unwrap(),
        "root.usda",
        &[("steel.mtlx", STEEL_MTLX)],
    )
    .unwrap();
    let mut stage =
        Stage::open_memory("beam.usdz", Bytes::from(package), &StageOptions::default()).unwrap();
    let scene = scene::read(&mut stage, &ReadOptions::default()).unwrap();
    assert!(
        !stage.warnings().iter().any(|w| w.contains("steel.mtlx")),
        "{:?}",
        stage.warnings()
    );
    let steel = scene
        .material(&Path::parse("/Root/MaterialX/Materials/Steel").unwrap())
        .unwrap();
    assert!(steel.nodes.is_empty());
    assert_eq!(steel.mtlx[0].target.text(), "/MaterialX/Materials/Steel");
    let chrome = scene
        .material(&Path::parse("/Root/Chrome").unwrap())
        .unwrap();
    assert_eq!(chrome.mtlx[0].target.text(), "/MaterialX/Materials/Steel");
    assert_eq!(chrome.mtlx[0].asset, steel.mtlx[0].asset);
    let document = scene.asset(&steel.mtlx[0].asset).unwrap();
    assert_eq!(
        document.source,
        AssetSource::Bytes(Bytes::from(STEEL_MTLX.to_vec()))
    );
    let beam = node(&scene, "/Root/Beam").material.unwrap();
    assert_eq!(
        scene.materials[beam].path.text(),
        "/Root/MaterialX/Materials/Steel"
    );
}
