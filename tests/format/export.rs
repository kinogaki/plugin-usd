//! Items written and read back through the format: polygons with their
//! attributes, bindings and groups where they were, in any project's
//! units; curves; lights and cameras; a dome naming an image of the
//! file's. Instances are `instances.rs`'s.
use crate::support::{
    Doubling, Scratch, item, materials, metres, read_file, read_in, square, write_file, write_in,
};
use kinogaki_render_api::standard::{Light, LightShape};
use kinogaki_render_api::{Camera, Projection};
use kinogaki_studio_api::FileItem;
use kinogaki_studio_api::geometry::EmbeddedFile;
use kinogaki_studio_api::geometry::image::{ImageFile, ImageSource};
use kinogaki_studio_api::geometry::{
    Attributes, Basis, CurveData, Domain, Elements, Geometry, Kind, Knots, Primitives, Units,
    UpAxis, Vec3, Xform, names,
};
use kinogaki_studio_api::network::{NetworkSeed, NewNode};
use kinogaki_studio_api::standard::{self, ImportedMaterial, Materials};
use kinogaki_studio_api::value::ImageRef;
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

fn looks(materials: Vec<ImportedMaterial>) -> FileItem {
    FileItem::top("Looks", standard::materials().wrap(Materials { materials }))
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
        &[looks(vec![plain("/Looks/Red", "Red")]), panel],
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
fn lights_cameras_and_a_domes_image_come_back() {
    let dir = Scratch::new("lights");
    let path = dir.join("stage.usdz");
    let held = ImageSource::new(
        ImageFile::Embedded(EmbeddedFile::new("sky.exr", vec![7; 64])),
        Default::default(),
    );
    let dome = Light {
        shape: LightShape::Dome {
            image: Some(ImageRef::File(held)),
            format: kinogaki_render_api::standard::DomeFormat::Automatic,
            pole: kinogaki_render_api::standard::Pole::Scene,
            ground: None,
        },
        intensity: 2.0,
        ..Light::default()
    };
    let spot = Light {
        shape: LightShape::Sphere {
            radius: 0.25,
            point: false,
        },
        shaping: Some(kinogaki_render_api::standard::Shaping {
            cone_angle: 30.0,
            cone_softness: 0.5,
            ..Default::default()
        }),
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
        FileItem::top("Sky", standard::light().wrap(dome.clone()).unwrap()),
        FileItem::top("Key", standard::light().wrap(spot.clone()).unwrap()),
        FileItem::top("Shot", standard::camera().wrap(lens.clone()).unwrap()),
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
    let LightShape::Dome {
        image: Some(ImageRef::File(source)),
        ..
    } = &back.shape
    else {
        panic!("a dome reading its file: {:?}", back.shape);
    };
    let ImageFile::Embedded(file) = source.file() else {
        panic!("packed in the usdz");
    };
    assert_eq!((file.name(), file.bytes()), ("sky.exr", &[7u8; 64][..]));
    assert_eq!(back.intensity, 2.0);
}

/// A dome's image of each kind through a USD layer: a file on disk names
/// it, a held one is written into the layer's textures folder and named, and an Image
/// node (Export's to resolve to its file) is left out, said.
#[test]
fn a_domes_file_held_file_and_node_go_to_a_layer() {
    let dir = Scratch::new("domes");
    let path = dir.join("domes.usda");
    std::fs::write(dir.join("noon.exr"), b"an exr").unwrap();
    let dome = |image: ImageRef| {
        let light = Light {
            shape: LightShape::Dome {
                image: Some(image),
                format: kinogaki_render_api::standard::DomeFormat::Automatic,
                pole: kinogaki_render_api::standard::Pole::Scene,
                ground: None,
            },
            ..Light::default()
        };
        standard::light().wrap(light).unwrap()
    };
    let on_disk = dir.join("noon.exr").display().to_string();
    let held = ImageFile::Embedded(EmbeddedFile::new("dusk.exr", vec![9; 16]));
    let items = [
        FileItem::top("Noon", dome(ImageRef::File(ImageSource::path(&on_disk)))),
        FileItem::top(
            "Dusk",
            dome(ImageRef::File(ImageSource::new(held, Default::default()))),
        ),
        FileItem::top("Node", dome(ImageRef::Node(Arc::from("/Library/Sky")))),
    ];
    let said = write_file(&path, &items, metres()).join("\n");
    assert!(said.contains("Image node /Library/Sky"), "{said}");
    assert_eq!(
        std::fs::read(dir.join("domes_textures/dusk.exr")).unwrap(),
        [9; 16]
    );
    let read = read_file(&path, metres(), false);
    let file = |name: &str| {
        let light = standard::light().get(item(&read, name).1.unwrap()).unwrap();
        match &light.shape {
            LightShape::Dome {
                image: Some(ImageRef::File(source)),
                ..
            } => source.file().clone(),
            other => panic!("{name}: {other:?}"),
        }
    };
    let named = |name: &str| match file(name) {
        ImageFile::Path(path) => std::path::PathBuf::from(&*path),
        ImageFile::Embedded(f) => panic!("{name}: held {}", f.name()),
    };
    assert_eq!(
        named("Noon").canonicalize().unwrap(),
        dir.join("noon.exr").canonicalize().unwrap()
    );
    assert_eq!(named("Dusk").file_name().unwrap(), "dusk.exr");
    let node = standard::light()
        .get(item(&read, "Node").1.unwrap())
        .unwrap();
    assert!(matches!(node.shape, LightShape::Dome { image: None, .. }));
}

#[test]
fn colors_are_written_in_linear_rec709_as_other_applications_read_them() {
    let dir = Scratch::new("display-color");
    let path = dir.join("square.usda");
    let red = plain("/Looks/Red", "Red");
    let square = FileItem::top("Square", standard::geometry().wrap(square("/Looks/Red")));
    // Where the working space holds colors at twice their numbers.
    write_in(&path, &[looks(vec![red]), square], &Doubling);
    let text = std::fs::read_to_string(&path).unwrap();
    let written = "color3f[] primvars:displayColor = [(0.25, 0.125, 0.375),";
    assert!(text.contains(written), "{text}");
    assert!(!text.contains("colorSpace"), "{text}");
    let read = read_in(&path, &Doubling);
    let back = standard::geometry().get(item(&read, "Square").1.unwrap());
    let cd = back
        .unwrap()
        .attributes()
        .tuples::<[f32; 3]>(Domain::Point, names::CD);
    assert_eq!(cd.unwrap()[0], [0.5, 0.25, 0.75]);
}
