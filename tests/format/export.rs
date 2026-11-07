//! Items written and read back through the format: polygons with their
//! attributes, bindings and groups where they were, in any project's
//! units; curves; instances as copies, each with its own material; lights
//! and cameras; a dome naming an image of the file's.
use crate::support::{Scratch, item, materials, metres, read_file, square, write_file};
use kinogaki_render_api::standard::{Light, Shape};
use kinogaki_render_api::{Camera, Projection};
use kinogaki_studio_api::FileItem;
use kinogaki_studio_api::geometry::image::{EmbeddedFile, ImageFile, ImageSource};
use kinogaki_studio_api::geometry::{
    Attribute, Attributes, Basis, Column, CurveData, Domain, Elements, Geometry, Kind, Knots,
    Primitives, Text, Units, UpAxis, Vec3, Xform, names,
};
use kinogaki_studio_api::image_files::OP;
use kinogaki_studio_api::network::{NetworkSeed, NewNode};
use kinogaki_studio_api::standard::{self, ImportedImage, ImportedMaterial, Instances, Materials};
use std::sync::Arc;

/// A material of no network but its OpenPBR Surface.
fn plain(key: &str, name: &str) -> ImportedMaterial {
    let mut inside = NetworkSeed::default();
    inside.node(
        NewNode::materialx("ND_open_pbr_surface_surfaceshader")
            .unwrap()
            .displayed(),
    );
    ImportedMaterial {
        key: key.into(),
        name: name.into(),
        inside,
        standard: None,
        double_sided: true,
    }
}

fn looks(materials: Vec<ImportedMaterial>, images: Vec<ImportedImage>) -> FileItem {
    FileItem::top(
        "Looks",
        standard::materials().wrap(Materials { materials, images }),
    )
}

#[test]
fn polygons_come_back_where_they_were_in_any_units() {
    let dir = Scratch::new("units");
    let path = dir.join("panel.usdc");
    let millimetres = Units::new(0.001, UpAxis::Z).unwrap();
    let grouped = square("/Looks/Red");
    let members = Elements::from_indices(1, [0]).unwrap();
    let grouped =
        (grouped
            .attributes()
            .as_ref()
            .clone()
            .with_group(Domain::Primitive, "front", members))
        .and_then(|a| grouped.with_attributes(a))
        .unwrap();
    let mut panel = FileItem::top("Panel", standard::geometry().wrap(grouped.clone()));
    panel.placement = Xform::translation(Vec3::new(0.0, 0.0, 250.0));
    write_file(
        &path,
        &[looks(vec![plain("/Looks/Red", "Red")], vec![]), panel],
        millimetres,
    );
    // Read in the units it was written in, it is as it was.
    let read = read_file(&path, millimetres, false);
    let (placed, value) = item(&read, "Panel");
    assert_eq!(placed.placement.origin(), Vec3::new(0.0, 0.0, 250.0));
    let back = standard::geometry().get(value.unwrap()).unwrap();
    assert_eq!(back.positions().as_slice(), grouped.positions().as_slice());
    let uv = |g: &Geometry| {
        g.attributes()
            .tuples::<[f32; 2]>(Domain::Vertex, names::UV)
            .map(<[_]>::to_vec)
    };
    assert_eq!(uv(back), uv(&grouped));
    for (domain, name) in [(Domain::Vertex, names::N), (Domain::Point, names::CD)] {
        let a = back.attributes().tuples::<[f32; 3]>(domain, name);
        assert_eq!(
            a,
            grouped.attributes().tuples::<[f32; 3]>(domain, name),
            "{name}"
        );
    }
    let key = &materials(&read).materials[0].key;
    let bound = back
        .attributes()
        .text(Domain::Primitive, names::MATERIAL)
        .unwrap();
    assert_eq!(bound.get(0), Some(key.as_str()));
    assert!(
        back.attributes()
            .group(Domain::Primitive, "front")
            .is_some()
    );
    // Another project finds it where it is: a quarter of a metre up Y.
    let read = read_file(&path, metres(), false);
    let (placed, value) = item(&read, "Panel");
    assert_eq!(placed.placement.origin(), Vec3::new(0.0, 0.25, 0.0));
    let corner = standard::geometry()
        .get(value.unwrap())
        .unwrap()
        .positions()[2];
    assert!((corner[0] - 0.001).abs() < 1e-12 && (corner[2] + 0.001).abs() < 1e-12);
}

