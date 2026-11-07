//! 4x4 double matrices as Gf lays them out: row-major, row vectors
//! (`p' = p * M`), translation in the last row.

/// A row-major 4x4 transform (`GfMatrix4d`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix4d(pub [f64; 16]);

impl Default for Matrix4d {
    fn default() -> Matrix4d {
        Matrix4d::IDENTITY
    }
}

impl Matrix4d {
    /// The identity.
    pub const IDENTITY: Matrix4d = Matrix4d([
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]);

    /// A translation.
    pub fn translation(x: f64, y: f64, z: f64) -> Matrix4d {
        let mut m = Matrix4d::IDENTITY;
        m.0[12] = x;
        m.0[13] = y;
        m.0[14] = z;
        m
    }

    /// A scale.
    pub fn scaling(x: f64, y: f64, z: f64) -> Matrix4d {
        let mut m = Matrix4d::IDENTITY;
        m.0[0] = x;
        m.0[5] = y;
        m.0[10] = z;
        m
    }

    /// A rotation of `degrees` about axis 0 (x), 1 (y) or 2 (z), counter-
    /// clockwise looking down the axis (`GfRotation` for row vectors).
    pub fn rotation(axis: usize, degrees: f64) -> Matrix4d {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let mut m = Matrix4d::IDENTITY;
        let (a, b) = match axis {
            0 => (1, 2),
            1 => (2, 0),
            _ => (0, 1),
        };
        m.0[a * 4 + a] = cos;
        m.0[a * 4 + b] = sin;
        m.0[b * 4 + a] = -sin;
        m.0[b * 4 + b] = cos;
        m
    }

