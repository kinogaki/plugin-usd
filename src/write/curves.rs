//! The standard Geometry's curves and points as USD prims: curves of one
//! kind, degree, closing and material as one BasisCurves or NurbsCurves
//! prim (each curve's points its own, a closed polyline periodic without
//! its last point), points alone as Points; their attributes as primvars,
//! as a mesh writes them (`width` as `widths`, a point's `id` as `ids`).
use super::geometry::{positions, primvar};
use crate::usd::scene::{CurveBasis, CurveForm, CurveWrap, Curves, Interpolation, Points, Primvar};
use crate::usd::sdf::Token;
use kinogaki_studio_api::color::ColorService;
use kinogaki_studio_api::geometry::{Basis, Domain, Geometry, Kind, Primitives, names};
use std::sync::Arc;

/// USD's curves of `kind` and `degree`, periodic polylines when `closed`;
/// none for curves USD has none of.
pub(super) fn form(kind: Kind, degree: u8, closed: bool) -> Option<CurveForm> {
    let shut = kind.closed();
    Some(match (kind.basis(), degree) {
        (Basis::Linear, _) => CurveForm::Linear { periodic: closed },
        (Basis::Bezier, 3) => cubic(CurveBasis::Bezier, shut, CurveWrap::Nonperiodic),
        (Basis::BSpline, 3) => cubic(CurveBasis::BSpline, shut, CurveWrap::Pinned),
        (Basis::CatmullRom, _) => cubic(CurveBasis::CatmullRom, shut, CurveWrap::Pinned),
        (Basis::Nurbs, _) => CurveForm::Nurbs {
            orders: Vec::new(),
            knots: Vec::new(),
            ranges: Vec::new(),
            weights: Vec::new(),
        },
        _ => return None,
    })
}

/// Whether primitive `p` is a polyline closing on its first point.
pub(super) fn closes(table: &Primitives, p: usize) -> bool {
    let points = table.primitive(p);
    table.kind(p) == Kind::POLYLINE && points.len() > 3 && points.first() == points.last()
}

/// The curves `members` of `value`, all of one `form` (and `degree`) and
/// closed polylines when `closed`, as one prim (each curve's points its
/// own).
pub(super) fn curves(
    value: &Geometry,
    (mut form, degree, closed): (CurveForm, u8, bool),
    members: &[usize],
    colors: &dyn ColorService,
    warnings: &mut Vec<String>,
) -> Curves {
    let table = value.primitives();
    let mut counts = Vec::with_capacity(members.len());
    // Each written point's point and vertex in `value`.
    let (mut at, mut vertices) = (Vec::new(), Vec::new());
    for &p in members {
        let points = table.primitive(p);
        let n = points.len() - usize::from(closed);
        counts.push(n as i32);
        at.extend(points[..n].iter().map(|&i| i as usize));
        vertices.extend(table.primitive_vertices(p).take(n));
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
    for (domain, name, attribute) in value.attributes().iter() {
        let (interpolation, picked) = match domain {
            Domain::Point => (Interpolation::Vertex, Some(&at[..])),
            Domain::Vertex => (Interpolation::Vertex, Some(&vertices[..])),
            Domain::Primitive => (Interpolation::Uniform, Some(members)),
            Domain::Detail => (Interpolation::Constant, None),
            _ => continue,
        };
        // The material is the prim's binding.
        if name == names::MATERIAL {
            continue;
        }
        match primvar(name, attribute, interpolation, picked, colors, warnings) {
            Some(written) => primvars.push(widths(name, written)),
            None => warnings.push(format!("attribute {name} has no USD primvar; left out")),
        }
    }
    Curves {
        form,
        vertex_counts: Arc::new(counts),
        points: positions(value, at.into_iter()),
        primvars,
    }
}

fn cubic(basis: CurveBasis, closed: bool, open: CurveWrap) -> CurveForm {
    let wrap = if closed { CurveWrap::Periodic } else { open };
    CurveForm::Cubic { basis, wrap }
}

/// Points alone, `id` as `ids` and the other attributes as primvars.
pub(super) fn points(
    value: &Geometry,
    colors: &dyn ColorService,
    warnings: &mut Vec<String>,
) -> Points {
    let attributes = value.attributes();
    let ids = attributes.tuples::<i64>(Domain::Point, names::ID);
    let mut primvars = Vec::new();
    for (domain, name, attribute) in attributes.iter() {
        let interpolation = match domain {
            Domain::Point => Interpolation::Vertex,
            Domain::Detail => Interpolation::Constant,
            _ => continue,
        };
        if name == names::ID && ids.is_some() {
            continue;
        }
        match primvar(name, attribute, interpolation, None, colors, warnings) {
            Some(written) => primvars.push(widths(name, written)),
            None => warnings.push(format!("attribute {name} has no USD primvar; left out")),
        }
    }
    Points {
        points: positions(value, 0..value.positions().len()),
        ids: ids.map(|ids| Arc::new(ids.to_vec())),
        primvars,
    }
}

/// `written` (attribute `name`'s primvar) as `widths` when it is `width`:
/// curves' and points' own attribute.
fn widths(name: &str, written: Primvar) -> Primvar {
    match name {
        names::WIDTH => Primvar {
            name: Token::new("widths"),
            ..written
        },
        _ => written,
    }
}