#[test]
fn curves_come_back_of_their_basis() {
    let dir = Scratch::new("curves");
    let path = dir.join("curves.usda");
    let positions: Vec<[f64; 3]> = (0..8).map(|i| [i as f64, (i % 2) as f64, 0.0]).collect();
    let kinds = vec![
        Kind::curve(Basis::Bezier, false),
        Kind::curve(Basis::Nurbs, false),
    ];
    let knots = Knots::new(
        vec![0, 0, 8].into(),
        vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0].into(),
    );
    let data = CurveData::new(vec![3, 3].into(), knots, None);
    let table = Primitives::new(
        8,
        vec![0, 4, 8].into(),
        (0..8).collect::<Vec<u32>>().into(),
        kinds.into(),
        Some(data),
    )
    .unwrap();
    let curves = Geometry::new(positions.into(), table, Attributes::new()).unwrap();
    write_file(
        &path,
        &[FileItem::top("Wires", standard::geometry().wrap(curves))],
        metres(),
    );
    let read = read_file(&path, metres(), false);
    let (wires, _) = item(&read, "Wires");
    let shown: Vec<Kind> = (read.items.iter())
        .filter(|i| {
            i.parent.is_some_and(|p| read.items[p].name == wires.name) || i.name == wires.name
        })
        .filter_map(|i| standard::geometry().get(i.data.as_ref()?))
        .flat_map(|g| {
            (0..g.primitives().len())
                .map(|p| g.primitives().kind(p))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        shown,
        [
            Kind::curve(Basis::Bezier, false),
            Kind::curve(Basis::Nurbs, false)
        ]
    );
}

#[test]
fn instances_are_written_as_copies_each_with_its_material() {
    let dir = Scratch::new("copies");
    let path = dir.join("row.usda");
    let leaf = standard::geometry().wrap(square(""));
    let xforms: Vec<Xform> = (0..3)
        .map(|i| Xform::translation(Vec3::new(i as f64, 0.0, 0.0)))
        .collect();
    let bound = Text::new(
        vec![0, 1, 0].into(),
        [Arc::from("/Looks/Red"), Arc::from("/Looks/Blue")].into(),
    )
    .unwrap();
    let attributes = Attributes::new()
        .with(Domain::Instance, names::MATERIAL, Attribute::text(bound))
        .unwrap();
    let row = Instances::new(vec![leaf], Column::new(xforms), None, attributes).unwrap();
    let items = [
        looks(
            vec![plain("/Looks/Red", "Red"), plain("/Looks/Blue", "Blue")],
            vec![],
        ),
        FileItem::top("Row", standard::instances().wrap(row)),
    ];
    write_file(&path, &items, metres());
    let read = read_file(&path, metres(), false);
    let keys: Vec<&str> = materials(&read)
        .materials
        .iter()
        .map(|m| m.key.as_str())
        .collect();
    let copies: Vec<&FileItem> = read
        .items
        .iter()
        .filter(|i| i.name.starts_with("instance"))
        .collect();
    assert_eq!(copies.len(), 3);
    for (k, copy) in copies.iter().enumerate() {
        assert_eq!(copy.placement.origin(), Vec3::new(k as f64, 0.0, 0.0));
    }
    // Each copy's polygons show the material its instance bound.
    let shown: Vec<String> = (read.items.iter())
        .filter(|i| {
            i.parent
                .is_some_and(|p| read.items[p].name.starts_with("instance"))
        })
        .filter_map(|i| standard::geometry().get(i.data.as_ref()?))
        .map(|g| {
            g.attributes()
                .text(Domain::Primitive, names::MATERIAL)
                .unwrap()
                .get(0)
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(shown, [keys[0], keys[1], keys[0]]);
}

#[test]
fn lights_cameras_and_a_domes_image_come_back() {
    let dir = Scratch::new("lights");
    let path = dir.join("stage.usdz");
    let sky = ImportedImage {
        key: "/Looks/Sky".into(),
        name: "Sky".into(),
        source: ImageSource::new(
            ImageFile::Embedded(EmbeddedFile::new("sky.exr", vec![7; 64])),
            Default::default(),
        ),
    };
    let dome = Light {
        shape: Shape::Dome {
            image: Some(format!("{OP}/Looks/Sky").into()),
            ground: None,
        },
        intensity: 2.0,
        ..Light::default()
    };
    let spot = Light {
        shape: Shape::Spot {
            radius: 0.25,
            cone_angle: 30.0,
            cone_softness: 0.5,
        },
        color: [1.0, 0.5, 0.25],
        ..Light::default()
    };
    let lens = Camera {
        projection: Projection::Perspective,
        focal_length: 35.0,
        focus_distance: 4.0,
        f_stop: 2.8,
        ..Camera::default()
    };
    let items = [
        looks(vec![], vec![sky]),
        FileItem::top("Sky", standard::light().wrap(dome.clone())),
        FileItem::top("Key", standard::light().wrap(spot.clone())),
        FileItem::top("Shot", standard::camera().wrap(lens.clone())),
    ];
    write_file(&path, &items, metres());
    let read = read_file(&path, metres(), false);
    assert_eq!(
        standard::light().get(item(&read, "Key").1.unwrap()),
        Some(&spot)
    );
    assert_eq!(
        standard::camera().get(item(&read, "Shot").1.unwrap()),
        Some(&lens)
    );
    let back = standard::light()
        .get(item(&read, "Sky").1.unwrap())
        .unwrap();
    let image = &materials(&read).images[0];
    let Shape::Dome {
        image: Some(named), ..
    } = &back.shape
    else {
        panic!("a dome");
    };
    assert_eq!(**named, format!("{OP}{}", image.key));
    let ImageFile::Embedded(file) = image.source.file() else {
        panic!("packed in the usdz");
    };
    assert_eq!((file.name(), file.bytes()), ("sky.exr", &[7u8; 64][..]));
    assert_eq!(back.intensity, 2.0);
}
