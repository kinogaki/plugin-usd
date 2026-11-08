//! UsdGeomPointInstancer prims: placements of prototypes, after
//! `UsdGeomPointInstancer::ComputeInstanceTransformsAtTime` (scale, then
//! orientation, then position; the prototype's own transform first).

use std::sync::Arc;

use super::matrix::Matrix4d;
use super::primvar::{Primvar, read_primvars};
use super::values::{Time, int64s, ints, value_at, vec3fs};
use crate::usd::sdf::{Array, LayerData, Path, Value};

/// One PointInstancer: instance `i` places prototype
/// `prototypes[proto_indices[i]]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Instancer {
    /// The prototypes, indices into the scene's, in the order of the
    /// `prototypes` relationship's targets.
    pub prototypes: Vec<usize>,
    /// `protoIndices`, one per instance.
    pub proto_indices: Arc<Vec<i32>>,
    /// `positions`, one per instance.
    pub positions: Arc<Vec<[f32; 3]>>,
    /// `orientationsf` (else `orientations`) as `i, j, k, real`, one per
    /// instance, when authored; written as both.
    pub orientations: Option<Arc<Vec<[f32; 4]>>>,
    /// `scales`, one per instance, when authored.
    pub scales: Option<Arc<Vec<[f32; 3]>>>,
    /// `ids`, one per instance, when authored.
    pub ids: Option<Arc<Vec<i64>>>,
    /// `invisibleIds`: instances hidden, by id (by index without `ids`).
    pub invisible_ids: Vec<i64>,
    /// `inactiveIds` metadata: instances left out, by id (by index).
    pub inactive_ids: Vec<i64>,
    /// Per-instance primvars in name order, `velocities` among them.
    pub primvars: Vec<Primvar>,
}

impl Instancer {
    /// Instances.
    pub fn len(&self) -> usize {
        self.proto_indices.len()
    }

    /// Whether it places nothing.
    pub fn is_empty(&self) -> bool {
        self.proto_indices.is_empty()
    }

    /// Each instance's placement relative to the instancer (row vectors):
    /// scale, then orientation, then position.
    pub fn transforms(&self) -> Vec<Matrix4d> {
        (0..self.len())
            .map(|i| {
                let orientation = self.orientations.as_ref().map(|o| o[i]);
                let scale = self.scales.as_ref().map(|s| s[i]);
                transform(self.positions[i], orientation, scale)
            })
            .collect()
    }

    /// `m` as a point instancer places an instance: its position, orientation
    /// (`i, j, k, real`) and scale, which give `m` back to single precision;
    /// none for a placement they can't hold (a shear, a scale of zero, a
    /// projection).
    pub fn placement(m: &Matrix4d) -> Option<([f32; 3], [f32; 4], [f32; 3])> {
        let a = &m.0;
        if [a[3], a[7], a[11], a[15]] != [0.0, 0.0, 0.0, 1.0] {
            return None;
        }
        // The rows are the scale times the rotation's rows (row vectors).
        let mut rows = [0, 4, 8].map(|at| [a[at], a[at + 1], a[at + 2]]);
        let dot = |u: [f64; 3], v: [f64; 3]| u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
        let mut scale = rows.map(|r| dot(r, r).sqrt());
        if scale.iter().any(|s| !s.is_normal()) {
            return None;
        }
        for (row, s) in rows.iter_mut().zip(scale) {
            *row = row.map(|c| c / s);
        }
        let [x, y, z] = rows;
        if [dot(x, y), dot(y, z), dot(z, x)]
            .iter()
            .any(|d| d.abs() > 1e-6)
        {
            return None;
        }
        let cross = [
            x[1] * y[2] - x[2] * y[1],
            x[2] * y[0] - x[0] * y[2],
            x[0] * y[1] - x[1] * y[0],
        ];
        if dot(cross, z) < 0.0 {
            scale[0] = -scale[0];
            rows[0] = x.map(|c| -c);
        }
        let orientation = quaternion(&rows).map(|c| c as f32);
        let position = [a[12], a[13], a[14]].map(|c| c as f32);
        let scale = scale.map(|c| c as f32);
        // Single precision's error, the turn's against its size and the
        // position's against its own.
        let back = transform(position, Some(orientation), Some(scale)).0;
        let size = (a[..12].iter()).fold(0.0_f64, |most, c| most.max(c.abs()));
        let close = (0..15).all(|k| match k {
            12.. => (back[k] - a[k]).abs() <= 1e-6 * (1.0 + a[k].abs()),
            _ => (back[k] - a[k]).abs() <= 1e-5 * size,
        });
        close.then_some((position, orientation, scale))
    }

    /// Whether each instance shows: neither invisible nor inactive.
    pub fn shown(&self) -> Vec<bool> {
        let id = |i: usize| self.ids.as_ref().map_or(i as i64, |ids| ids[i]);
        (0..self.len())
            .map(|i| {
                let id = id(i);
                !self.invisible_ids.contains(&id) && !self.inactive_ids.contains(&id)
            })
            .collect()
    }
}

