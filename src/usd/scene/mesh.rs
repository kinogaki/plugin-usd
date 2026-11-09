//! UsdGeomMesh prims as authored: points, topology, orientation,
//! primvars, subdivision tags and GeomSubsets, the arrays shared with the
//! layer. A mesh whose topology doesn't hold together is refused with the
//! reason.

use std::sync::Arc;

use super::primvar::{Interpolation, Primvar, read_primvars};
use super::values::{Time, ints, reals, token_at, value_at, vec3fs};
use crate::usd::sdf::{LayerData, Path, Token, Value, field};

/// One Mesh prim.
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    /// Points in the prim's own space.
    pub points: Arc<Vec<[f32; 3]>>,
    /// Vertices per face.
    pub face_vertex_counts: Arc<Vec<i32>>,
    /// Each face's points, face after face.
    pub face_vertex_indices: Arc<Vec<i32>>,
    /// `orientation = "leftHanded"`: faces wind clockwise.
    pub left_handed: bool,
    /// Primvars in name order, `normals` included.
    pub primvars: Vec<Primvar>,
    /// Subdivision tags.
    pub subdivision: Subdivision,
    /// GeomSubsets.
    pub subsets: Vec<Subset>,
    /// `doubleSided`.
    pub double_sided: bool,
}

impl Mesh {
    /// Values a primvar of `interpolation` holds on this mesh.
    pub fn elements(&self, interpolation: Interpolation) -> usize {
        match interpolation {
            Interpolation::Constant => 1,
            Interpolation::Uniform => self.face_vertex_counts.len(),
            Interpolation::Vertex | Interpolation::Varying => self.points.len(),
            Interpolation::FaceVarying => self.face_vertex_indices.len(),
        }
    }

    /// The primvar `name`.
    pub fn primvar(&self, name: &str) -> Option<&Primvar> {
        self.primvars.iter().find(|p| p.name.as_str() == name)
    }
}

/// Kinogaki's attribute holding how many times a subdivision mesh is
/// refined (USD has none: a refinement level is a renderer's).
pub const LEVEL: &str = "kinogaki:subdivisionLevel";

/// The attributes holding a mesh's refinement level, in the order they are
/// read, the first authored one deciding: ours, Arnold's (and its older
/// spelling), Omniverse's (only with `refinementEnableOverride` on).
pub const LEVEL_READ: [&str; 4] = [
    LEVEL,
    "primvars:arnold:subdiv_iterations",
    "arnold:subdiv_iterations",
    OMNIVERSE_LEVEL,
];

/// Omniverse's refinement level, read where its override is on.
const OMNIVERSE_LEVEL: &str = "refinementLevel";

/// The primvar a level is read from too, taken in rather than kept as a
/// primvar.
const ARNOLD_PRIMVAR: &str = "arnold:subdiv_iterations";

/// Subdivision tags (`subdivisionScheme`, its options, creases, corners,
/// holes) and the refinement level.
#[derive(Clone, Debug, PartialEq)]
pub struct Subdivision {
    /// `catmullClark` (USD's fallback), `loop`, `bilinear` or `none`.
    pub scheme: Token,
    /// How many times it is refined, when an attribute says: ours, Arnold's
    /// or Omniverse's (`LEVEL_READ`).
    pub level: Option<i32>,
    /// `interpolateBoundary` (`edgeAndCorner` by default).
    pub interpolate_boundary: Token,
    /// `faceVaryingLinearInterpolation` (`cornersPlus1` by default).
    pub face_varying: Token,
    /// `triangleSubdivisionRule` (`catmullClark` by default).
    pub triangle_rule: Token,
    /// Point indices of crease chains, `crease_lengths` points each.
    pub crease_indices: Vec<i32>,
    /// Points per crease chain.
    pub crease_lengths: Vec<i32>,
    /// One sharpness per chain, or one per chain edge.
    pub crease_sharpnesses: Vec<f64>,
    /// Sharp corner points.
    pub corner_indices: Vec<i32>,
    /// One sharpness per corner.
    pub corner_sharpnesses: Vec<f64>,
    /// Faces that are holes.
    pub hole_indices: Vec<i32>,
}

impl Default for Subdivision {
    /// No subdivision: a polygon mesh as authored.
    fn default() -> Subdivision {
        Subdivision {
            scheme: Token::new("none"),
            level: None,
            interpolate_boundary: Token::new("edgeAndCorner"),
            face_varying: Token::new("cornersPlus1"),
            triangle_rule: Token::new("catmullClark"),
            crease_indices: Vec::new(),
            crease_lengths: Vec::new(),
            crease_sharpnesses: Vec::new(),
            corner_indices: Vec::new(),
            corner_sharpnesses: Vec::new(),
            hole_indices: Vec::new(),
        }
    }
}

/// What a GeomSubset's indices name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubsetElement {
    /// Faces.
    Face,
    /// Points.
    Point,
    /// Edges, as point index pairs.
    Edge,
}

/// A GeomSubset of a mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct Subset {
    /// The subset prim's name.
    pub name: Token,
    /// What the indices name.
    pub element: SubsetElement,
    /// The elements.
    pub indices: Arc<Vec<i32>>,
    /// `familyName` (`materialBind` for per-face materials).
    pub family: Token,
    /// The material bound to the subset, an index into the scene's
    /// materials.
    pub material: Option<usize>,
}

