//! The standard Geometry as USD prims: its polygons a Mesh with its
//! attributes as primvars (`uv` as `st`, `N` as `normals`, `Cd` as
//! `displayColor`, on the interpolation their domain is), its faces'
//! materials as bindings (GeomSubsets where they differ) and its primitive
//! groups as GeomSubsets; its curves BasisCurves or NurbsCurves, one prim
//! for each kind of curve; points alone, Points.
use crate::names;
use crate::usd::scene::{
    CurveBasis, CurveForm, CurveWrap, Curves, Interpolation, Mesh, NodeKind, Points, Primvar,
    Subdivision, Subset, SubsetElement,
};
use crate::usd::sdf::{Array, Token, Value as Usd};
use kinogaki_studio_api::format::Unique;
use kinogaki_studio_api::geometry::{
    Attribute, Basis, Domain, Geometry, Kind, Meaning, Values, names as standard,
};
use std::collections::HashMap;
use std::sync::Arc;

/// What a value is written as: prims, each with its own name's suffix
/// (empty for the item's own prim) and its material binding.
pub(super) struct Prims {
    pub prims: Vec<(&'static str, NodeKind, Option<usize>)>,
}

/// What writing geometry needs: each material's index in the scene by its
/// key, whether each is double-sided, and the working space's name.
pub(super) struct Shaping<'a> {
    pub materials: &'a HashMap<Arc<str>, usize>,
    pub double_sided: &'a [bool],
    pub working: &'a str,
}

/// `value` as USD prims.
pub(super) fn prims(value: &Geometry, cx: &Shaping<'_>, warnings: &mut Vec<String>) -> Prims {
    let table = value.primitives();
    let polygons: Vec<usize> = (0..table.len())
        .filter(|&p| table.kind(p).is_polygon())
        .collect();
    let mut prims = Vec::new();
    if !polygons.is_empty() {
        let (mesh, material) = mesh(value, &polygons, cx, warnings);
        prims.push(("", NodeKind::Mesh(Box::new(mesh)), material));
    }
    let mut kinds: Vec<(Kind, u8)> = Vec::new();
    for p in (0..table.len()).filter(|&p| !table.kind(p).is_polygon()) {
        let degree = table.curves().map_or(1, |c| c.degrees()[p]);
        if !kinds.contains(&(table.kind(p), degree)) {
            kinds.push((table.kind(p), degree));
        }
    }
    for (kind, degree) in kinds {
        match curves(value, kind, degree) {
            Some(curves) => prims.push(("Curves", NodeKind::Curves(Box::new(curves)), None)),
            None => warnings.push(format!(
                "{} curves of degree {degree} have no USD curves; left out",
                kind.basis().name()
            )),
        }
    }
    if table.is_empty() && !value.positions().is_empty() {
        prims.push(("", NodeKind::Points(Box::new(points(value))), None));
    }
    Prims { prims }
}

/// Positions as USD holds them.
fn positions(value: &Geometry, at: impl Iterator<Item = usize>) -> Arc<Vec<[f32; 3]>> {
    let p = value.positions();
    Arc::new(at.map(|i| p[i].map(|c| c as f32)).collect())
}

