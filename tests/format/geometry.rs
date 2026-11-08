//! Geometry written and read back with everything it holds: a mesh's
//! material subsets once however many round trips (named after the group
//! holding their faces), its `name` and `part`; curves' attributes,
//! materials and closing; points' attributes; and no gprim holding prims.
use crate::support::{Scratch, item, metres, read_file, square, write_file};
use kinogaki_studio_api::geometry::{
    Attribute, Attributes, Domain, Elements, Geometry, Kind, Meaning, Primitives, Text, names,
};
use kinogaki_studio_api::network::{NetworkSeed, NewNode};
use kinogaki_studio_api::standard::{self, ImportedMaterial, Materials};
use kinogaki_studio_api::{FileItem, Imported};

/// A library of plain materials keyed and named `names`.
fn looks(names: &[&str]) -> FileItem {
    let material = |name: &&str| {
        let mut inside = NetworkSeed::default();
        let surface = NewNode::materialx("ND_open_pbr_surface_surfaceshader").unwrap();
        inside.node(surface.displayed());
        ImportedMaterial {
            key: format!("/Looks/{name}"),
            name: (*name).to_owned(),
            inside,
            standard: None,
            double_sided: true,
        }
    };
    let materials = names.iter().map(material).collect();
    FileItem::top("Looks", standard::materials().wrap(Materials { materials }))
}

/// The geometry of the item named `name`.
fn geometry<'a>(read: &'a Imported, name: &str) -> &'a Geometry {
    standard::geometry()
        .get(item(read, name).1.unwrap())
        .unwrap()
}

/// The texts of `geometry`'s attribute `name` on `domain`.
fn texts(geometry: &Geometry, domain: Domain, name: &str) -> Vec<String> {
    let text = geometry.attributes().text(domain, name).unwrap();
    (0..text.len())
        .map(|k| text.get(k).unwrap_or("").to_owned())
        .collect()
}

/// The names of the GeomSubsets a layer's text holds.
fn subsets(layer: &str) -> Vec<&str> {
    (layer.lines())
        .filter_map(|l| l.trim().strip_prefix("def GeomSubset \""))
        .filter_map(|l| l.split('"').next())
        .collect()
}

#[test]
fn a_meshs_subsets_come_back_once_with_its_names_and_parts() {
    let dir = Scratch::new("usd-subsets");
    // Two faces: Red and Blue, the group `front` holding Red's face and
    // `all` both; named and of two parts.
    let positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    let table =
        Primitives::of_polygons(4, vec![0, 3, 6].into(), vec![0, 1, 2, 0, 2, 3].into()).unwrap();
    let text = |values: [&str; 2]| Attribute::text(Text::from_strs(values));
    let attributes = Attributes::new()
        .with(
            Domain::Primitive,
            names::MATERIAL,
            text(["/Looks/Red", "/Looks/Blue"]),
        )
        .and_then(|a| a.with(Domain::Primitive, names::NAME, text(["lid", "body"])))
        .and_then(|a| a.with(Domain::Primitive, names::PART, text(["Lid", "Body"])))
        .and_then(|a| a.with_group(Domain::Primitive, "front", Elements::from_indices(2, [0])?))
        .and_then(|a| a.with_group(Domain::Primitive, "all", Elements::from_indices(2, [0, 1])?))
        .unwrap();
    let pot = Geometry::new(positions.into(), table, attributes).unwrap();
    let items = [
        looks(&["Red", "Blue"]),
        FileItem::top("Pot", standard::geometry().wrap(pot)),
    ];
    let first = dir.join("first.usda");
    write_file(&first, &items, metres());
    let text = std::fs::read_to_string(&first).unwrap();
    assert_eq!(
        subsets(&text),
        ["front", "material_1", "Lid", "Body", "all"]
    );
    assert!(
        text.contains("string[] primvars:name = [\"lid\", \"body\"]"),
        "{text}"
    );
    assert_eq!(
        text.matches("uniform token familyName = \"part\"").count(),
        2
    );
    let read = read_file(&first, metres(), false);
    let back = geometry(&read, "Pot");
    let looks = texts(back, Domain::Primitive, names::MATERIAL);
    assert_eq!(looks, ["/first/Looks/Red", "/first/Looks/Blue"]);
    assert_eq!(texts(back, Domain::Primitive, names::NAME), ["lid", "body"]);
    assert_eq!(texts(back, Domain::Primitive, names::PART), ["Lid", "Body"]);
    for group in ["front", "all", "material_1"] {
        assert!(
            back.attributes().group(Domain::Primitive, group).is_some(),
            "{group}"
        );
    }
    // Written again, it holds the same subsets: none doubles.
    let again = dir.join("again.usda");
    write_file(&again, &read.items, metres());
    let text = std::fs::read_to_string(&again).unwrap();
    assert_eq!(
        subsets(&text),
        ["front", "material_1", "Lid", "Body", "all"]
    );
}

