//! A stage's geometry as the standard Geometry, in the project's space:
//! meshes (implicit shapes as the meshes they tessellate to) with their
//! primvars as attributes (`st` as `uv`, `displayColor` as `Cd`, `normals`
//! as `N`, on the domain their interpolation names), each face's material
//! (a GeomSubset's binding over its mesh's) and GeomSubsets as groups;
//! curves; points.
use crate::names;
use crate::usd::scene::{
    CurveBasis, CurveForm, CurveWrap, Curves, Interpolation, Mesh, Points, Primvar, SubsetElement,
};
use crate::usd::sdf::{Array, Value as Usd};
use kinogaki_studio_api::Error;
use kinogaki_studio_api::color::{ColorService, LIN_REC709};
use kinogaki_studio_api::format::Conversion;
use kinogaki_studio_api::geometry::{
    Attribute, AttributeType, Attributes, Basis, Column, CurveData, Domain, Elements, Geometry,
    Kind, Knots, Meaning, Primitives, Storage, Text, Values, check_name, names as standard,
};
use std::borrow::Cow;
use std::sync::Arc;

/// What reading geometry needs: the project's space and colors, and each
/// material's key by its index in the scene.
pub(super) struct Shaping<'a> {
    pub space: &'a Conversion,
    pub colors: &'a dyn ColorService,
    pub keys: &'a [Arc<str>],
}

/// `mesh`, bound to material `material` where no GeomSubset binds its own.
pub(super) fn mesh(
    mesh: &Mesh,
    material: Option<usize>,
    cx: &Shaping<'_>,
    warnings: &mut Vec<String>,
) -> Result<Geometry, Error> {
    let counts = &mesh.face_vertex_counts;
    let indices = &mesh.face_vertex_indices;
    let holes: std::collections::HashSet<i32> =
        mesh.subdivision.hole_indices.iter().copied().collect();
    // Faces kept: at least three corners, no point twice, no hole.
    let mut starts = Vec::with_capacity(counts.len() + 1);
    let mut at = 0usize;
    for &n in counts.iter() {
        starts.push(at);
        at += n as usize;
    }
    starts.push(at);
    let kept: Vec<u32> = (0..counts.len() as u32)
        .filter(|&f| {
            let corners = &indices[starts[f as usize]..starts[f as usize + 1]];
            corners.len() >= 3 && !repeats(corners) && !holes.contains(&(f as i32))
        })
        .collect();
    let dropped = counts.len() - kept.len();
    if dropped > 0 {
        warnings.push(format!(
            "{dropped} faces with fewer than three corners, a point twice, or holes, left out"
        ));
    }
    // Each kept corner's corner in the file: reversed where the mesh winds
    // clockwise (a face keeps its first corner).
    let whole = dropped == 0 && !mesh.left_handed;
    let mut offsets = Vec::with_capacity(kept.len() + 1);
    offsets.push(0u32);
    let mut corner_from: Vec<u32> = Vec::new();
    if !whole {
        corner_from.reserve(indices.len());
        for &f in &kept {
            let (start, end) = (starts[f as usize], starts[f as usize + 1]);
            let n = end - start;
            for j in 0..n {
                let from = match (mesh.left_handed, j) {
                    (true, 0) | (false, _) => j,
                    (true, j) => n - j,
                };
                corner_from.push((start + from) as u32);
            }
            offsets.push(corner_from.len() as u32);
        }
    } else {
        for &n in counts.iter() {
            offsets.push(offsets.last().copied().unwrap_or(0) + n as u32);
        }
    }
    let vertices: Vec<u32> = match whole {
        true => indices.iter().map(|&i| i as u32).collect(),
        false => corner_from
            .iter()
            .map(|&c| indices[c as usize] as u32)
            .collect(),
    };
    let positions: Vec<[f64; 3]> = (mesh.points.iter())
        .map(|p| cx.space.point(p.map(f64::from).into()).to_array())
        .collect();
    let table = Primitives::of_polygons(positions.len(), offsets.into(), vertices.into())?;
    let mut attributes = Attributes::new();
    let spread = Spread {
        primitives: (!whole).then_some(&kept[..]),
        corners: (!whole).then_some(&corner_from[..]),
        points: positions.len(),
        faces: kept.len(),
        vertices: indices.len(),
    };
    for primvar in &mesh.primvars {
        let domain = match primvar.interpolation {
            Interpolation::Constant => Domain::Detail,
            Interpolation::Uniform => Domain::Primitive,
            Interpolation::Vertex | Interpolation::Varying => Domain::Point,
            Interpolation::FaceVarying => Domain::Vertex,
        };
        attributes = attribute(attributes, primvar, domain, &spread, cx, warnings)?;
    }
    // Each face's material: its subset's binding, else the mesh's.
    let mut face_material: Vec<Option<usize>> = vec![material; counts.len()];
    let mut bound = material.is_some();
    for subset in &mesh.subsets {
        match subset.element {
            SubsetElement::Face => {
                if let Some(m) = subset.material {
                    bound = true;
                    for &f in subset.indices.iter() {
                        face_material[f as usize] = Some(m);
                    }
                }
                let at = |f: u32| kept.binary_search(&f).ok().map(|k| k as u32);
                let members = subset.indices.iter().filter_map(|&f| at(f as u32));
                attributes = group(
                    attributes,
                    Domain::Primitive,
                    &subset.name,
                    kept.len(),
                    members,
                )?;
            }
            SubsetElement::Point => {
                let members = subset.indices.iter().map(|&p| p as u32);
                attributes = group(
                    attributes,
                    Domain::Point,
                    &subset.name,
                    positions.len(),
                    members,
                )?;
            }
            SubsetElement::Edge => {}
        }
    }
    if bound {
        // The materials' keys once each, a face naming its own.
        let mut table: Vec<Arc<str>> = Vec::new();
        let mut at: Vec<Option<u32>> = vec![None; cx.keys.len() + 1];
        let mut index = |m: Option<usize>| {
            let slot = m.unwrap_or(cx.keys.len());
            *at[slot].get_or_insert_with(|| {
                table.push(m.map_or_else(|| Arc::from(""), |m| cx.keys[m].clone()));
                (table.len() - 1) as u32
            })
        };
        let faces: Vec<u32> = kept
            .iter()
            .map(|&f| index(face_material[f as usize]))
            .collect();
        let text = Text::new(faces.into(), table.into())?;
        attributes =
            attributes.with(Domain::Primitive, standard::MATERIAL, Attribute::text(text))?;
    }
    Geometry::new(positions.into(), table, attributes)
}