/// The polygons `polygons` of `value` as a Mesh over all its points, and
/// the material every face shows when they all show one.
fn mesh(
    value: &Geometry,
    polygons: &[usize],
    cx: &Shaping<'_>,
    warnings: &mut Vec<String>,
) -> (Mesh, Option<usize>) {
    let table = value.primitives();
    let every = polygons.len() == table.len();
    let mut counts = Vec::with_capacity(polygons.len());
    let mut indices = Vec::with_capacity(table.vertex_count());
    let mut corners: Vec<usize> = Vec::new();
    for &p in polygons {
        let points = table.primitive(p);
        counts.push(points.len() as i32);
        indices.extend(points.iter().map(|&i| i as i32));
        if !every {
            corners.extend(table.primitive_vertices(p));
        }
    }
    let mut primvars = Vec::new();
    for (domain, name, attribute) in value.attributes().iter() {
        let (interpolation, picked) = match domain {
            Domain::Point => (Interpolation::Vertex, None),
            Domain::Vertex => (Interpolation::FaceVarying, (!every).then_some(&corners[..])),
            Domain::Primitive => (Interpolation::Uniform, (!every).then_some(polygons)),
            Domain::Detail => (Interpolation::Constant, None),
            _ => continue,
        };
        if matches!(name, standard::MATERIAL | standard::NAME | standard::PART) {
            continue;
        }
        match primvar(name, attribute, interpolation, picked, cx.working) {
            Some(primvar) => primvars.push(primvar),
            None => warnings.push(format!("attribute {name} has no USD primvar; left out")),
        }
    }
    let mut subsets = Vec::new();
    let mut names = Unique::default();
    let material = materials(value, polygons, cx, &mut subsets, &mut names, warnings);
    let at: HashMap<usize, i32> = (polygons.iter().enumerate())
        .map(|(k, &p)| (p, k as i32))
        .collect();
    for (domain, name, members) in value.attributes().groups() {
        let (element, indices): (_, Vec<i32>) = match domain {
            Domain::Primitive => (
                SubsetElement::Face,
                members
                    .iter()
                    .filter_map(|p| at.get(&(p as usize)).copied())
                    .collect(),
            ),
            Domain::Point => (
                SubsetElement::Point,
                members.iter().map(|p| p as i32).collect(),
            ),
            _ => continue,
        };
        if name.starts_with('.') || indices.is_empty() {
            continue;
        }
        subsets.push(Subset {
            name: Token::new(&names.take(&names::identifier(name))),
            element,
            indices: Arc::new(indices),
            family: Token::empty(),
            material: None,
        });
    }
    // A mesh's `doubleSided` is its materials' Double Sided (and on where
    // it shows none, as Studio's default material is).
    let shown = material
        .into_iter()
        .chain(subsets.iter().filter_map(|s| s.material));
    let double_sided = shown.into_iter().all(|m| cx.double_sided[m]);
    let mesh = Mesh {
        points: positions(value, 0..value.positions().len()),
        face_vertex_counts: Arc::new(counts),
        face_vertex_indices: Arc::new(indices),
        left_handed: false,
        primvars,
        subdivision: Subdivision::default(),
        subsets,
        double_sided,
    };
    (mesh, material)
}

/// The faces' materials: the one all show, else a GeomSubset binding each
/// (family `materialBind`) pushed to `subsets`.
fn materials(
    value: &Geometry,
    polygons: &[usize],
    cx: &Shaping<'_>,
    subsets: &mut Vec<Subset>,
    names: &mut Unique,
    warnings: &mut Vec<String>,
) -> Option<usize> {
    let text = value
        .attributes()
        .text(Domain::Primitive, standard::MATERIAL)?;
    // Each key of the text's table once: its material, said once when the
    // file holds none of that key.
    let table: Vec<Option<usize>> = (text.table().iter())
        .map(|key| {
            let found = cx.materials.get(&**key).copied();
            if found.is_none() && !key.is_empty() {
                warnings.push(format!(
                    "faces name {key}, which isn't written; left unbound"
                ));
            }
            found
        })
        .collect();
    let mut faces: Vec<(usize, Vec<i32>)> = Vec::new();
    let mut unbound = false;
    for (k, &p) in polygons.iter().enumerate() {
        // An index past the table is a face naming nothing.
        match table.get(text.indices()[p] as usize).copied().flatten() {
            Some(m) => match faces.iter_mut().find(|(at, _)| *at == m) {
                Some((_, members)) => members.push(k as i32),
                None => faces.push((m, vec![k as i32])),
            },
            None => unbound = true,
        }
    }
    match faces.as_slice() {
        [(m, _)] if !unbound => return Some(*m),
        _ => {}
    }
    for (m, members) in faces {
        subsets.push(Subset {
            name: Token::new(&names.take(&format!("material_{m}"))),
            element: SubsetElement::Face,
            indices: Arc::new(members),
            family: Token::new("materialBind"),
            material: Some(m),
        });
    }
    None
}