#[test]
fn curves_keep_their_attributes_materials_and_closing() {
    let dir = Scratch::new("usd-curves");
    let path = dir.join("wires.usda");
    // An open polyline and a closed one (ending on its first point), each
    // of its own heat, colored by point, both Red.
    let positions: Vec<[f64; 3]> = (0..6).map(|i| [i as f64, (i % 2) as f64, 0.0]).collect();
    let kinds = vec![Kind::POLYLINE; 2];
    let vertices = vec![0, 1, 2, 3, 4, 5, 3];
    let table =
        Primitives::new(6, vec![0, 3, 7].into(), vertices.into(), kinds.into(), None).unwrap();
    let colors: Vec<[f32; 3]> = (0..6).map(|i| [i as f32 / 5.0, 0.5, 0.25]).collect();
    let attributes = Attributes::new()
        .with(
            Domain::Point,
            names::CD,
            Attribute::tuples(colors.clone(), Meaning::Color).unwrap(),
        )
        .and_then(|a| {
            let heat = Attribute::tuples(vec![1.5f32, 2.5], Meaning::None)?;
            a.with(Domain::Primitive, "heat", heat)
        })
        .and_then(|a| {
            let red = Attribute::text(Text::from_strs(["/Looks/Red"; 2]));
            a.with(Domain::Primitive, names::MATERIAL, red)
        })
        .unwrap();
    let wires = Geometry::new(positions.into(), table, attributes).unwrap();
    let items = [
        looks(&["Red"]),
        FileItem::top("Wires", standard::geometry().wrap(wires)),
    ];
    let said = write_file(&path, &items, metres());
    assert!(said.is_empty(), "{said:?}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("uniform token wrap = \"periodic\""), "{text}");
    assert_eq!(
        text.matches("rel material:binding = </wires/Looks/Red>")
            .count(),
        2
    );
    let read = read_file(&path, metres(), false);
    let said: Vec<&str> = read.warnings.iter().map(|w| w.message.as_str()).collect();
    assert!(said.is_empty(), "{said:?}\n{text}");
    // The open curve and the closed one, each a prim of its own.
    for (name, at, heat) in [("Curves", 0..3, 1.5), ("Curves_1", 3..6, 2.5)] {
        let back = geometry(&read, name);
        let table = back.primitives();
        assert_eq!(table.kind(0), Kind::POLYLINE);
        let ends = table.primitive(0);
        assert_eq!(ends.len(), if name == "Curves" { 3 } else { 4 }, "{name}");
        assert_eq!(
            ends.first() == ends.last(),
            name == "Curves_1",
            "{name} closes"
        );
        let cd = back
            .attributes()
            .tuples::<[f32; 3]>(Domain::Point, names::CD)
            .unwrap();
        assert_eq!(cd, &colors[at], "{name}");
        let hot = back
            .attributes()
            .tuples::<f32>(Domain::Primitive, "heat")
            .unwrap();
        assert_eq!(hot, [heat], "{name}");
        let looks = texts(back, Domain::Primitive, names::MATERIAL);
        assert_eq!(looks, ["/wires/Looks/Red"], "{name}");
    }
}

#[test]
fn points_keep_their_attributes() {
    let dir = Scratch::new("usd-points");
    let path = dir.join("cloud.usda");
    let positions: Vec<[f64; 3]> = (0..3).map(|i| [i as f64, 0.0, 0.0]).collect();
    let table = Primitives::new(
        3,
        vec![0].into(),
        Vec::new().into(),
        Vec::new().into(),
        None,
    )
    .unwrap();
    let heat = vec![0.25f32, 0.5, 0.75];
    let attributes = Attributes::new()
        .with(
            Domain::Point,
            "heat",
            Attribute::tuples(heat.clone(), Meaning::None).unwrap(),
        )
        .and_then(|a| {
            let width = Attribute::tuples(vec![0.1f32; 3], Meaning::None)?;
            a.with(Domain::Point, names::WIDTH, width)
        })
        .and_then(|a| {
            a.with(
                Domain::Point,
                names::ID,
                Attribute::tuples(vec![7i64, 8, 9], Meaning::None)?,
            )
        })
        .unwrap();
    let cloud = Geometry::new(positions.into(), table, attributes).unwrap();
    write_file(
        &path,
        &[FileItem::top("Cloud", standard::geometry().wrap(cloud))],
        metres(),
    );
    let read = read_file(&path, metres(), false);
    let back = geometry(&read, "Cloud");
    let attributes = back.attributes();
    assert_eq!(
        attributes.tuples::<f32>(Domain::Point, "heat").unwrap(),
        heat
    );
    assert_eq!(
        attributes
            .tuples::<f32>(Domain::Point, names::WIDTH)
            .unwrap(),
        [0.1; 3]
    );
    assert_eq!(
        attributes.tuples::<i64>(Domain::Point, names::ID).unwrap(),
        [7, 8, 9]
    );
}

