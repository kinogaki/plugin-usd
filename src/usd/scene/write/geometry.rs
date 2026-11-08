//! Geometry authored as its prims: meshes with their topology, `extent`,
//! primvars, subdivision tags and GeomSubsets; implicit shapes; curves;
//! points; point instancers. Values are written in the types they were
//! read in.

use std::sync::Arc;

use super::Writer;
use crate::usd::error::{Error, Result};
use crate::usd::scene::{
    Author, CurveBasis, CurveForm, CurveWrap, Curves, Instancer, Mesh, Points, Primvar, Shape,
    ShapeForm, SubsetElement,
};
use crate::usd::sdf::{Array, Half, ListOp, Path, Token, Value};

/// An int array value.
fn ints(values: &Arc<Vec<i32>>) -> Value {
    Value::Array(Array::Int(values.clone()))
}

/// A token value.
fn token(text: &str) -> Value {
    Value::Token(Token::new(text))
}

/// `points` (as `point3f[]`) and their `extent`.
pub(super) fn points(author: &mut Author, path: &Path, points: &Arc<Vec<[f32; 3]>>) {
    if let Some(first) = points.first() {
        let (mut low, mut high) = (*first, *first);
        for p in points.iter() {
            for axis in 0..3 {
                low[axis] = low[axis].min(p[axis]);
                high[axis] = high[axis].max(p[axis]);
            }
        }
        let extent = Value::Array(Array::Vec3f(Arc::new(vec![low, high])));
        author.attribute(path, "extent", "float3[]", extent, false);
    }
    let value = Value::Array(Array::Vec3f(points.clone()));
    author.attribute(path, "points", "point3f[]", value, false);
}

/// Primvars as `primvars:<name>` attributes (`normals`, `widths` and
/// `velocities` as those attributes), with interpolation, element size,
/// color space and indices.
pub(super) fn primvars(author: &mut Author, path: &Path, primvars: &[Primvar]) -> Result<()> {
    for primvar in primvars {
        let name = match primvar.name.as_str() {
            "normals" | "widths" | "velocities" => primvar.name.to_string(),
            other => format!("primvars:{other}"),
        };
        let type_name = match primvar.type_name.as_str() {
            "" => {
                let kind = primvar.values.attribute_kind().and_then(|k| k.type_name());
                let kind = kind.ok_or_else(|| {
                    Error::invalid(format!(
                        "{path}: primvar {} holds no attribute value",
                        primvar.name
                    ))
                })?;
                let array = matches!(primvar.values, Value::Array(_));
                format!("{kind}{}", if array { "[]" } else { "" })
            }
            declared => declared.to_owned(),
        };
        let property = author.attribute(path, &name, &type_name, primvar.values.clone(), false);
        author.property_metadata(
            &property,
            "interpolation",
            token(primvar.interpolation.token()),
        );
        if primvar.element_size > 1 {
            author.property_metadata(
                &property,
                "elementSize",
                Value::Int(primvar.element_size as i32),
            );
        }
        if let Some(space) = &primvar.color_space {
            author.property_metadata(&property, "colorSpace", Value::Token(space.clone()));
        }
        if let Some(indices) = &primvar.indices {
            author.attribute(
                path,
                &format!("{name}:indices"),
                "int[]",
                ints(indices),
                false,
            );
        }
    }
    Ok(())
}

