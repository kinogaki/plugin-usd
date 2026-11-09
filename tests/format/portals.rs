//! Portals and mesh lights, from minimal layers: a dome's `portals` read as
//! those portals' dome and written back as the dome's `portals`; MeshLightAPI
//! (and LightAPI) on a Mesh read as a geometry light item beside the mesh
//! naming it, the deprecated GeometryLight as one naming its geometry, and
//! written back as MeshLightAPI on the mesh; VolumeLightAPI, LightAPI on
//! another gprim, PluginLight and a mesh light's links said and left out.
use crate::support::{Scratch, item, metres, read_file, write_file};
use kinogaki_render_api::standard::{DomeFormat, Light, LightShape, MaterialSync, Pole};
use kinogaki_studio_api::{
    FileFormat, FileItem, Imported, Input, ObjectRef, Purpose, Reading, standard,
};
use kinogaki_studio_api_testing::Unconverted;
use kinogaki_usd::Usd;

fn read(layer: &str) -> Imported {
    let reading = Reading::new(Purpose::Objects, metres(), &Unconverted);
    let input = Input::memory("portals.usda", layer.as_bytes());
    Usd.read(&input, &Usd.options(), &reading).unwrap()
}

/// The light item `name` holds, and its index among the items.
fn light(read: &Imported, name: &str) -> (Light, usize) {
    let at = (read.items.iter()).position(|i| i.name == name);
    let at = at.unwrap_or_else(|| panic!("no item {name}"));
    let held = read.items[at].data.as_ref().expect("a light");
    (standard::light().get(held).unwrap().clone(), at)
}

fn index(read: &Imported, name: &str) -> usize {
    (read.items.iter())
        .position(|i| i.name == name)
        .expect("the item")
}

fn said(read: &Imported) -> Vec<String> {
    read.warnings.iter().map(|w| w.message.clone()).collect()
}

/// A metre stage of `prims`.
fn stage(prims: &str) -> String {
    format!("#usda 1.0\n(\n    metersPerUnit = 1\n)\n{prims}")
}

const QUAD: &str = r#"int[] faceVertexCounts = [4]
        int[] faceVertexIndices = [0, 1, 2, 3]
        point3f[] points = [(-0.5, 2, -0.5), (0.5, 2, -0.5), (0.5, 2, 0.5), (-0.5, 2, 0.5)]"#;

#[test]
fn a_domes_portals_name_it_and_a_portal_no_dome_names_names_none() {
    let read = read(&stage(
        r#"def Xform "World"
{
    def DomeLight "Sky"
    {
        rel portals = [</World/Window>, </World/Missing>]
    }
    def PortalLight "Window"
    {
        float inputs:width = 2
        float inputs:height = 0.5
        float inputs:intensity = 9
    }
    def PortalLight "Unused"
    {
    }
}
"#,
    ));
    let sky = index(&read, "Sky");
    let (window, _) = light(&read, "Window");
    let expected = LightShape::Portal {
        width: 2.0,
        height: 0.5,
        dome: Some(ObjectRef(sky as u64)),
    };
    assert_eq!(window.shape, expected);
    // A portal has no emission of its own: its LightAPI inputs do nothing.
    assert_eq!(window.intensity, 1.0);
    assert!(matches!(
        light(&read, "Unused").0.shape,
        LightShape::Portal { dome: None, .. }
    ));
    let said = said(&read);
    assert!(
        said.iter()
            .any(|s| s.contains("portal /World/Missing isn't read")),
        "{said:?}"
    );
}

#[test]
fn a_mesh_light_is_a_geometry_light_beside_its_mesh() {
    let read = read(&stage(&format!(
        r#"def Xform "World"
{{
    def Mesh "Neon" (
        prepend apiSchemas = ["MeshLightAPI", "ShapingAPI"]
    )
    {{
        {QUAD}
        float inputs:intensity = 10
        bool inputs:normalize = 1
    }}
    def Mesh "Panel" (
        prepend apiSchemas = ["LightAPI"]
    )
    {{
        {QUAD}
    }}
    def Mesh "Board"
    {{
        {QUAD}
    }}
    def GeometryLight "Old"
    {{
        float inputs:intensity = 3
        rel geometry = </World/Board>
    }}
}}
"#
    )));
    let neon = index(&read, "Neon");
    let (lit, at) = light(&read, "Neon Light");
    assert_eq!(read.items[at].parent, read.items[neon].parent);
    assert_eq!(
        lit.shape,
        LightShape::Geometry {
            geometry: Some(ObjectRef(neon as u64)),
            sync: MaterialSync::GlowTintsLight,
        }
    );
    assert!((lit.intensity, lit.normalize) == (10.0, true));
    // LightAPI alone takes its own fallback: no material response.
    let (panel, _) = light(&read, "Panel Light");
    assert!(matches!(
        panel.shape,
        LightShape::Geometry {
            sync: MaterialSync::NoMaterialResponse,
            ..
        }
    ));
    let board = index(&read, "Board");
    let (old, _) = light(&read, "Old");
    assert_eq!(
        old.shape,
        LightShape::Geometry {
            geometry: Some(ObjectRef(board as u64)),
            sync: MaterialSync::NoMaterialResponse,
        }
    );
    assert_eq!(old.intensity, 3.0);
    let said = said(&read);
    assert!(
        said.iter()
            .any(|s| s.contains("/World/Neon: ShapingAPI on a GeometryLight is left out")),
        "{said:?}"
    );
}

