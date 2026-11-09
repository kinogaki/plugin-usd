//! A mesh's subdivision between USD's tags and Studio's standard
//! attributes: `subdivisionScheme` and its options as the
//! [`Subdivision`] setting (`none`, no setting), the refinement level as
//! `kinogaki:subdivisionLevel` (read from Arnold's and Omniverse's too,
//! `usd::scene::mesh::LEVEL_READ`), creases on the edges' `crease`, corners
//! on the points' `crease`, holes as the `subdivision_hole` faces.
use crate::usd::scene::Subdivision as Tags;
use crate::usd::sdf::Token;
use kinogaki_studio_api::Error;
use kinogaki_studio_api::geometry::{
    Attribute, Attributes, Boundary, Domain, Elements, FaceVarying, Geometry, Meaning, Primitives,
    Scheme, Subdivision, Triangles, names,
};

/// Each [`Scheme`]'s token, and none's.
const SCHEMES: [(Option<Scheme>, &str); 4] = [
    (Some(Scheme::CatmullClark), "catmullClark"),
    (Some(Scheme::Loop), "loop"),
    (Some(Scheme::Bilinear), "bilinear"),
    (None, "none"),
];
const BOUNDARIES: [(Boundary, &str); 3] = [
    (Boundary::EdgeAndCorner, "edgeAndCorner"),
    (Boundary::EdgeOnly, "edgeOnly"),
    (Boundary::None, "none"),
];
const FACE_VARYING: [(FaceVarying, &str); 6] = [
    (FaceVarying::None, "none"),
    (FaceVarying::CornersOnly, "cornersOnly"),
    (FaceVarying::CornersPlus1, "cornersPlus1"),
    (FaceVarying::CornersPlus2, "cornersPlus2"),
    (FaceVarying::Boundaries, "boundaries"),
    (FaceVarying::All, "all"),
];
const TRIANGLES: [(Triangles, &str); 2] = [
    (Triangles::CatmullClark, "catmullClark"),
    (Triangles::Smooth, "smooth"),
];

/// The value `token` names in `table`.
fn named<T: Copy>(table: &[(T, &str)], token: &Token) -> Option<T> {
    (table.iter())
        .find(|(_, t)| *t == token.as_str())
        .map(|(v, _)| *v)
}

