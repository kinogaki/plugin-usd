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
    /// `orientations` (or `orientationsf`) as `i, j, k, real`, one per
    /// instance, when authored.
    pub orientations: Option<Arc<Vec<[f32; 4]>>>,
    /// `scales`, one per instance, when authored.
    pub scales: Option<Arc<Vec<[f32; 3]>>>,
    /// `ids`, one per instance, when authored.
    pub ids: Option<Arc<Vec<i64>>>,
    /// `invisibleIds`: instances hidden, by id (by index without `ids`).
    pub invisible_ids: Vec<i64>,
    /// `inactiveIds` metadata: instances left out, by id (by index).
    pub inactive_ids: Vec<i64>,
    /// Per-instance primvars in name order.
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
                let mut m = Matrix4d::IDENTITY;
                if let Some(scales) = &self.scales {
                    let [x, y, z] = scales[i].map(f64::from);
                    m = Matrix4d::scaling(x, y, z);
                }
                if let Some(orientations) = &self.orientations {
                    let [qi, qj, qk, real] = orientations[i].map(f64::from);
                    m = m.times(&Matrix4d::quaternion(real, qi, qj, qk));
                }
                let [x, y, z] = self.positions[i].map(f64::from);
                m.times(&Matrix4d::translation(x, y, z))
            })
            .collect()
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
    let orientations = get("orientations")
        .or_else(|| get("orientationsf"))
        .as_ref()
        .and_then(quaternions);
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
        primvars: read_primvars(layer, prim, &[], time),
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