/// Whether a point is among `corners` twice.
fn repeats(corners: &[i32]) -> bool {
    match corners.len() {
        0..=8 => (1..corners.len()).any(|i| corners[..i].contains(&corners[i])),
        _ => {
            let mut sorted = corners.to_vec();
            sorted.sort_unstable();
            sorted.windows(2).any(|w| w[0] == w[1])
        }
    }
}

/// Where a primvar's elements land: the faces kept (by their index in the
/// file) and each kept corner's corner in the file, `None` when every one
/// is kept in order.
struct Spread<'a> {
    primitives: Option<&'a [u32]>,
    corners: Option<&'a [u32]>,
    points: usize,
    faces: usize,
    vertices: usize,
}

impl Spread<'_> {
    /// Each element of `domain` by its element in the file.
    fn from(&self, domain: Domain) -> Box<dyn Iterator<Item = usize> + '_> {
        match (domain, self.primitives, self.corners) {
            (Domain::Primitive, Some(kept), _) => Box::new(kept.iter().map(|&f| f as usize)),
            (Domain::Vertex, _, Some(corners)) => Box::new(corners.iter().map(|&c| c as usize)),
            (Domain::Primitive, None, _) => Box::new(0..self.faces),
            (Domain::Vertex, _, None) => Box::new(0..self.vertices),
            (Domain::Point, ..) => Box::new(0..self.points),
            _ => Box::new(0..1),
        }
    }

    /// The items (of `width`) of `values` each element of `domain` holds,
    /// through `indices` when indexed: copied whole where every element is
    /// the file's own.
    fn gather<T: Clone>(
        &self,
        domain: Domain,
        values: &[T],
        width: usize,
        indices: Option<&Vec<i32>>,
    ) -> Vec<T> {
        let whole = match domain {
            Domain::Primitive => self.primitives.is_none(),
            Domain::Vertex => self.corners.is_none(),
            _ => true,
        };
        if whole && indices.is_none() {
            return values.to_vec();
        }
        let mut out = Vec::with_capacity(values.len());
        for at in self.from(domain) {
            let item = indices.map_or(at, |i| i[at] as usize);
            out.extend_from_slice(&values[item * width..(item + 1) * width]);
        }
        out
    }
}