/// The token `value` is in `table`.
fn token<T: PartialEq>(table: &[(T, &'static str)], value: T) -> Token {
    let found = table.iter().find(|(v, _)| *v == value);
    Token::new(found.map_or(table[0].1, |(_, t)| t))
}

/// `attributes` of a mesh over `table` with what `tags` say: the setting,
/// creases, corners, and the faces of the file `holes` names among `kept`
/// (the file's faces Studio holds, in order). What doesn't fit the mesh is
/// said in `warnings` and left out.
///
/// # Errors
/// What the attributes refuse.
pub(crate) fn read(
    tags: &Tags,
    table: &Primitives,
    kept: &[u32],
    mut attributes: Attributes,
    warnings: &mut Vec<String>,
) -> Result<Attributes, Error> {
    let scheme = named(&SCHEMES, &tags.scheme).unwrap_or_else(|| {
        warnings.push(format!(
            "subdivisionScheme {} unknown; read as none",
            tags.scheme
        ));
        None
    });
    if let Some(scheme) = scheme {
        let level = tags
            .level
            .map_or(Subdivision::DEFAULT_LEVEL, |l| l.max(0) as u32);
        let setting = Subdivision {
            boundary: named(&BOUNDARIES, &tags.interpolate_boundary)
                .unwrap_or(Boundary::EdgeAndCorner),
            face_varying: named(&FACE_VARYING, &tags.face_varying)
                .unwrap_or(FaceVarying::CornersPlus1),
            triangles: named(&TRIANGLES, &tags.triangle_rule).unwrap_or(Triangles::CatmullClark),
            ..Subdivision::new(scheme).with_level(level)
        };
        attributes = setting.write(&attributes)?;
    }
    if let Some(crease) = creases(tags, table, warnings) {
        let column = Attribute::tuples(crease, Meaning::None)?;
        attributes = attributes.with(Domain::Edge, names::CREASE, column)?;
    }
    if let Some(corner) = corners(tags, table.points(), warnings) {
        let column = Attribute::tuples(corner, Meaning::None)?;
        attributes = attributes.with(Domain::Point, names::CREASE, column)?;
    }
    let holes = (tags.hole_indices.iter())
        .filter_map(|&f| kept.binary_search(&(f as u32)).ok().map(|k| k as u32));
    let holes = Elements::from_indices(kept.len(), holes)?;
    if !holes.is_empty() {
        attributes = attributes.with_group(Domain::Primitive, names::SUBDIVISION_HOLE, holes)?;
    }
    Ok(attributes)
}

/// Each edge's sharpness from the crease chains, the highest where chains
/// meet; `None` with no crease on the mesh.
fn creases(tags: &Tags, table: &Primitives, warnings: &mut Vec<String>) -> Option<Vec<f32>> {
    let lengths = &tags.crease_lengths;
    if lengths.is_empty() {
        return None;
    }
    let total: i64 = lengths.iter().map(|&n| i64::from(n)).sum();
    let edges: i64 = lengths.iter().map(|&n| i64::from(n) - 1).sum();
    let sharp = &tags.crease_sharpnesses;
    let per_chain = sharp.len() == lengths.len();
    let fits = lengths.iter().all(|&n| n >= 2)
        && total == tags.crease_indices.len() as i64
        && (per_chain || sharp.len() as i64 == edges);
    if !fits {
        warnings.push("creases whose counts don't add up; left out".into());
        return None;
    }
    let mut out = vec![0.0f32; table.edge_count()];
    let (mut start, mut edge) = (0usize, 0usize);
    let mut missing = 0;
    for (c, &n) in lengths.iter().enumerate() {
        let chain = &tags.crease_indices[start..start + n as usize];
        for pair in chain.windows(2) {
            let at = |p: i32| {
                u32::try_from(p)
                    .ok()
                    .filter(|&p| (p as usize) < table.points())
            };
            let found = at(pair[0])
                .zip(at(pair[1]))
                .and_then(|(a, b)| table.edge_of(a, b));
            let s = (if per_chain { sharp[c] } else { sharp[edge] }) as f32;
            match found {
                Some(e) => out[e as usize] = out[e as usize].max(s),
                None => missing += 1,
            }
            edge += 1;
        }
        start += n as usize;
    }
    if missing > 0 {
        warnings.push(format!(
            "{missing} crease edges the mesh doesn't have; left out"
        ));
    }
    out.iter().any(|&s| s > 0.0).then_some(out)
}

/// Each point's corner sharpness; `None` with no corner.
fn corners(tags: &Tags, points: usize, warnings: &mut Vec<String>) -> Option<Vec<f32>> {
    let (indices, sharp) = (&tags.corner_indices, &tags.corner_sharpnesses);
    if indices.is_empty() {
        return None;
    }
    if indices.len() != sharp.len() || indices.iter().any(|&p| p < 0 || p as usize >= points) {
        warnings.push("corners that don't fit the mesh; left out".into());
        return None;
    }
    let mut out = vec![0.0f32; points];
    for (&p, &s) in indices.iter().zip(sharp) {
        out[p as usize] = out[p as usize].max(s as f32);
    }
    out.iter().any(|&s| s > 0.0).then_some(out)
}

/// The tags of the mesh written of `value`'s `polygons` (its faces, in
/// order, over all its points).
pub(crate) fn write(value: &Geometry, polygons: &[usize]) -> Tags {
    let attributes = value.attributes();
    let table = value.primitives();
    let setting = Subdivision::of(value);
    // A mesh without a scheme keeps the options a file brought.
    let option = |name: &str| {
        let text = attributes.text(Domain::Detail, name)?;
        text.get(0).map(str::to_owned)
    };
    let boundary = setting
        .map(|s| s.boundary)
        .or_else(|| option(names::SUBDIVISION_BOUNDARY).and_then(|k| Boundary::parse(&k)));
    let face_varying = setting
        .map(|s| s.face_varying)
        .or_else(|| option(names::SUBDIVISION_FACE_VARYING).and_then(|k| FaceVarying::parse(&k)));
    let triangles = setting
        .map(|s| s.triangles)
        .or_else(|| option(names::SUBDIVISION_TRIANGLES).and_then(|k| Triangles::parse(&k)));
    let mut tags = Tags {
        scheme: token(&SCHEMES, setting.map(|s| s.scheme)),
        level: setting.map(|s| s.level as i32),
        interpolate_boundary: token(&BOUNDARIES, boundary.unwrap_or(Boundary::EdgeAndCorner)),
        face_varying: token(
            &FACE_VARYING,
            face_varying.unwrap_or(FaceVarying::CornersPlus1),
        ),
        triangle_rule: token(&TRIANGLES, triangles.unwrap_or(Triangles::CatmullClark)),
        ..Tags::default()
    };
    if let Some(crease) = attributes.tuples::<f32>(Domain::Edge, names::CREASE) {
        let index = table.index();
        let on_faces =
            |e: usize| (index.edge_faces(e).iter()).any(|&f| table.kind(f as usize).is_polygon());
        let creased = crease
            .iter()
            .enumerate()
            .filter(|&(e, s)| *s > 0.0 && on_faces(e));
        for (e, &s) in creased {
            let [a, b] = table.edge(e).points;
            tags.crease_indices.extend([a as i32, b as i32]);
            tags.crease_lengths.push(2);
            tags.crease_sharpnesses.push(f64::from(s));
        }
    }
    if let Some(corner) = attributes.tuples::<f32>(Domain::Point, names::CREASE) {
        for (p, &s) in corner.iter().enumerate().filter(|(_, s)| **s > 0.0) {
            tags.corner_indices.push(p as i32);
            tags.corner_sharpnesses.push(f64::from(s));
        }
    }
    if let Some(holes) = attributes.group(Domain::Primitive, names::SUBDIVISION_HOLE) {
        let at = |f: u32| polygons.binary_search(&(f as usize)).ok();
        tags.hole_indices = holes.iter().filter_map(at).map(|k| k as i32).collect();
    }
    tags
}

/// Whether `name` on `domain` is one this module reads and writes as tags,
/// never as a primvar or a GeomSubset.
pub(crate) fn is_tag(domain: Domain, name: &str) -> bool {
    match domain {
        Domain::Detail => matches!(
            name,
            names::SUBDIVISION_SCHEME
                | names::SUBDIVISION_LEVEL
                | names::SUBDIVISION_BOUNDARY
                | names::SUBDIVISION_FACE_VARYING
                | names::SUBDIVISION_TRIANGLES
        ),
        Domain::Point | Domain::Edge => name == names::CREASE,
        Domain::Primitive => name == names::SUBDIVISION_HOLE,
        _ => false,
    }
}
