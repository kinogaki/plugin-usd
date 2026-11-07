//! UsdGeom curves and points as authored: BasisCurves (linear, or cubic
//! Bézier, B-spline and Catmull-Rom, each nonperiodic, periodic or pinned)
//! and NurbsCurves (an order per curve, knots, ranges and point weights),
//! with `widths`, `normals` and primvars; and Points, with `widths`, `ids`
//! and primvars.

use std::sync::Arc;

use super::primvar::{Interpolation, Primvar, read_primvars};
use super::values::{Time, int64s, ints, reals, token_at, value_at, vec3fs};
use crate::usd::sdf::{LayerData, Path};

/// A cubic BasisCurves' basis (`basis`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveBasis {
    /// `bezier` (the fallback): segments of 4 points sharing ends.
    Bezier,
    /// `bspline`: uniform cubic B-splines.
    BSpline,
    /// `catmullRom`: through the inner points.
    CatmullRom,
}

/// How a BasisCurves' curves end (`wrap`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveWrap {
    /// `nonperiodic` (the fallback): open.
    Nonperiodic,
    /// `periodic`: closed.
    Periodic,
    /// `pinned`: open B-splines and Catmull-Roms that reach their end
    /// points (phantom points mirror the ends).
    Pinned,
}

/// What kind of curves a prim holds.
#[derive(Clone, Debug, PartialEq)]
pub enum CurveForm {
    /// BasisCurves of `type = "linear"`: polylines.
    Linear {
        /// `wrap = "periodic"`: each polyline closes.
        periodic: bool,
    },
    /// BasisCurves of `type = "cubic"` (the fallback).
    Cubic {
        /// The basis.
        basis: CurveBasis,
        /// How the curves end.
        wrap: CurveWrap,
    },
    /// NurbsCurves.
    Nurbs {
        /// `order` (degree + 1), one per curve.
        orders: Vec<i32>,
        /// `knots`: each curve's `count + order` knots, one curve after
        /// another.
        knots: Vec<f64>,
        /// `ranges`, one per curve; empty when unauthored.
        ranges: Vec<[f64; 2]>,
        /// `pointWeights`, one per point; empty: non-rational.
        weights: Vec<f64>,
    },
}

/// One BasisCurves or NurbsCurves prim.
#[derive(Clone, Debug, PartialEq)]
pub struct Curves {
    /// Linear, cubic or NURBS.
    pub form: CurveForm,
    /// `curveVertexCounts`: points per curve.
    pub vertex_counts: Arc<Vec<i32>>,
    /// Points in the prim's own space, curve after curve.
    pub points: Arc<Vec<[f32; 3]>>,
    /// Primvars in name order, `widths` and `normals` included under those
    /// names.
    pub primvars: Vec<Primvar>,
}

impl Curves {
    /// Values a primvar of `interpolation` holds: one, one per curve, one
    /// per point, or one per segment end (varying and faceVarying).
    pub fn elements(&self, interpolation: Interpolation) -> usize {
        match interpolation {
            Interpolation::Constant => 1,
            Interpolation::Uniform => self.vertex_counts.len(),
            Interpolation::Vertex => self.points.len(),
            Interpolation::Varying | Interpolation::FaceVarying => self.varying(),
        }
    }

    /// Segment ends over every curve, as UsdGeomCurves counts them.
    fn varying(&self) -> usize {
        let counts = self.vertex_counts.iter().map(|&c| c as usize);
        match &self.form {
            CurveForm::Linear { .. } => self.points.len(),
            CurveForm::Cubic { basis, wrap } => counts
                .map(|n| {
                    let segments = match (basis, wrap) {
                        (CurveBasis::Bezier, CurveWrap::Periodic) => n / 3,
                        (CurveBasis::Bezier, _) => n.saturating_sub(1) / 3,
                        (_, CurveWrap::Periodic) => n,
                        (_, CurveWrap::Pinned) => n.saturating_sub(1),
                        (_, CurveWrap::Nonperiodic) => n.saturating_sub(3),
                    };
                    segments + usize::from(*wrap != CurveWrap::Periodic)
                })
                .sum(),
            CurveForm::Nurbs { orders, .. } => counts
                .zip(orders)
                .map(|(n, &order)| (n + 2).saturating_sub(order.max(0) as usize))
                .sum(),
        }
    }
}

