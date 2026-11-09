//! A stage's geometry as the standard Geometry, in the project's space:
//! meshes (implicit shapes as the meshes they tessellate to) with their
//! primvars as attributes ([`super::primvars`]), each face's material (a
//! GeomSubset's binding over its mesh's), GeomSubsets of family `part` as
//! the faces' `part` and the others as groups, and their subdivision
//! ([`crate::subdivision`]); curves; points. What is left out is said with
//! the prim's path.
use super::primvars::{self, Spread};
use crate::usd::scene::{
    CurveBasis, CurveForm, CurveWrap, Curves, Interpolation, Mesh, Points, Primvar, SubsetElement,
};
use crate::usd::sdf::Path;
use kinogaki_studio_api::Error;
use kinogaki_studio_api::color::ColorService;
use kinogaki_studio_api::format::Conversion;
use kinogaki_studio_api::geometry::{
    Attribute, Attributes, Basis, Column, CurveData, Domain, Elements, Geometry, Kind, Knots,
    Meaning, Primitives, Text, check_name, names as standard,
};
use std::sync::Arc;

/// What reading geometry needs: the project's space and colors, and each
/// material's key by its index in the scene.
pub(super) struct Shaping<'a> {
    pub space: &'a Conversion,
    pub colors: &'a dyn ColorService,
    pub keys: &'a [Arc<str>],
}

/// `mesh`, the prim at `prim`, bound to material `material` where no
/// GeomSubset binds its own.
pub(super) fn mesh(
    mesh: &Mesh,
    material: Option<usize>,
    cx: &Shaping<'_>,
    prim: &Path,
    warnings: &mut Vec<String>,
) -> Result<Geometry, Error> {
    let counts = &mesh.face_vertex_counts;
    let indices = &mesh.face_vertex_indices;
    // Faces kept: at least three corners, no point twice (a hole is kept,
    // in `subdivision_hole`).
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
            corners.len() >= 3 && !repeats(corners)
        })
        .collect();
    let dropped = counts.len() - kept.len();
    if dropped > 0 {
        warnings.push(format!(
            "{prim}: {dropped} faces with fewer than three corners or a point twice, left out"
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
        attributes = primvars::attribute(attributes, primvar, &spread, cx, prim, warnings)?;
    }
    // Each face's material: its subset's binding, else the mesh's.
    let mut face_material: Vec<Option<usize>> = vec![material; counts.len()];
    let mut bound = material.is_some();
    // Each face's part: its `part` subset's name.
    let mut face_part: Option<Vec<&str>> = None;
    for subset in &mesh.subsets {
        match subset.element {
            SubsetElement::Face if subset.family.as_str() == "part" => {
                let parts = face_part.get_or_insert_with(|| vec![""; counts.len()]);
                for &f in subset.indices.iter() {
                    if let Some(part) = parts.get_mut(f as usize) {
                        *part = subset.name.as_str();
                    }
                }
            }
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
    if let Some(parts) = face_part {
        let text = Text::from_strs(kept.iter().map(|&f| parts[f as usize]));
        attributes = attributes.with(Domain::Primitive, standard::PART, Attribute::text(text))?;
    }
    let mut said = Vec::new();
    attributes = crate::subdivision::read(&mesh.subdivision, &table, &kept, attributes, &mut said)?;
    warnings.extend(said.into_iter().map(|s| format!("{prim}: {s}")));
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

/// `curves`, the prim at `prim` bound to `material`, as curve primitives:
/// linear curves polylines (a periodic one closing on its first point),
/// cubic ones Bézier, B-spline or Catmull-Rom curves, NURBS as they are;
/// `widths` as `width`, the other primitives as a mesh's are.
pub(super) fn curves(
    curves: &Curves,
    material: Option<usize>,
    cx: &Shaping<'_>,
    prim: &Path,
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
        warnings.push(format!(
            "{prim}: open B-spline and Catmull-Rom curves end on their end points"
        ));
    }
    // Polylines alone hold no curve data.
    let curved = kinds.iter().any(|k| k.basis() != Basis::Linear);
    let data = curved.then(|| {
        let knots = Knots::new(knot_offsets.into(), knots.into());
        CurveData::new(degrees.into(), knots, weights.map(Column::new))
    });
    let table = Primitives::new(
        positions.len(),
        offsets.into(),
        vertices.into(),
        kinds.into(),
        data,
    )?;
    let count = curves.vertex_counts.len();
    let mut attributes = Attributes::new();
    let spread = Spread {
        primitives: None,
        corners: None,
        points: positions.len(),
        faces: count,
        vertices: 0,
    };
    for primvar in &curves.primvars {
        attributes = match primvar.name.as_str() {
            "widths" => width(attributes, primvar, count, positions.len(), cx)?,
            _ => primvars::attribute(attributes, primvar, &spread, cx, prim, warnings)?,
        };
    }
    if let Some(m) = material {
        let key = std::iter::repeat_n(&*cx.keys[m], count);
        let text = Attribute::text(Text::from_strs(key));
        attributes = attributes.with(Domain::Primitive, standard::MATERIAL, text)?;
    }
    Geometry::new(positions.into(), table, attributes)
}

/// `points`, the prim at `prim`, as points alone: `widths` as `width`,
/// `ids` as `id`, the other primvars as a mesh's are.
pub(super) fn points(
    points: &Points,
    cx: &Shaping<'_>,
    prim: &Path,
    warnings: &mut Vec<String>,
) -> Result<Geometry, Error> {
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
    let spread = Spread {
        primitives: None,
        corners: None,
        points: positions.len(),
        faces: 0,
        vertices: 0,
    };
    for primvar in &points.primvars {
        attributes = match primvar.name.as_str() {
            "widths" => width(attributes, primvar, 0, positions.len(), cx)?,
            _ => primvars::attribute(attributes, primvar, &spread, cx, prim, warnings)?,
        };
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