#[test]
fn no_gprim_holds_prims() {
    let dir = Scratch::new("usd-encapsulation");
    let path = dir.join("rig.usda");
    // A panel holding a bolt, each polygons.
    let panel = FileItem::top("Panel", standard::geometry().wrap(square("")));
    let bolt = FileItem {
        parent: Some(0),
        ..FileItem::top("Bolt", standard::geometry().wrap(square("")))
    };
    write_file(&path, &[panel, bolt], metres());
    let text = std::fs::read_to_string(&path).unwrap();
    for def in [
        "def Xform \"Panel\"",
        "def Mesh \"Geometry\"",
        "def Mesh \"Bolt\"",
    ] {
        assert!(text.contains(def), "{def} in\n{text}");
    }
    // Read back, the panel is a group holding its polygons and the bolt.
    let read = read_file(&path, metres(), false);
    let (panel, value) = item(&read, "Panel");
    assert!(value.is_none());
    let at = read
        .items
        .iter()
        .position(|i| i.name == panel.name)
        .unwrap();
    let under: Vec<&str> = (read.items.iter())
        .filter(|i| i.parent == Some(at))
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(under, ["Geometry", "Bolt"]);
}

#[test]
fn prims_carry_model_kinds() {
    use kinogaki_render_api::standard::Light;
    use kinogaki_studio_api::geometry::{Vec3, Xform};
    use kinogaki_usd::usd::scene::{ReadOptions, read};
    let dir = Scratch::new("usd-kinds");
    let path = dir.join("shelf.usda");
    let cup = standard::geometry().wrap(square(""));
    let under = |parent: usize, name: &str, data| FileItem {
        parent: Some(parent),
        ..FileItem::top(name, data)
    };
    let mut rack = FileItem {
        parent: Some(0),
        data: None,
        ..FileItem::top("Rack", cup.clone())
    };
    rack.placement = Xform::translation(Vec3::new(0.0, 1.0, 0.0));
    let items = [
        FileItem {
            data: None,
            ..FileItem::top("Shelf", cup.clone())
        },
        under(0, "Board", standard::geometry().wrap(square(""))),
        under(0, "Lamp", standard::light().wrap(Light::default()).unwrap()),
        under(0, "Cup", cup.clone()),
        under(0, "Mug", cup),
        rack,
        under(5, "Bolt", standard::geometry().wrap(square(""))),
    ];
    let mut with_looks = vec![looks(&["Red"])];
    with_looks.extend(items.into_iter().map(|mut i| {
        i.parent = i.parent.map(|p| p + 1);
        i
    }));
    with_looks[0].parent = Some(1);
    write_file(&path, &with_looks, metres());
    let mut stage = crate::support::stage(&path);
    let scene = read(&mut stage, &ReadOptions::default()).unwrap();
    let models: Vec<(String, &str)> = (0..scene.nodes.len())
        .map(|k| (scene.path(k).to_string(), scene.nodes[k].model.as_str()))
        .collect();
    let expected = [
        ("/Shelf", "assembly"),
        ("/Shelf/Looks", ""),
        ("/Shelf/Board", "component"),
        ("/Shelf/Lamp", ""),
        ("/Shelf/Cup", "component"),
        ("/Shelf/Mug", "component"),
        ("/Shelf/Rack", "group"),
        ("/Shelf/Rack/Bolt", "component"),
    ];
    let expected: Vec<(String, &str)> = expected
        .iter()
        .map(|(p, k)| ((*p).to_owned(), *k))
        .collect();
    assert_eq!(models, expected);
    // The prototype Cup and Mug show is a component, as they are.
    let text = std::fs::read_to_string(&path).unwrap();
    let store = text.split("class \"Prototypes\"").nth(1).unwrap();
    assert!(store.contains("kind = \"component\""), "{store}");
}