/// `attributes` with `primvar` on `domain`, its elements taken as `spread`
/// says; a primvar no attribute holds is left out, said.
fn attribute(
    attributes: Attributes,
    primvar: &Primvar,
    domain: Domain,
    spread: &Spread<'_>,
    cx: &Shaping<'_>,
    warnings: &mut Vec<String>,
) -> Result<Attributes, Error> {
    let name = names::attribute(&primvar.name);
    let Some((flat, width)) = flat(&primvar.values) else {
        warnings.push(format!(
            "primvar {} holds no numbers; left out",
            primvar.name
        ));
        return Ok(attributes);
    };
    let width = width * primvar.element_size;
    let standard_type = standard::standard_type(name);
    if let Some((_, domains)) = standard_type
        && !domains.contains(&domain)
    {
        let interpolation = primvar.interpolation.token();
        let said = format!(
            "primvar {} is {interpolation}; {name} isn't held there, left out",
            primvar.name
        );
        warnings.push(said);
        return Ok(attributes);
    }
    let meaning = match primvar.type_name.as_str().trim_end_matches("[]") {
        "texCoord2f" | "texCoord2d" | "texCoord2h" | "texCoord3f" => Meaning::TextureCoord,
        t if t.starts_with("color") => Meaning::Color,
        t if t.starts_with("normal") => Meaning::Normal,
        t if t.starts_with("point") => Meaning::Point,
        t if t.starts_with("vector") => Meaning::Vector,
        _ => Meaning::None,
    };
    let (ty, values) = match flat {
        Flat::Reals(flat) => {
            let mut out = spread.gather(domain, &flat, width, primvar.indices.as_deref());
            moved(&mut out, meaning, width, primvar, cx, warnings);
            let ty = match standard_type {
                Some((ty, _)) if ty.width as usize == width => ty,
                Some(_) => {
                    warnings.push(format!("primvar {} is no {name}; left out", primvar.name));
                    return Ok(attributes);
                }
                None => match u8::try_from(width).ok().filter(|w| (1..=16).contains(w)) {
                    Some(w) => AttributeType::new(Storage::F32, w, meaning),
                    None => {
                        warnings.push(format!("primvar {} is too wide; left out", primvar.name));
                        return Ok(attributes);
                    }
                },
            };
            (ty, Values::F32(Column::new(out)))
        }
        Flat::Integers(flat) => {
            let out = spread.gather(domain, flat, width, primvar.indices.as_deref());
            let ty = AttributeType::new(Storage::I32, width.min(255) as u8, Meaning::None);
            (ty, Values::I32(Column::new(out)))
        }
        Flat::Texts(flat) => {
            let out = spread.gather(domain, &flat, 1, primvar.indices.as_deref());
            (AttributeType::TEXT, Values::Text(Text::from_strs(out)))
        }
    };
    if standard_type.is_some_and(|(t, _)| t != ty)
        || check_name(name).is_err()
        || ty.check().is_err()
    {
        warnings.push(format!(
            "primvar {} can't be an attribute; left out",
            primvar.name
        ));
        return Ok(attributes);
    }
    attributes.with(domain, name, Attribute::new(ty, values)?)
}