/// `attribute` (named `name`) as a primvar of `interpolation`, its values
/// at the elements `picked` (all, in order, when none); none for one USD
/// has no type for.
fn primvar(
    name: &str,
    attribute: &Attribute,
    interpolation: Interpolation,
    picked: Option<&[usize]>,
    working: &str,
) -> Option<Primvar> {
    let ty = attribute.ty();
    let width = usize::from(ty.width);
    let meaning = ty.meaning;
    let (type_name, values, element_size) = match attribute.values() {
        Values::F32(v) => reals(v.as_slice(), width, meaning, picked),
        Values::F64(v) => {
            let v: Vec<f32> = v.iter().map(|&x| x as f32).collect();
            reals(&v, width, meaning, picked)
        }
        Values::I32(v) if width == 1 => {
            ("int[]", Array::Int(Arc::new(pick(v.as_slice(), picked))), 1)
        }
        Values::I64(v) if width == 1 => (
            "int64[]",
            Array::Int64(Arc::new(pick(v.as_slice(), picked))),
            1,
        ),
        Values::Text(text) => {
            let all: Vec<Token> = text.iter().map(|t| Token::new(t.unwrap_or(""))).collect();
            ("string[]", Array::String(Arc::new(pick(&all, picked))), 1)
        }
        _ => return None,
    };
    Some(Primvar {
        name: Token::new(names::primvar(name)),
        type_name: Token::new(type_name),
        interpolation,
        element_size,
        values: Usd::Array(values),
        indices: None,
        color_space: (meaning == Meaning::Color).then(|| Token::new(working)),
    })
}

/// The items `picked` of `values` (all when none).
fn pick<T: Clone>(values: &[T], picked: Option<&[usize]>) -> Vec<T> {
    match picked {
        None => values.to_vec(),
        Some(picked) => picked.iter().map(|&e| values[e].clone()).collect(),
    }
}

/// Reals of `width` an item as USD's array of their meaning: its USD type,
/// the array, and the element size (wider items as several floats).
fn reals(
    flat: &[f32],
    width: usize,
    meaning: Meaning,
    picked: Option<&[usize]>,
) -> (&'static str, Array, usize) {
    fn items<const N: usize>(flat: &[f32], picked: Option<&[usize]>) -> Arc<Vec<[f32; N]>> {
        Arc::new(pick(flat.as_chunks::<N>().0, picked))
    }
    match (meaning, width) {
        (Meaning::TextureCoord, 2) => ("texCoord2f[]", Array::Vec2f(items(flat, picked)), 1),
        (Meaning::TextureCoord, 3) => ("texCoord3f[]", Array::Vec3f(items(flat, picked)), 1),
        (Meaning::Color, 3) => ("color3f[]", Array::Vec3f(items(flat, picked)), 1),
        (Meaning::Color, 4) => ("color4f[]", Array::Vec4f(items(flat, picked)), 1),
        (Meaning::Normal, 3) => ("normal3f[]", Array::Vec3f(items(flat, picked)), 1),
        (Meaning::Point, 3) => ("point3f[]", Array::Vec3f(items(flat, picked)), 1),
        (Meaning::Vector, 3) => ("vector3f[]", Array::Vec3f(items(flat, picked)), 1),
        (_, 1) => ("float[]", Array::Float(Arc::new(pick(flat, picked))), 1),
        (_, 2) => ("float2[]", Array::Vec2f(items(flat, picked)), 1),
        (_, 3) => ("float3[]", Array::Vec3f(items(flat, picked)), 1),
        (_, 4) => ("float4[]", Array::Vec4f(items(flat, picked)), 1),
        (_, n) => {
            let all: Vec<f32> = match picked {
                None => flat.to_vec(),
                Some(picked) => picked
                    .iter()
                    .flat_map(|&e| flat[e * n..(e + 1) * n].iter().copied())
                    .collect(),
            };
            ("float[]", Array::Float(Arc::new(all)), n)
        }
    }
}