impl Writer<'_> {
    /// Author `mesh` at `path` as a Mesh prim.
    pub(super) fn mesh(&self, author: &mut Author, path: &Path, mesh: &Mesh) -> Result<()> {
        author.prim(path, "Mesh");
        points(author, path, &mesh.points);
        author.attribute(
            path,
            "faceVertexCounts",
            "int[]",
            ints(&mesh.face_vertex_counts),
            false,
        );
        author.attribute(
            path,
            "faceVertexIndices",
            "int[]",
            ints(&mesh.face_vertex_indices),
            false,
        );
        if mesh.left_handed {
            author.attribute(path, "orientation", "token", token("leftHanded"), true);
        }
        if mesh.double_sided {
            author.attribute(path, "doubleSided", "bool", Value::Bool(true), true);
        }
        primvars(author, path, &mesh.primvars)?;
        let tags = &mesh.subdivision;
        let scheme = Value::Token(tags.scheme.clone());
        author.attribute(path, "subdivisionScheme", "token", scheme, true);
        if tags.interpolate_boundary.as_str() != "edgeAndCorner" {
            let boundary = Value::Token(tags.interpolate_boundary.clone());
            author.attribute(path, "interpolateBoundary", "token", boundary, false);
        }
        let signed = |values: &[i32]| Value::Array(Array::Int(Arc::new(values.to_vec())));
        let reals = |values: &[f64]| {
            Value::Array(Array::Float(Arc::new(
                values.iter().map(|&v| v as f32).collect(),
            )))
        };
        let mut int_tag = |name: &str, values: &[i32]| {
            if !values.is_empty() {
                author.attribute(path, name, "int[]", signed(values), false);
            }
        };
        int_tag("creaseIndices", &tags.crease_indices);
        int_tag("creaseLengths", &tags.crease_lengths);
        int_tag("cornerIndices", &tags.corner_indices);
        int_tag("holeIndices", &tags.hole_indices);
        for (name, values) in [
            ("creaseSharpnesses", &tags.crease_sharpnesses),
            ("cornerSharpnesses", &tags.corner_sharpnesses),
        ] {
            if !values.is_empty() {
                author.attribute(path, name, "float[]", reals(values), false);
            }
        }
        // Per-face material subsets must not overlap (UsdShade's rule).
        if mesh
            .subsets
            .iter()
            .any(|s| s.family.as_str() == "materialBind")
        {
            let family = "subsetFamily:materialBind:familyType";
            author.attribute(path, family, "token", token("nonOverlapping"), true);
        }
        for subset in &mesh.subsets {
            let at = self.child(path, &subset.name)?;
            author.prim(&at, "GeomSubset");
            let element = match subset.element {
                SubsetElement::Face => "face",
                SubsetElement::Point => "point",
                SubsetElement::Edge => "edge",
            };
            author.attribute(&at, "elementType", "token", token(element), true);
            if !subset.family.is_empty() {
                author.attribute(
                    &at,
                    "familyName",
                    "token",
                    Value::Token(subset.family.clone()),
                    true,
                );
            }
            author.attribute(&at, "indices", "int[]", ints(&subset.indices), false);
            self.bind(author, &at, subset.material, false)?;
        }
        Ok(())
    }

    /// Author `shape` at `path` as its intrinsic prim.
    pub(super) fn shape(&self, author: &mut Author, path: &Path, shape: &Shape) -> Result<()> {
        author.prim(path, shape.type_name());
        let mut number = |name: &str, value: f64| {
            author.attribute(path, name, "double", Value::Double(value), false);
        };
        let axis = match &shape.form {
            ShapeForm::Cube { size } => {
                number("size", *size);
                None
            }
            ShapeForm::Sphere { radius } => {
                number("radius", *radius);
                None
            }
            ShapeForm::Cylinder {
                radius,
                height,
                axis,
            }
            | ShapeForm::Cone {
                radius,
                height,
                axis,
            }
            | ShapeForm::Capsule {
                radius,
                height,
                axis,
            } => {
                number("radius", *radius);
                number("height", *height);
                Some(axis)
            }
            ShapeForm::Plane {
                width,
                length,
                axis,
            } => {
                number("width", *width);
                number("length", *length);
                Some(axis)
            }
        };
        if let Some(axis) = axis {
            author.attribute(path, "axis", "token", Value::Token(axis.clone()), true);
        }
        if shape.double_sided {
            author.attribute(path, "doubleSided", "bool", Value::Bool(true), true);
        }
        primvars(author, path, &shape.primvars)
    }

    /// Author `curves` at `path` as its BasisCurves or NurbsCurves prim.
    pub(super) fn curves(&self, author: &mut Author, path: &Path, curves: &Curves) -> Result<()> {
        let nurbs = matches!(curves.form, CurveForm::Nurbs { .. });
        author.prim(path, if nurbs { "NurbsCurves" } else { "BasisCurves" });
        points(author, path, &curves.points);
        author.attribute(
            path,
            "curveVertexCounts",
            "int[]",
            ints(&curves.vertex_counts),
            false,
        );
        let doubles = |v: &[f64]| Value::Array(Array::Double(Arc::new(v.to_vec())));
        match &curves.form {
            CurveForm::Linear { periodic } => {
                author.attribute(path, "type", "token", token("linear"), true);
                if *periodic {
                    author.attribute(path, "wrap", "token", token("periodic"), true);
                }
            }
            CurveForm::Cubic { basis, wrap } => {
                let basis = match basis {
                    CurveBasis::Bezier => "bezier",
                    CurveBasis::BSpline => "bspline",
                    CurveBasis::CatmullRom => "catmullRom",
                };
                let wrap = match wrap {
                    CurveWrap::Nonperiodic => "nonperiodic",
                    CurveWrap::Periodic => "periodic",
                    CurveWrap::Pinned => "pinned",
                };
                author.attribute(path, "type", "token", token("cubic"), true);
                author.attribute(path, "basis", "token", token(basis), true);
                author.attribute(path, "wrap", "token", token(wrap), true);
            }
            CurveForm::Nurbs {
                orders,
                knots,
                ranges,
                weights,
            } => {
                let orders = Value::Array(Array::Int(Arc::new(orders.clone())));
                author.attribute(path, "order", "int[]", orders, false);
                author.attribute(path, "knots", "double[]", doubles(knots), false);
                if !ranges.is_empty() {
                    let ranges = Value::Array(Array::Vec2d(Arc::new(ranges.clone())));
                    author.attribute(path, "ranges", "double2[]", ranges, false);
                }
                if !weights.is_empty() {
                    author.attribute(path, "pointWeights", "double[]", doubles(weights), false);
                }
            }
        }
        primvars(author, path, &curves.primvars)
    }

    /// Author `points` at `path` as a Points prim.
    pub(super) fn points(&self, author: &mut Author, path: &Path, prim: &Points) -> Result<()> {
        author.prim(path, "Points");
        points(author, path, &prim.points);
        if let Some(ids) = &prim.ids {
            author.attribute(
                path,
                "ids",
                "int64[]",
                Value::Array(Array::Int64(ids.clone())),
                false,
            );
        }
        primvars(author, path, &prim.primvars)
    }

    /// Author `instancer` at `path` as a PointInstancer, its prototypes
    /// beneath it in `Prototypes`.
    pub(super) fn instancer(
        &self,
        author: &mut Author,
        path: &Path,
        instancer: &Instancer,
        depth: usize,
    ) -> Result<()> {
        author.prim(path, "PointInstancer");
        let holder = path.child("Prototypes");
        author.prim(&holder, "Scope");
        let mut targets = Vec::with_capacity(instancer.prototypes.len());
        for (rank, &index) in instancer.prototypes.iter().enumerate() {
            let prototype = self.scene.prototypes.get(index).ok_or_else(|| {
                Error::invalid(format!("{path}: prototype {index} does not exist"))
            })?;
            // A prototype is one prim: several top prims are grouped.
            let tops: Vec<usize> = (prototype.nodes.iter().enumerate())
                .filter(|(_, node)| node.parent.is_none())
                .map(|(at, _)| at)
                .collect();
            let target = match tops.as_slice() {
                [only] => holder.child(prototype.nodes[*only].name.clone()),
                _ => {
                    let group = holder.child(format!("Prototype_{rank}"));
                    author.prim(&group, "Xform");
                    group
                }
            };
            let parent = if tops.len() == 1 {
                holder.clone()
            } else {
                target.clone()
            };
            self.nodes(author, &parent, &prototype.nodes, depth + 1)?;
            targets.push(target);
        }
        author.relationship(path, "prototypes", targets);
        author.attribute(
            path,
            "protoIndices",
            "int[]",
            ints(&instancer.proto_indices),
            false,
        );
        let positions = Value::Array(Array::Vec3f(instancer.positions.clone()));
        author.attribute(path, "positions", "point3f[]", positions, false);
        // Both: `orientationsf` holds them whole (OpenUSD 24.05 on, which
        // takes it over the other), the half-precision `orientations` for
        // readers before it.
        if let Some(orientations) = &instancer.orientations {
            let halves = orientations.iter().map(|q| q.map(Half::from_f32)).collect();
            let value = Value::Array(Array::Quath(Arc::new(halves)));
            author.attribute(path, "orientations", "quath[]", value, false);
            let whole = Value::Array(Array::Quatf(orientations.clone()));
            author.attribute(path, "orientationsf", "quatf[]", whole, false);
        }
        if let Some(scales) = &instancer.scales {
            let value = Value::Array(Array::Vec3f(scales.clone()));
            author.attribute(path, "scales", "float3[]", value, false);
        }
        if let Some(ids) = &instancer.ids {
            author.attribute(
                path,
                "ids",
                "int64[]",
                Value::Array(Array::Int64(ids.clone())),
                false,
            );
        }
        if !instancer.invisible_ids.is_empty() {
            let hidden = Value::Array(Array::Int64(Arc::new(instancer.invisible_ids.clone())));
            author.attribute(path, "invisibleIds", "int64[]", hidden, false);
        }
        if !instancer.inactive_ids.is_empty() {
            let inactive = ListOp::explicit(instancer.inactive_ids.clone());
            author.prim_metadata(path, "inactiveIds", Value::Int64ListOp(Arc::new(inactive)));
        }
        primvars(author, path, &instancer.primvars)
    }
}