    /// The rotation of the unit quaternion `real + (i, j, k)` (normalized
    /// first, as `GfMatrix4d::SetRotate` does).
    pub fn quaternion(real: f64, i: f64, j: f64, k: f64) -> Matrix4d {
        let length = (real * real + i * i + j * j + k * k).sqrt();
        if length == 0.0 || !length.is_finite() {
            return Matrix4d::IDENTITY;
        }
        let (r, x, y, z) = (real / length, i / length, j / length, k / length);
        Matrix4d([
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + z * r),
            2.0 * (z * x - y * r),
            0.0,
            2.0 * (x * y - z * r),
            1.0 - 2.0 * (z * z + x * x),
            2.0 * (y * z + x * r),
            0.0,
            2.0 * (z * x + y * r),
            2.0 * (y * z - x * r),
            1.0 - 2.0 * (y * y + x * x),
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ])
    }

    /// `self * other`: `self` applies first to row vectors.
    pub fn times(&self, other: &Matrix4d) -> Matrix4d {
        let (a, b) = (&self.0, &other.0);
        let mut out = [0.0; 16];
        for row in 0..4 {
            for column in 0..4 {
                out[row * 4 + column] = (0..4).map(|k| a[row * 4 + k] * b[k * 4 + column]).sum();
            }
        }
        Matrix4d(out)
    }

    /// The point `p` transformed (`p * M`, w = 1, projective divide skipped
    /// for affine matrices).
    pub fn transform_point(&self, p: [f64; 3]) -> [f64; 3] {
        let m = &self.0;
        [
            p[0] * m[0] + p[1] * m[4] + p[2] * m[8] + m[12],
            p[0] * m[1] + p[1] * m[5] + p[2] * m[9] + m[13],
            p[0] * m[2] + p[1] * m[6] + p[2] * m[10] + m[14],
        ]
    }

    /// The direction `v` transformed by the upper 3x3.
    pub fn transform_direction(&self, v: [f64; 3]) -> [f64; 3] {
        let m = &self.0;
        [
            v[0] * m[0] + v[1] * m[4] + v[2] * m[8],
            v[0] * m[1] + v[1] * m[5] + v[2] * m[9],
            v[0] * m[2] + v[1] * m[6] + v[2] * m[10],
        ]
    }

    /// The determinant of the upper 3x3 (negative for a mirroring transform).
    pub fn determinant3(&self) -> f64 {
        let m = &self.0;
        m[0] * (m[5] * m[10] - m[6] * m[9]) - m[1] * (m[4] * m[10] - m[6] * m[8])
            + m[2] * (m[4] * m[9] - m[5] * m[8])
    }

    /// The inverse; none when the matrix is singular.
    pub fn inverse(&self) -> Option<Matrix4d> {
        let m = &self.0;
        let mut inv = [0.0; 16];
        inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15]
            + m[9] * m[7] * m[14]
            + m[13] * m[6] * m[11]
            - m[13] * m[7] * m[10];
        inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15]
            - m[8] * m[7] * m[14]
            - m[12] * m[6] * m[11]
            + m[12] * m[7] * m[10];
        inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15]
            + m[8] * m[7] * m[13]
            + m[12] * m[5] * m[11]
            - m[12] * m[7] * m[9];
        inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14]
            - m[8] * m[6] * m[13]
            - m[12] * m[5] * m[10]
            + m[12] * m[6] * m[9];
        inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15]
            - m[9] * m[3] * m[14]
            - m[13] * m[2] * m[11]
            + m[13] * m[3] * m[10];
        inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15]
            + m[8] * m[3] * m[14]
            + m[12] * m[2] * m[11]
            - m[12] * m[3] * m[10];
        inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15]
            - m[8] * m[3] * m[13]
            - m[12] * m[1] * m[11]
            + m[12] * m[3] * m[9];
        inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14]
            + m[8] * m[2] * m[13]
            + m[12] * m[1] * m[10]
            - m[12] * m[2] * m[9];
        inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15]
            + m[5] * m[3] * m[14]
            + m[13] * m[2] * m[7]
            - m[13] * m[3] * m[6];
        inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15]
            - m[4] * m[3] * m[14]
            - m[12] * m[2] * m[7]
            + m[12] * m[3] * m[6];
        inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15]
            + m[4] * m[3] * m[13]
            + m[12] * m[1] * m[7]
            - m[12] * m[3] * m[5];
        inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14]
            - m[4] * m[2] * m[13]
            - m[12] * m[1] * m[6]
            + m[12] * m[2] * m[5];
        inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11]
            - m[5] * m[3] * m[10]
            - m[9] * m[2] * m[7]
            + m[9] * m[3] * m[6];
        inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11]
            + m[4] * m[3] * m[10]
            + m[8] * m[2] * m[7]
            - m[8] * m[3] * m[6];
        inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11]
            - m[4] * m[3] * m[9]
            - m[8] * m[1] * m[7]
            + m[8] * m[3] * m[5];
        inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10]
            + m[4] * m[2] * m[9]
            + m[8] * m[1] * m[6]
            - m[8] * m[2] * m[5];
        let determinant = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
        if determinant == 0.0 || !determinant.is_finite() {
            return None;
        }
        Some(Matrix4d(inv.map(|value| value / determinant)))
    }

    /// Whether this is the identity.
    pub fn is_identity(&self) -> bool {
        *self == Matrix4d::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::Matrix4d;

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-12)
    }

    #[test]
    fn row_vector_convention() {
        // Scale, then translate: p * S * T.
        let m = Matrix4d::scaling(2.0, 2.0, 2.0).times(&Matrix4d::translation(1.0, 0.0, 0.0));
        assert!(close(m.transform_point([1.0, 1.0, 1.0]), [3.0, 2.0, 2.0]));
        // 90 degrees about z takes x to y.
        assert!(close(
            Matrix4d::rotation(2, 90.0).transform_point([1.0, 0.0, 0.0]),
            [0.0, 1.0, 0.0]
        ));
        assert!(close(
            Matrix4d::rotation(0, 90.0).transform_point([0.0, 1.0, 0.0]),
            [0.0, 0.0, 1.0]
        ));
        assert!(close(
            Matrix4d::rotation(1, 90.0).transform_point([0.0, 0.0, 1.0]),
            [1.0, 0.0, 0.0]
        ));
        // The quaternion for 90 degrees about z matches.
        let half = std::f64::consts::FRAC_PI_4;
        let q = Matrix4d::quaternion(half.cos(), 0.0, 0.0, half.sin());
        assert!(close(q.transform_point([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
    }

    #[test]
    fn inverse_undoes() {
        let m = Matrix4d::rotation(1, 30.0)
            .times(&Matrix4d::scaling(2.0, 3.0, 4.0))
            .times(&Matrix4d::translation(5.0, -1.0, 2.0));
        let back = m.times(&m.inverse().unwrap());
        assert!(
            back.0
                .iter()
                .zip(Matrix4d::IDENTITY.0)
                .all(|(a, b)| (a - b).abs() < 1e-12)
        );
        assert!(Matrix4d::scaling(0.0, 1.0, 1.0).inverse().is_none());
        assert!(Matrix4d::scaling(-1.0, 1.0, 1.0).determinant3() < 0.0);
    }
}