/// Read the Mesh at `prim`; `Err` says why it can't be read. Primvars and
/// GeomSubsets that don't fit are left out, each said in `warnings`.
pub(super) fn read(
    layer: &LayerData,
    prim: &Path,
    time: Time,
    warnings: &mut Vec<String>,
) -> Result<Mesh, String> {
    let get = |name: &str| value_at(layer, prim, name, time);
    let points = get("points").as_ref().and_then(vec3fs);
    let counts = get("faceVertexCounts").as_ref().and_then(ints);
    let indices = get("faceVertexIndices").as_ref().and_then(ints);
    let (Some(points), Some(counts), Some(indices)) = (points, counts, indices) else {
        return Err(
            "points, faceVertexCounts or faceVertexIndices missing or of the wrong type".into(),
        );
    };
    let total: i64 = counts.iter().map(|&count| i64::from(count)).sum();
    if counts.iter().any(|&count| count < 0) || total != indices.len() as i64 {
        return Err("faceVertexCounts do not add up to faceVertexIndices".into());
    }
    let point_count = points.len();
    if indices
        .iter()
        .any(|&index| index < 0 || index as usize >= point_count)
    {
        return Err("faceVertexIndices name points that do not exist".into());
    }
    let mut mesh = Mesh {
        points,
        face_vertex_counts: counts,
        face_vertex_indices: indices,
        left_handed: token_at(layer, prim, "orientation", "rightHanded").as_str() == "leftHanded",
        primvars: Vec::new(),
        subdivision: subdivision(layer, prim, time),
        subsets: Vec::new(),
        double_sided: (value_at(layer, prim, "doubleSided", Time::Default))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    };
    let mut primvars = read_primvars(layer, prim, &["normals", "velocities"], time);
    primvars.retain(|primvar| {
        let fits = primvar.fits(mesh.elements(primvar.interpolation));
        if !fits {
            warnings.push(format!(
                "{prim}: primvar {} does not fit the mesh; left out",
                primvar.name
            ));
        }
        fits
    });
    // Arnold's level is the mesh's level, not one of its primvars.
    primvars.retain(|primvar| primvar.name.as_str() != ARNOLD_PRIMVAR);
    mesh.primvars = primvars;
    mesh.subsets = subsets(layer, prim, &mesh, time, warnings);
    Ok(mesh)
}

/// The subdivision tags of the Mesh at `prim`.
fn subdivision(layer: &LayerData, prim: &Path, time: Time) -> Subdivision {
    let integers = |name: &str| {
        (value_at(layer, prim, name, time).as_ref())
            .and_then(ints)
            .map(|v| v.to_vec())
            .unwrap_or_default()
    };
    let numbers = |name: &str| {
        (value_at(layer, prim, name, time).as_ref())
            .and_then(reals)
            .map(|(v, _)| v)
            .unwrap_or_default()
    };
    let on = |name: &str| (value_at(layer, prim, name, time)).and_then(|v| v.as_bool());
    let level = (LEVEL_READ.iter())
        .filter(|&&name| name != OMNIVERSE_LEVEL || on("refinementEnableOverride") == Some(true))
        .find_map(|name| value_at(layer, prim, name, time)?.as_f64())
        .map(|level| level.round().clamp(0.0, f64::from(i32::MAX)) as i32);
    Subdivision {
        scheme: token_at(layer, prim, "subdivisionScheme", "catmullClark"),
        level,
        interpolate_boundary: token_at(layer, prim, "interpolateBoundary", "edgeAndCorner"),
        face_varying: token_at(
            layer,
            prim,
            "faceVaryingLinearInterpolation",
            "cornersPlus1",
        ),
        triangle_rule: token_at(layer, prim, "triangleSubdivisionRule", "catmullClark"),
        crease_indices: integers("creaseIndices"),
        crease_lengths: integers("creaseLengths"),
        crease_sharpnesses: numbers("creaseSharpnesses"),
        corner_indices: integers("cornerIndices"),
        corner_sharpnesses: numbers("cornerSharpnesses"),
        hole_indices: integers("holeIndices"),
    }
}

/// The GeomSubset children of the Mesh at `prim`, their bindings left to
/// the caller.
fn subsets(
    layer: &LayerData,
    prim: &Path,
    mesh: &Mesh,
    time: Time,
    warnings: &mut Vec<String>,
) -> Vec<Subset> {
    let mut subsets = Vec::new();
    for name in layer
        .spec(prim)
        .map_or(&[][..], |spec| spec.tokens(field::PRIM_CHILDREN))
    {
        let child = prim.child(name.clone());
        let type_name = layer
            .get(&child, field::TYPE_NAME)
            .and_then(Value::as_token);
        if type_name.map(|t| t.as_str()) != Some("GeomSubset") {
            continue;
        }
        let element = match token_at(layer, &child, "elementType", "face").as_str() {
            "point" => SubsetElement::Point,
            "edge" => SubsetElement::Edge,
            _ => SubsetElement::Face,
        };
        let indices = (value_at(layer, &child, "indices", time).as_ref())
            .and_then(ints)
            .unwrap_or_default();
        let limit = match element {
            SubsetElement::Face => mesh.face_vertex_counts.len(),
            SubsetElement::Point | SubsetElement::Edge => mesh.points.len(),
        };
        if indices
            .iter()
            .any(|&index| index < 0 || index as usize >= limit)
        {
            warnings.push(format!(
                "{prim}: GeomSubset {name} has indices outside the mesh; left out"
            ));
            continue;
        }
        subsets.push(Subset {
            name: name.clone(),
            element,
            indices,
            family: token_at(layer, &child, "familyName", ""),
            material: None,
        });
    }
    subsets
}