#[test]
fn what_isnt_held_of_a_light_is_said() {
    let read = read(&stage(
        r#"def Xform "World"
{
    def Volume "Smoke" (
        prepend apiSchemas = ["VolumeLightAPI"]
    )
    {
    }
    def Sphere "Ball" (
        prepend apiSchemas = ["LightAPI"]
    )
    {
    }
    def PluginLight "Custom"
    {
        uniform token info:id = "AcmeSkyLight"
    }
    def Mesh "Linked" (
        prepend apiSchemas = ["MeshLightAPI"]
    )
    {
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
        uniform bool collection:lightLink:includeRoot = 0
    }
}
"#,
    ));
    let said = said(&read);
    for line in [
        "/World/Smoke: its VolumeLightAPI is left out",
        "/World/Ball: LightAPI on a Sphere is left out",
        "/World/Custom: PluginLight (AcmeSkyLight) names a renderer's own shader",
        "/World/Linked: its light and shadow links are left out",
    ] {
        assert!(said.iter().any(|s| s.contains(line)), "{line} in {said:?}");
    }
}

#[test]
fn portals_and_mesh_lights_write_back_and_read_back() {
    let quad = crate::support::square("/Looks/Panel");
    let dome = Light {
        shape: LightShape::Dome {
            image: None,
            format: DomeFormat::Automatic,
            pole: Pole::Y,
            ground: None,
        },
        ..Light::default()
    };
    let portal = |dome| Light {
        shape: LightShape::Portal {
            width: 2.0,
            height: 1.0,
            dome,
        },
        ..Light::default()
    };
    let lamp = Light {
        shape: LightShape::Geometry {
            geometry: Some(ObjectRef(3)),
            sync: MaterialSync::Independent,
        },
        intensity: 4.0,
        ..Light::default()
    };
    let wrap = |l: &Light| standard::light().wrap(l.clone()).unwrap();
    let items = vec![
        FileItem::top("Sky", wrap(&dome)),
        FileItem::top("Window", wrap(&portal(Some(ObjectRef(0))))),
        FileItem::top("Door", wrap(&portal(Some(ObjectRef(0))))),
        FileItem::top("Panel", standard::geometry().wrap(quad)),
        FileItem::top("Lamp", wrap(&lamp)),
    ];
    let dir = Scratch::new("portals");
    let path = dir.join("portals.usda");
    let said = write_file(&path, &items, metres());
    let lights = ["Sky", "Window", "Door", "Lamp"];
    assert!(
        said.iter().all(|s| lights.iter().all(|l| !s.contains(l))),
        "{said:?}"
    );
    let text = std::fs::read_to_string(&path).unwrap();
    for line in [
        "def PortalLight \"Window\"",
        "rel portals = [",
        "prepend apiSchemas = [\"MeshLightAPI\"]",
        "uniform token light:materialSyncMode = \"independent\"",
    ] {
        assert!(text.contains(line), "{line} in\n{text}");
    }
    assert!(
        !text.contains("\"Lamp\""),
        "a geometry light is no prim:\n{text}"
    );
    let back = read_file(&path, metres(), false);
    let sky = index(&back, "Sky");
    for name in ["Window", "Door"] {
        assert_eq!(
            light(&back, name).0,
            portal(Some(ObjectRef(sky as u64))),
            "{name}"
        );
    }
    let panel = index(&back, "Panel");
    let (lit, _) = light(&back, "Panel Light");
    let expected = Light {
        shape: LightShape::Geometry {
            geometry: Some(ObjectRef(panel as u64)),
            sync: MaterialSync::Independent,
        },
        ..lamp
    };
    assert_eq!(lit, expected);
    assert!(item(&back, "Panel").1.is_some());
}