/// A primvar's values, flattened, and how many an item: reals as single
/// floats (the layer's own when they are), whole numbers and text as they
/// are.
enum Flat<'a> {
    Reals(Cow<'a, [f32]>),
    Integers(&'a [i32]),
    Texts(Vec<&'a str>),
}

fn flat(values: &Usd) -> Option<(Flat<'_>, usize)> {
    fn borrowed(v: &[f32], n: usize) -> Option<(Flat<'_>, usize)> {
        Some((Flat::Reals(Cow::Borrowed(v)), n))
    }
    match values {
        Usd::Array(Array::Float(v)) => borrowed(v, 1),
        Usd::Array(Array::Vec2f(v)) => borrowed(v.as_flattened(), 2),
        Usd::Array(Array::Vec3f(v)) => borrowed(v.as_flattened(), 3),
        Usd::Array(Array::Vec4f(v)) => borrowed(v.as_flattened(), 4),
        Usd::Array(Array::Int(v)) => Some((Flat::Integers(v), 1)),
        Usd::Array(Array::String(v) | Array::Token(v)) => {
            Some((Flat::Texts(v.iter().map(|t| t.as_str()).collect()), 1))
        }
        Usd::String(t) | Usd::Token(t) => Some((Flat::Texts(vec![t.as_str()]), 1)),
        values => {
            let (reals, width) = crate::usd::scene::values::reals(values)?;
            let reals = reals.iter().map(|&v| v as f32).collect();
            Some((Flat::Reals(Cow::Owned(reals)), width))
        }
    }
}

/// `out`'s items (of `width`) in the project's space: points moved, vectors
/// turned and scaled, normals turned, colors in the working space.
fn moved(
    out: &mut [f32],
    meaning: Meaning,
    width: usize,
    primvar: &Primvar,
    cx: &Shaping<'_>,
    warnings: &mut Vec<String>,
) {
    let each = |out: &mut [f32], change: &dyn Fn([f64; 3]) -> [f64; 3]| {
        for item in out.chunks_exact_mut(width) {
            let v = change([item[0], item[1], item[2]].map(f64::from));
            item[..3].copy_from_slice(&v.map(|c| c as f32));
        }
    };
    let space = cx.space;
    match (meaning, width) {
        // A vector is of lengths: turned and scaled, as a point is.
        (Meaning::Point | Meaning::Vector, 3) => each(out, &|p| space.point(p.into()).to_array()),
        (Meaning::Normal, 3) => each(out, &|n| space.direction(n.into()).to_array()),
        (Meaning::Color, 3 | 4) => {
            let from = primvar.color_space.as_deref().unwrap_or(LIN_REC709);
            let mut rgb: Vec<[f64; 3]> = (out.chunks_exact(width))
                .map(|c| [c[0], c[1], c[2]].map(f64::from))
                .collect();
            match cx.colors.to_working(from, &mut rgb) {
                Ok(()) => (out.chunks_exact_mut(width).zip(&rgb))
                    .for_each(|(c, w)| c[..3].copy_from_slice(&w.map(|v| v as f32))),
                Err(e) => warnings.push(format!(
                    "primvar {}: colors kept as they are: {e}",
                    primvar.name
                )),
            }
        }
        _ => {}
    }
}

/// `attributes` with group `name` of `members` on `domain` (of `count`
/// elements), when `name` may name a group.
fn group(
    attributes: Attributes,
    domain: Domain,
    name: &str,
    count: usize,
    members: impl IntoIterator<Item = u32>,
) -> Result<Attributes, Error> {
    if check_name(name).is_err() || standard::standard_type(name).is_some() {
        return Ok(attributes);
    }
    let members = Elements::from_indices(count, members)?;
    attributes.with_group(domain, name, members)
}

/// `curves` as curve primitives: linear curves polylines (a periodic one
/// closing on its first point), cubic ones Bézier, B-spline or Catmull-Rom
/// curves, NURBS as they are; `widths` as `width`.
pub(super) fn curves(
    curves: &Curves,
    cx: &Shaping<'_>,
    warnings: &mut Vec<String>,
) -> Result<Geometry, Error> {
    let positions: Vec<[f64; 3]> = (curves.points.iter())
        .map(|p| cx.space.point(p.map(f64::from).into()).to_array())
        .collect();
    let mut offsets = vec![0u32];
    let mut vertices: Vec<u32> = Vec::with_capacity(positions.len());
    let mut kinds = Vec::with_capacity(curves.vertex_counts.len());
    let mut degrees = Vec::with_capacity(curves.vertex_counts.len());
    let mut knot_offsets = vec![0u32];
    let mut knots: Vec<f64> = Vec::new();
    let mut weights: Option<Vec<f64>> = None;
    let mut start = 0u32;
    for (c, &count) in curves.vertex_counts.iter().enumerate() {
        let count = count.max(0) as u32;
        vertices.extend(start..start + count);
        let (kind, degree) = match &curves.form {
            CurveForm::Linear { periodic } => {
                if *periodic && count > 2 {
                    vertices.push(start);
                }
                (Kind::POLYLINE, 1)
            }
            CurveForm::Cubic { basis, wrap } => {
                let basis = match basis {
                    CurveBasis::Bezier => Basis::Bezier,
                    CurveBasis::BSpline => Basis::BSpline,
                    CurveBasis::CatmullRom => Basis::CatmullRom,
                };
                (Kind::curve(basis, *wrap == CurveWrap::Periodic), 3)
            }
            CurveForm::Nurbs {
                orders,
                knots: all,
                weights: w,
                ..
            } => {
                let order = orders.get(c).copied().unwrap_or(4).max(2) as usize;
                let from: usize = (curves.vertex_counts[..c].iter())
                    .zip(orders.iter())
                    .map(|(&n, &o)| n.max(0) as usize + o.max(2) as usize)
                    .sum();
                knots.extend(all.iter().skip(from).take(count as usize + order));
                if !w.is_empty() {
                    weights = Some(w.clone());
                }
                (Kind::curve(Basis::Nurbs, false), (order - 1) as u8)
            }
        };
        knot_offsets.push(knots.len() as u32);
        offsets.push(vertices.len() as u32);
        kinds.push(kind);
        degrees.push(degree);
        start += count;
    }
    if matches!(
        curves.form,
        CurveForm::Cubic {
            basis: CurveBasis::BSpline | CurveBasis::CatmullRom,
            wrap: CurveWrap::Nonperiodic | CurveWrap::Pinned
        }
    ) {
        warnings.push("open B-spline and Catmull-Rom curves end on their end points".into());
    }
    let data = CurveData::new(
        degrees.into(),
        Knots::new(knot_offsets.into(), knots.into()),
        weights.map(Column::new),
    );
    let table = Primitives::new(
        positions.len(),
        offsets.into(),
        vertices.into(),
        kinds.into(),
        Some(data),
    )?;
    let mut attributes = Attributes::new();
    if let Some(widths) = curves.primvars.iter().find(|p| p.name.as_str() == "widths") {
        attributes = width(
            attributes,
            widths,
            curves.vertex_counts.len(),
            positions.len(),
            cx,
        )?;
    }
    Geometry::new(positions.into(), table, attributes)
}

/// `points` as points alone: `widths` as `width`, `ids` as `id`.
pub(super) fn points(points: &Points, cx: &Shaping<'_>) -> Result<Geometry, Error> {
    let positions: Vec<[f64; 3]> = (points.points.iter())
        .map(|p| cx.space.point(p.map(f64::from).into()).to_array())
        .collect();
    let table = Primitives::new(
        positions.len(),
        vec![0].into(),
        Vec::new().into(),
        Vec::new().into(),
        None,
    )?;
    let mut attributes = Attributes::new();
    if let Some(widths) = points.primvars.iter().find(|p| p.name.as_str() == "widths") {
        attributes = width(attributes, widths, 0, positions.len(), cx)?;
    }
    if let Some(ids) = &points.ids
        && ids.len() == positions.len()
    {
        attributes = attributes.with(
            Domain::Point,
            standard::ID,
            Attribute::tuples(ids.to_vec(), Meaning::None)?,
        )?;
    }
    Geometry::new(positions.into(), table, attributes)
}

/// `attributes` with `widths` as `width` in the project's units: per
/// point, per primitive (of `primitives`), or one for all points.
fn width(
    attributes: Attributes,
    widths: &Primvar,
    primitives: usize,
    points: usize,
    cx: &Shaping<'_>,
) -> Result<Attributes, Error> {
    let Some((flat, 1)) = crate::usd::scene::values::reals(&widths.values) else {
        return Ok(attributes);
    };
    let read = |at: usize| widths.indices.as_ref().map_or(at, |i| i[at] as usize);
    let (domain, count) = match (widths.interpolation, primitives) {
        (Interpolation::Uniform | Interpolation::Constant, n) if n > 0 => (Domain::Primitive, n),
        (Interpolation::Vertex | Interpolation::Constant, _) => (Domain::Point, points),
        // Widths at segment ends: no element of the table holds them.
        _ => return Ok(attributes),
    };
    let constant = widths.interpolation == Interpolation::Constant;
    let values: Option<Vec<f32>> = (0..count)
        .map(|at| flat.get(if constant { 0 } else { read(at) }).copied())
        .map(|w| w.map(|w| cx.space.length(w) as f32))
        .collect();
    let Some(values) = values else {
        return Ok(attributes);
    };
    attributes.with(
        domain,
        standard::WIDTH,
        Attribute::tuples(values, Meaning::None)?,
    )
}
