//! A stage's space and the project's: its `metersPerUnit` and `upAxis`
//! against the project's units ([`Space`]), and USD's matrices (row
//! vectors) against the SDK's placements (column vectors).
use crate::usd::scene::{Matrix4d, UpAxis as StageUp};
use kinogaki_studio_api::geometry::{Units, UpAxis, Vec3, Xform};

/// The change from a stage's space into the project's: a uniform scale
/// (the stage's unit over the project's) and the turn between their up
/// axes. A placement `M` of the stage becomes `C·M·C⁻¹` in the project, a
/// point `p` becomes `C·p`, so a hierarchy keeps its shape and no
/// placement carries the change of units.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Space {
    /// The project's units per stage unit.
    pub scale: f64,
    /// The turn from the stage's up axis to the project's.
    turn: Xform,
}

impl Space {
    /// The change from a stage of `meters_per_unit`, `up`, into `project`.
    pub fn new(meters_per_unit: f64, up: StageUp, project: Units) -> Space {
        let scale = meters_per_unit / project.meters_per_unit();
        let from = match up {
            StageUp::Y => UpAxis::Y,
            StageUp::Z => UpAxis::Z,
        };
        let turn = match (from, project.up()) {
            (UpAxis::Y, UpAxis::Z) => Xform::from_axes(Vec3::X, Vec3::Z, -Vec3::Y, Vec3::ZERO),
            (UpAxis::Z, UpAxis::Y) => Xform::from_axes(Vec3::X, -Vec3::Z, Vec3::Y, Vec3::ZERO),
            _ => Xform::IDENTITY,
        };
        Space { scale, turn }
    }

    /// A point of the stage in the project.
    pub fn point(&self, p: [f64; 3]) -> [f64; 3] {
        (self.turn.apply_point(Vec3::from(p)) * self.scale).to_array()
    }

    /// A direction of the stage (a normal) in the project: turned, not
    /// scaled.
    pub fn direction(&self, v: [f64; 3]) -> [f64; 3] {
        self.turn.apply_vector(Vec3::from(v)).to_array()
    }

    /// A placement of the stage (USD's row-vector matrix) in the project.
    pub fn placement(&self, m: &Matrix4d) -> Xform {
        let inverse = self.turn.inverse().unwrap_or(Xform::IDENTITY);
        let local = xform(m);
        let moved = self.turn.compose(&local).compose(&inverse);
        let rows = moved.rows();
        let t = [rows[0][3], rows[1][3], rows[2][3]].map(|c| c * self.scale);
        Xform::from_rows([
            [rows[0][0], rows[0][1], rows[0][2], t[0]],
            [rows[1][0], rows[1][1], rows[1][2], t[1]],
            [rows[2][0], rows[2][1], rows[2][2], t[2]],
        ])
    }
}

/// USD's matrix (row vectors, translation in the last row) as a placement.
pub(crate) fn xform(m: &Matrix4d) -> Xform {
    let m = &m.0;
    Xform::from_rows([
        [m[0], m[4], m[8], m[12]],
        [m[1], m[5], m[9], m[13]],
        [m[2], m[6], m[10], m[14]],
    ])
}

/// A placement as USD's matrix.
pub(crate) fn matrix(x: &Xform) -> Matrix4d {
    let r = x.rows();
    Matrix4d([
        r[0][0], r[1][0], r[2][0], 0.0, //
        r[0][1], r[1][1], r[2][1], 0.0, //
        r[0][2], r[1][2], r[2][2], 0.0, //
        r[0][3], r[1][3], r[2][3], 1.0,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-12)
    }

    #[test]
    fn a_centimetre_z_up_stage_lands_in_metres_y_up_with_its_hierarchy() {
        let project = Units::new(1.0, UpAxis::Y).unwrap();
        let space = Space::new(0.01, StageUp::Z, project);
        // 100 cm up the stage's Z is 1 m up the project's Y.
        assert!(close(space.point([0.0, 0.0, 100.0]), [0.0, 1.0, 0.0]));
        assert!(close(space.direction([0.0, 0.0, 1.0]), [0.0, 1.0, 0.0]));
        // A parent raised 100 cm and a child 50 cm along X: the child's
        // origin lands where the stage puts it, in the project's space.
        let parent = space.placement(&Matrix4d::translation(0.0, 0.0, 100.0));
        let child = space.placement(&Matrix4d::translation(50.0, 0.0, 0.0));
        let at = parent.compose(&child).apply_point(Vec3::ZERO);
        assert!(close(at.to_array(), [0.5, 1.0, 0.0]));
        // Placements carry no scale: the change of units is in the points.
        assert!(close(parent.axes()[1].to_array(), [0.0, 1.0, 0.0]));
    }

    #[test]
    fn matrices_and_placements_convert_both_ways() {
        let m = Matrix4d::rotation(2, 30.0).times(&Matrix4d::translation(1.0, 2.0, 3.0));
        let x = xform(&m);
        assert_eq!(matrix(&x), m);
        let p = m.transform_point([1.0, 0.0, 0.0]);
        let q = x.apply_point(Vec3::X);
        assert!(close(p, q.to_array()));
    }
}