/// Read the BasisCurves (`nurbs` false) or NurbsCurves at `source`; `Err`
/// names why it can't be read.
pub(super) fn read(
    layer: &LayerData,
    source: &Path,
    nurbs: bool,
    time: Time,
) -> Result<Curves, String> {
    let points = value_at(layer, source, "points", time)
        .as_ref()
        .and_then(vec3fs);
    let counts = value_at(layer, source, "curveVertexCounts", time)
        .as_ref()
        .and_then(ints);
    let (Some(points), Some(counts)) = (points, counts) else {
        return Err("points or curveVertexCounts missing or of the wrong type".into());
    };
    let total: i64 = counts.iter().map(|&c| i64::from(c)).sum();
    if counts.iter().any(|&c| c < 1) || total != points.len() as i64 {
        return Err("curveVertexCounts do not add up to the points".into());
    }
    let form = if nurbs {
        nurbs_form(layer, source, &counts, points.len(), time)?
    } else {
        let wrap = match token_at(layer, source, "wrap", "nonperiodic").as_str() {
            "periodic" => CurveWrap::Periodic,
            "pinned" => CurveWrap::Pinned,
            _ => CurveWrap::Nonperiodic,
        };
        match token_at(layer, source, "type", "cubic").as_str() {
            "linear" => CurveForm::Linear {
                periodic: wrap == CurveWrap::Periodic,
            },
            _ => CurveForm::Cubic {
                basis: match token_at(layer, source, "basis", "bezier").as_str() {
                    "bspline" => CurveBasis::BSpline,
                    "catmullRom" => CurveBasis::CatmullRom,
                    _ => CurveBasis::Bezier,
                },
                wrap,
            },
        }
    };
    let mut curves = Curves {
        form,
        vertex_counts: counts,
        points,
        primvars: Vec::new(),
    };
    let primvars = read_primvars(layer, source, &["widths", "normals"], time);
    if let Some(misfit) = primvars
        .iter()
        .find(|p| !p.fits(curves.elements(p.interpolation)))
    {
        return Err(format!("primvar {} does not fit the curves", misfit.name));
    }
    curves.primvars = primvars;
    Ok(curves)
}

/// A NurbsCurves' orders, knots, ranges and weights, checked against the
/// counts.
fn nurbs_form(
    layer: &LayerData,
    source: &Path,
    counts: &[i32],
    points: usize,
    time: Time,
) -> Result<CurveForm, String> {
    let orders = value_at(layer, source, "order", time)
        .and_then(|v| ints(&v))
        .map(|o| o.to_vec())
        .unwrap_or_default();
    let reals_of = |name: &str| value_at(layer, source, name, time).and_then(|v| reals(&v));
    let knots = reals_of("knots").map(|(k, _)| k).unwrap_or_default();
    let weights = reals_of("pointWeights").map(|(w, _)| w).unwrap_or_default();
    let ranges = match reals_of("ranges") {
        Some((r, 2)) => r.as_chunks::<2>().0.to_vec(),
        _ => Vec::new(),
    };
    if orders.len() != counts.len() || orders.iter().zip(counts).any(|(&o, &c)| o < 2 || o > c) {
        return Err("order needs one order of 2 to the point count per curve".into());
    }
    let needed: i64 = counts
        .iter()
        .zip(&orders)
        .map(|(&c, &o)| i64::from(c) + i64::from(o))
        .sum();
    if knots.len() as i64 != needed {
        return Err("knots need a curve's point count plus its order each".into());
    }
    if !weights.is_empty() && weights.len() != points {
        return Err("pointWeights need one weight per point".into());
    }
    if !ranges.is_empty() && ranges.len() != counts.len() {
        return Err("ranges need one range per curve".into());
    }
    Ok(CurveForm::Nurbs {
        orders,
        knots,
        ranges,
        weights,
    })
}

/// One Points prim.
#[derive(Clone, Debug, PartialEq)]
pub struct Points {
    /// Points in the prim's own space.
    pub points: Arc<Vec<[f32; 3]>>,
    /// `ids`, one per point, when authored.
    pub ids: Option<Arc<Vec<i64>>>,
    /// Primvars in name order, `widths` and `normals` included.
    pub primvars: Vec<Primvar>,
}

/// Read the Points at `source`; `Err` names why it can't be read.
pub(super) fn read_points(layer: &LayerData, source: &Path, time: Time) -> Result<Points, String> {
    let Some(points) = value_at(layer, source, "points", time)
        .as_ref()
        .and_then(vec3fs)
    else {
        return Err("points missing or of the wrong type".into());
    };
    let ids = value_at(layer, source, "ids", time)
        .as_ref()
        .and_then(int64s);
    if ids.as_ref().is_some_and(|ids| ids.len() != points.len()) {
        return Err("ids need one id per point".into());
    }
    let primvars = read_primvars(layer, source, &["widths", "normals"], time);
    let elements = |interpolation| match interpolation {
        Interpolation::Constant => 1,
        Interpolation::Uniform => 1,
        _ => points.len(),
    };
    if let Some(misfit) = primvars.iter().find(|p| !p.fits(elements(p.interpolation))) {
        return Err(format!("primvar {} does not fit the points", misfit.name));
    }
    Ok(Points {
        points,
        ids,
        primvars,
    })
}