/// `value`'s curves of `kind` and `degree` as one prim (each curve's
/// points its own); none for a kind USD has no curves of.
fn curves(value: &Geometry, kind: Kind, degree: u8) -> Option<Curves> {
    let table = value.primitives();
    let closed = kind.closed();
    let form = match (kind.basis(), degree) {
        (Basis::Linear, _) => CurveForm::Linear { periodic: false },
        (Basis::Bezier, 3) => cubic(CurveBasis::Bezier, closed, CurveWrap::Nonperiodic),
        (Basis::BSpline, 3) => cubic(CurveBasis::BSpline, closed, CurveWrap::Pinned),
        (Basis::CatmullRom, _) => cubic(CurveBasis::CatmullRom, closed, CurveWrap::Pinned),
        (Basis::Nurbs, _) => CurveForm::Nurbs {
            orders: Vec::new(),
            knots: Vec::new(),
            ranges: Vec::new(),
            weights: Vec::new(),
        },
        _ => return None,
    };
    let of_kind: Vec<usize> = (0..table.len())
        .filter(|&p| {
            table.kind(p) == kind && table.curves().map_or(1, |c| c.degrees()[p]) == degree
        })
        .collect();
    let mut counts = Vec::with_capacity(of_kind.len());
    let mut at = Vec::new();
    let mut form = form;
    for &p in &of_kind {
        let points = table.primitive(p);
        counts.push(points.len() as i32);
        at.extend(points.iter().map(|&i| i as usize));
        if let CurveForm::Nurbs {
            orders,
            knots,
            weights,
            ..
        } = &mut form
        {
            let data = table.curves().expect("NURBS curves hold their data");
            orders.push(i32::from(degree) + 1);
            knots.extend_from_slice(data.knots().of(p));
            if let Some(w) = data.weights() {
                weights.extend(table.primitive_vertices(p).map(|v| w[v]));
            }
        }
    }
    let mut primvars = Vec::new();
    if let Some(widths) = value
        .attributes()
        .tuples::<f32>(Domain::Point, standard::WIDTH)
    {
        let values: Vec<f32> = at.iter().map(|&i| widths[i]).collect();
        primvars.push(Primvar {
            name: Token::new("widths"),
            type_name: Token::new("float[]"),
            interpolation: Interpolation::Vertex,
            element_size: 1,
            values: Usd::Array(Array::Float(Arc::new(values))),
            indices: None,
            color_space: None,
        });
    }
    Some(Curves {
        form,
        vertex_counts: Arc::new(counts),
        points: positions(value, at.into_iter()),
        primvars,
    })
}

fn cubic(basis: CurveBasis, closed: bool, open: CurveWrap) -> CurveForm {
    let wrap = if closed { CurveWrap::Periodic } else { open };
    CurveForm::Cubic { basis, wrap }
}

/// Points alone, with `width` as `widths` and `id` as `ids`.
fn points(value: &Geometry) -> Points {
    let attributes = value.attributes();
    let mut primvars = Vec::new();
    if let Some(widths) = attributes.tuples::<f32>(Domain::Point, standard::WIDTH) {
        primvars.push(Primvar {
            name: Token::new("widths"),
            type_name: Token::new("float[]"),
            interpolation: Interpolation::Vertex,
            element_size: 1,
            values: Usd::Array(Array::Float(Arc::new(widths.to_vec()))),
            indices: None,
            color_space: None,
        });
    }
    Points {
        points: positions(value, 0..value.positions().len()),
        ids: (attributes.tuples::<i64>(Domain::Point, standard::ID))
            .map(|ids| Arc::new(ids.to_vec())),
        primvars,
    }
}
