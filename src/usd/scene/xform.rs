// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Xform op stacks, after `UsdGeomXformable::GetLocalTransformation` and
//! `UsdGeomXformOp::GetOpTransform`: `xformOpOrder` names the ops outermost
//! first, each an attribute `xformOp:<type>[:<suffix>]`; `!invert!`
//! inverts one, and a leading `!resetXformStack!` makes the prim ignore its
//! parents' transform. Ops whose attribute is missing or unusable are
//! skipped, as UsdGeomXformable does after reporting them.

use super::matrix::Matrix4d;
use super::values::{Time, reals, value_at};
use crate::usd::sdf::{LayerData, Path, Value};

/// A prim's local transform and whether it resets its parents'.
#[derive(Clone, Copy, Debug)]
pub struct LocalTransform {
    /// The local matrix.
    pub matrix: Matrix4d,
    /// Whether the prim ignores its parents' transform.
    pub resets: bool,
}

/// The axes a three-angle rotation turns about, in the order it applies.
fn rotation_order(name: &str) -> Option<[usize; 3]> {
    Some(match name {
        "rotateXYZ" => [0, 1, 2],
        "rotateXZY" => [0, 2, 1],
        "rotateYXZ" => [1, 0, 2],
        "rotateYZX" => [1, 2, 0],
        "rotateZXY" => [2, 0, 1],
        "rotateZYX" => [2, 1, 0],
        _ => return None,
    })
}

/// The matrix one op's value gives; none when the value does not fit.
fn op_matrix(kind: &str, value: &Value) -> Option<Matrix4d> {
    if let Value::Matrix4d(m) = value {
        return (kind == "transform").then_some(Matrix4d(*m));
    }
    let (v, _) = reals(value)?;
    let one = |v: &[f64]| (v.len() == 1).then(|| v[0]);
    Some(match kind {
        "translate" if v.len() == 3 => Matrix4d::translation(v[0], v[1], v[2]),
        "translateX" => Matrix4d::translation(one(&v)?, 0.0, 0.0),
        "translateY" => Matrix4d::translation(0.0, one(&v)?, 0.0),
        "translateZ" => Matrix4d::translation(0.0, 0.0, one(&v)?),
        "scale" if v.len() == 3 => Matrix4d::scaling(v[0], v[1], v[2]),
        "scaleX" => Matrix4d::scaling(one(&v)?, 1.0, 1.0),
        "scaleY" => Matrix4d::scaling(1.0, one(&v)?, 1.0),
        "scaleZ" => Matrix4d::scaling(1.0, 1.0, one(&v)?),
        "rotateX" => Matrix4d::rotation(0, one(&v)?),
        "rotateY" => Matrix4d::rotation(1, one(&v)?),
        "rotateZ" => Matrix4d::rotation(2, one(&v)?),
        // Quaternions are stored imaginary first.
        "orient" if v.len() == 4 => Matrix4d::quaternion(v[3], v[0], v[1], v[2]),
        _ => {
            let order = rotation_order(kind)?;
            if v.len() != 3 {
                return None;
            }
            // rotateABC turns about A first: RA * RB * RC for row vectors.
            Matrix4d::rotation(order[0], v[order[0]])
                .times(&Matrix4d::rotation(order[1], v[order[1]]))
                .times(&Matrix4d::rotation(order[2], v[order[2]]))
        }
    })
}

/// The local transform of the prim at `prim`.
pub fn local_transform(layer: &LayerData, prim: &Path, time: Time) -> LocalTransform {
    let mut result = LocalTransform {
        matrix: Matrix4d::IDENTITY,
        resets: false,
    };
    let Some(order) = value_at(layer, prim, "xformOpOrder", Time::Default) else {
        return result;
    };
    let Some(ops) = order.as_array().and_then(|array| match array {
        crate::usd::sdf::Array::Token(tokens) => Some(tokens.clone()),
        _ => None,
    }) else {
        return result;
    };
    // Row vectors: the last op applies first, so the product runs from it.
    for (index, op) in ops.iter().enumerate().rev() {
        if op.as_str() == "!resetXformStack!" {
            result.resets |= index == 0;
            continue;
        }
        let (invert, name) = match op.strip_prefix("!invert!") {
            Some(name) => (true, name),
            None => (false, op.as_str()),
        };
        let Some(kind) = name
            .strip_prefix("xformOp:")
            .map(|rest| rest.split(':').next().unwrap_or(""))
        else {
            continue;
        };
        let Some(value) = value_at(layer, prim, name, time) else {
            continue;
        };
        let Some(mut matrix) = op_matrix(kind, &value) else {
            continue;
        };
        if invert {
            match matrix.inverse() {
                Some(inverse) => matrix = inverse,
                None => continue,
            }
        }
        result.matrix = result.matrix.times(&matrix);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_angle_rotations_turn_about_the_first_axis_first() {
        let m = op_matrix("rotateXYZ", &Value::Vec3f([90.0, 90.0, 0.0])).unwrap();
        let expected = Matrix4d::rotation(0, 90.0).times(&Matrix4d::rotation(1, 90.0));
        assert!(
            m.0.iter()
                .zip(expected.0)
                .all(|(a, b)| (a - b).abs() < 1e-6)
        );
        assert!(op_matrix("translate", &Value::Double(1.0)).is_none());
        assert!(op_matrix("frobnicate", &Value::Vec3f([0.0; 3])).is_none());
    }
}