/// One instance's placement: `scale`, then `orientation` (`i, j, k,
/// real`), then `position`.
fn transform(
    position: [f32; 3],
    orientation: Option<[f32; 4]>,
    scale: Option<[f32; 3]>,
) -> Matrix4d {
    let mut m = Matrix4d::IDENTITY;
    if let Some([x, y, z]) = scale.map(|s| s.map(f64::from)) {
        m = Matrix4d::scaling(x, y, z);
    }
    if let Some([qi, qj, qk, real]) = orientation.map(|o| o.map(f64::from)) {
        m = m.times(&Matrix4d::quaternion(real, qi, qj, qk));
    }
    let [x, y, z] = position.map(f64::from);
    m.times(&Matrix4d::translation(x, y, z))
}

/// The unit quaternion (`i, j, k, real`) of the rotation whose rows (row
/// vectors) are `r`, the inverse of [`Matrix4d::quaternion`].
fn quaternion(r: &[[f64; 3]; 3]) -> [f64; 4] {
    let trace = r[0][0] + r[1][1] + r[2][2];
    if trace > 0.0 {
        let s = 2.0 * (trace + 1.0).sqrt();
        let i = (r[1][2] - r[2][1]) / s;
        let j = (r[2][0] - r[0][2]) / s;
        let k = (r[0][1] - r[1][0]) / s;
        [i, j, k, s / 4.0]
    } else if r[0][0] >= r[1][1] && r[0][0] >= r[2][2] {
        let s = 2.0 * (1.0 + r[0][0] - r[1][1] - r[2][2]).sqrt();
        let real = (r[1][2] - r[2][1]) / s;
        [
            s / 4.0,
            (r[1][0] + r[0][1]) / s,
            (r[2][0] + r[0][2]) / s,
            real,
        ]
    } else if r[1][1] >= r[2][2] {
        let s = 2.0 * (1.0 + r[1][1] - r[0][0] - r[2][2]).sqrt();
        let real = (r[2][0] - r[0][2]) / s;
        [
            (r[1][0] + r[0][1]) / s,
            s / 4.0,
            (r[2][1] + r[1][2]) / s,
            real,
        ]
    } else {
        let s = 2.0 * (1.0 + r[2][2] - r[0][0] - r[1][1]).sqrt();
        let real = (r[0][1] - r[1][0]) / s;
        [
            (r[2][0] + r[0][2]) / s,
            (r[2][1] + r[1][2]) / s,
            s / 4.0,
            real,
        ]
    }
}

/// Read the PointInstancer at `prim` with its prototypes already resolved
/// to `prototypes`; `Err` says why it can't be read.
pub(super) fn read(
    layer: &LayerData,
    prim: &Path,
    prototypes: Vec<usize>,
    time: Time,
) -> Result<Instancer, String> {
    let get = |name: &str| value_at(layer, prim, name, time);
    let proto_indices = get("protoIndices")
        .as_ref()
        .and_then(ints)
        .unwrap_or_default();
    let count = proto_indices.len();
    if proto_indices
        .iter()
        .any(|&p| p < 0 || p as usize >= prototypes.len())
    {
        return Err("protoIndices name prototypes that do not exist".into());
    }
    let sized = |name: &str, column: Option<usize>| match column {
        Some(length) if length != count => Err(format!("{name} need one value per instance")),
        _ => Ok(()),
    };
    let positions = get("positions")
        .as_ref()
        .and_then(vec3fs)
        .unwrap_or_default();
    sized("positions", Some(positions.len()))?;
    // `orientationsf`, when authored, over the half-precision ones.
    let orientations = (get("orientationsf").as_ref().and_then(quaternions))
        .filter(|o| !o.is_empty())
        .or_else(|| get("orientations").as_ref().and_then(quaternions));
    sized("orientations", orientations.as_ref().map(|o| o.len()))?;
    let scales = get("scales").as_ref().and_then(vec3fs);
    sized("scales", scales.as_ref().map(|s| s.len()))?;
    let ids = get("ids").as_ref().and_then(int64s);
    sized("ids", ids.as_ref().map(|i| i.len()))?;
    let invisible_ids = get("invisibleIds")
        .as_ref()
        .and_then(int64s)
        .map(|i| i.to_vec());
    let inactive_ids = match layer.get(prim, "inactiveIds") {
        Some(Value::Int64ListOp(list)) => list.applied(),
        _ => Vec::new(),
    };
    Ok(Instancer {
        prototypes,
        proto_indices,
        positions,
        orientations,
        scales,
        ids,
        invisible_ids: invisible_ids.unwrap_or_default(),
        inactive_ids,
        primvars: read_primvars(layer, prim, &["velocities"], time),
    })
}

/// Quaternions (`quath[]`, `quatf[]`, `quatd[]`) as f32 `i, j, k, real`.
fn quaternions(value: &Value) -> Option<Arc<Vec<[f32; 4]>>> {
    Some(match value.as_array()? {
        Array::Quatf(items) => items.clone(),
        Array::Quath(items) => Arc::new(items.iter().map(|q| q.map(|c| c.to_f32())).collect()),
        Array::Quatd(items) => Arc::new(items.iter().map(|q| q.map(|c| c as f32)).collect()),
        _ => return None,
    })
}
