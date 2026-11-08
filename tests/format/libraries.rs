//! Material Libraries as USD Scopes of Materials, wherever they are: a
//! Scope of Materials alone imports as a library at its place, Materials
//! beside other prims (Scopes among them) as a library named `Materials`
//! there, and each library exports as a Scope at its place, its Materials
//! beneath, the bindings naming them there; a group holding only libraries
//! exports as a Scope.
use crate::support::{Scratch, item, libraries, metres, read_file, write_file};
use kinogaki_studio_api::geometry::{Domain, names};
use kinogaki_studio_api::{Imported, standard};

/// A Scope of Materials beside the geometry (`Looks`), one inside a piece
/// (`Piece/Materials`), a Material beside other prims (`Loose`), and a
/// Scope of Materials holding a Scope of Scopes of Materials (`mtl`,
/// `mtl/examples/preview`, as the Standard Shader Ball's `materials`).
const SCENE: &str = r#"#usda 1.0
(
    defaultPrim = "World"
    metersPerUnit = 1
)
def Xform "World"
{
    def Mesh "Ground" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
        rel material:binding = </World/Looks/Clay>
    }

    def Xform "Piece"
    {
        def Mesh "Body" (
            prepend apiSchemas = ["MaterialBindingAPI"]
        )
        {
            int[] faceVertexCounts = [3]
            int[] faceVertexIndices = [0, 1, 2]
            point3f[] points = [(0, 0, 1), (1, 0, 1), (0, 1, 1)]
            rel material:binding = </World/Piece/Materials/Brass>
        }

        def Scope "Materials"
        {
            def Material "Brass"
            {
                SURFACE
            }

            def Material "Unused"
            {
                SURFACE
            }
        }
    }

    def Mesh "Tile" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 2), (1, 0, 2), (0, 1, 2)]
        rel material:binding = </World/mtl/examples/preview/Copper>
    }

    def Material "Loose"
    {
        SURFACE
    }

    def Scope "Looks"
    {
        def Material "Clay"
        {
            SURFACE
        }
    }

    def Scope "mtl"
    {
        def Material "Neutral"
        {
            SURFACE
        }

        def Scope "examples"
        {
            def Scope "preview"
            {
                def Material "Copper"
                {
                    SURFACE
                }
            }
        }
    }
}
"#;

/// The scene, each Material a USD Preview Surface.
fn scene() -> String {
    let mut text = String::new();
    let mut at: Vec<(usize, String)> = Vec::new();
    for line in SCENE.lines() {
        let depth = line.len() - line.trim_start().len();
        if let Some(name) = line
            .trim()
            .strip_prefix("def ")
            .and_then(|d| d.split('"').nth(1))
        {
            at.retain(|(d, _)| *d < depth);
            at.push((depth, name.to_owned()));
        }
        if line.trim() == "SURFACE" {
            let path: String = at.iter().map(|(_, n)| format!("/{n}")).collect();
            let pad = &line[..depth];
            text += &format!(
                "{pad}token outputs:surface.connect = <{path}/Surface.outputs:surface>\n\
                 {pad}def Shader \"Surface\"\n{pad}{{\n\
                 {pad}    uniform token info:id = \"UsdPreviewSurface\"\n\
                 {pad}    token outputs:surface\n{pad}}}\n"
            );
        } else {
            text += line;
            text.push('\n');
        }
    }
    text
}

/// Each item's path among the items.
fn paths(read: &Imported) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for item in &read.items {
        let above = item.parent.map_or("", |p| paths[p].as_str());
        paths.push(format!("{above}/{}", item.name));
    }
    paths
}

/// The material the first face of the item named `name` shows.
fn shows(read: &Imported, name: &str) -> String {
    let polygons = standard::geometry()
        .get(item(read, name).1.unwrap())
        .unwrap();
    let text = polygons
        .attributes()
        .text(Domain::Primitive, names::MATERIAL);
    text.and_then(|t| t.get(0)).unwrap_or("").to_owned()
}

/// The libraries the scene holds, where they are.
fn expected() -> Vec<(String, Vec<String>)> {
    let library = |path: &str, names: &[&str]| {
        let names = names.iter().map(|n| (*n).to_owned()).collect();
        (path.to_owned(), names)
    };
    vec![
        library("/World/Materials", &["Loose"]),
        library("/World/Piece/Materials", &["Brass", "Unused"]),
        library("/World/Looks", &["Clay"]),
        library("/World/mtl/Materials", &["Neutral"]),
        library("/World/mtl/examples/preview", &["Copper"]),
    ]
}

#[test]
fn scopes_of_materials_import_as_libraries_where_they_are() {
    let dir = Scratch::new("usd-libraries");
    let path = dir.join("scene.usda");
    std::fs::write(&path, scene()).unwrap();
    let read = read_file(&path, metres(), false);
    assert_eq!(libraries(&read), expected());
    // A Scope holding Materials and Scopes is a group, as is one holding
    // a Scope of Materials.
    let paths = paths(&read);
    assert!(
        paths.contains(&"/World/mtl/examples".to_owned()),
        "{paths:?}"
    );
    let said: Vec<&str> = read.warnings.iter().map(|w| w.message.as_str()).collect();
    let gathered = |at: &str| {
        format!("{at}: Materials beside other prims go into a library named Materials there")
    };
    assert_eq!(said, [gathered("/World"), gathered("/World/mtl")]);
    assert_eq!(shows(&read, "Body"), "/World/Piece/Materials/Brass");
}

#[test]
fn libraries_export_as_scopes_where_they_are_and_import_back() {
    let dir = Scratch::new("usd-libraries-back");
    let path = dir.join("scene.usda");
    std::fs::write(&path, scene()).unwrap();
    let read = read_file(&path, metres(), false);
    let out = dir.join("out.usda");
    let said = write_file(&out, &read.items, metres());
    assert!(said.is_empty(), "{said:?}");
    let text = std::fs::read_to_string(&out).unwrap();
    for scope in ["Materials", "Looks", "mtl", "examples", "preview"] {
        let def = format!("def Scope \"{scope}\"");
        assert!(text.contains(&def), "{def} in\n{text}");
    }
    let back = read_file(&out, metres(), false);
    assert_eq!(libraries(&back), expected());
    let said: Vec<&str> = back.warnings.iter().map(|w| w.message.as_str()).collect();
    assert!(said.is_empty(), "{said:?}");
    for (name, key) in [
        ("Ground", "/World/Looks/Clay"),
        ("Body", "/World/Piece/Materials/Brass"),
        ("Tile", "/World/mtl/examples/preview/Copper"),
    ] {
        assert_eq!(shows(&back, name), key, "{name}");
    }
}
