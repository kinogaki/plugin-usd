//! A stage's space and the project's: its `metersPerUnit` and `upAxis`
//! against the project's units, as the SDK's [`Conversion`]: a placement
//! `M` of the stage becomes `C·M·C⁻¹` in the project (a camera's or a
//! light's `Q·M`: its value keeps the frame it looks down), a point `p`
//! becomes `C·p`, so a hierarchy keeps its shape and no placement carries
//! the change of units. And USD's matrices (row vectors) against the SDK's
//! placements (column vectors).
use crate::usd::scene::{Matrix4d, UpAxis as StageUp};
use kinogaki_studio_api::Error;
use kinogaki_studio_api::format::Conversion;
use kinogaki_studio_api::geometry::{Units, UpAxis, Xform};

/// The change from a stage of `meters_per_unit`, `up`, into `project`.
///
/// # Errors
/// A unit that isn't a finite length above zero.
pub(crate) fn conversion(
    meters_per_unit: f64,
    up: StageUp,
    project: Units,
) -> Result<Conversion, Error> {
    let up = match up {
        StageUp::Y => UpAxis::Y,
        StageUp::Z => UpAxis::Z,
    };
    Ok(Conversion::new(Units::new(meters_per_unit, up)?, project))
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
    use kinogaki_studio_api::geometry::Vec3;

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-12)
    }

    #[test]
    fn a_centimetre_z_up_stage_lands_in_metres_y_up_with_its_hierarchy() {
        let project = Units::new(1.0, UpAxis::Y).unwrap();
        let space = conversion(0.01, StageUp::Z, project).unwrap();
        // 100 cm up the stage's Z is 1 m up the project's Y.
        assert!(close(
            space.point(Vec3::new(0.0, 0.0, 100.0)).to_array(),
            [0.0, 1.0, 0.0]
        ));
        // A parent raised 100 cm and a child 50 cm along X: the child's
        // origin lands where the stage puts it, in the project's space.
        let parent = space.placement(&xform(&Matrix4d::translation(0.0, 0.0, 100.0)));
        let child = space.placement(&xform(&Matrix4d::translation(50.0, 0.0, 0.0)));
        let at = parent.compose(&child).apply_point(Vec3::ZERO);
        assert!(close(at.to_array(), [0.5, 1.0, 0.0]));
        // Placements carry no scale: the change of units is in the points.
        assert!(close(parent.axes()[1].to_array(), [0.0, 1.0, 0.0]));
        assert!(conversion(0.0, StageUp::Y, project).is_err());
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
