//! The standard Geometry as USD prims: its polygons a Mesh with its
//! attributes as primvars (`uv` as `st`, `N` as `normals`, `Cd` as
//! `displayColor`, `name` a uniform string, on the interpolation their
//! domain is; colors in linear Rec.709, USD's own, where other applications
//! read them), its faces' materials as bindings (GeomSubsets of family
//! `materialBind` where they differ, each named after the primitive group
//! holding exactly its faces when there is one), its `part` as GeomSubsets
//! of family `part`, and its other primitive and point groups as
//! GeomSubsets; its curves and points as [`super::curves`] writes them.
use super::curves;
use crate::names;
use crate::usd::scene::{
    Interpolation, Mesh, NodeKind, Primvar, Subdivision, Subset, SubsetElement,
};
use crate::usd::sdf::{Array, Token, Value as Usd};
use kinogaki_studio_api::color::{ColorService, LIN_REC709};
use kinogaki_studio_api::format::Unique;
use kinogaki_studio_api::geometry::{
    Attribute, Domain, Geometry, Kind, Meaning, Text, Values, names as standard,
};
use std::collections::HashMap;
use std::sync::Arc;

/// What a value is written as: prims, each with its own name's suffix
/// (empty for the item's own prim) and its material binding.
pub(super) struct Prims {
    pub prims: Vec<(&'static str, NodeKind, Option<usize>)>,
}

/// What writing geometry needs: each material's index in the scene by its
/// key, whether each is double-sided, and the project's colors.
pub(super) struct Shaping<'a> {
    pub materials: &'a HashMap<Arc<str>, usize>,
    pub double_sided: &'a [bool],
    pub colors: &'a dyn ColorService,
}

/// Each primitive's material (an index into the scene's): the `material`
/// text's keys resolved once, a key the file holds none of said.
struct Looks<'v> {
    text: Option<&'v Text>,
    table: Vec<Option<usize>>,
}

impl<'v> Looks<'v> {
    fn of(value: &'v Geometry, cx: &Shaping<'_>, warnings: &mut Vec<String>) -> Looks<'v> {
        let text = value
            .attributes()
            .text(Domain::Primitive, standard::MATERIAL);
        let keys = text.map_or(&[][..], |t| t.table());
        let table = (keys.iter())
            .map(|key| {
                let found = cx.materials.get(&**key).copied();
                if found.is_none() && !key.is_empty() {
                    warnings.push(format!(
                        "faces name {key}, which isn't written; left unbound"
                    ));
                }
                found
            })
            .collect();
        Looks { text, table }
    }

    /// Primitive `p`'s material; an index past the table names nothing.
    fn at(&self, p: usize) -> Option<usize> {
        let text = self.text?;
        self.table
            .get(text.indices()[p] as usize)
            .copied()
            .flatten()
    }
}

/// `value` as USD prims: its polygons a mesh, its curves a prim for each
/// kind, degree, closing and material.
pub(super) fn prims(value: &Geometry, cx: &Shaping<'_>, warnings: &mut Vec<String>) -> Prims {
    let table = value.primitives();
    let looks = Looks::of(value, cx, warnings);
    let polygons: Vec<usize> = (0..table.len())
        .filter(|&p| table.kind(p).is_polygon())
        .collect();
    let mut prims = Vec::new();
    if !polygons.is_empty() {
        let (mesh, material) = mesh(value, &polygons, (&looks, cx), warnings);
        prims.push(("", NodeKind::Mesh(Box::new(mesh)), material));
    }
    type Group = ((Kind, u8, bool), Option<usize>, Vec<usize>);
    let mut groups: Vec<Group> = Vec::new();
    let mut unwritten: Vec<(Kind, u8)> = Vec::new();
    for p in (0..table.len()).filter(|&p| !table.kind(p).is_polygon()) {
        let (kind, degree) = (table.kind(p), table.curves().map_or(1, |c| c.degrees()[p]));
        if curves::form(kind, degree, false).is_none() {
            if !unwritten.contains(&(kind, degree)) {
                unwritten.push((kind, degree));
                warnings.push(format!(
                    "{} curves of degree {degree} have no USD curves; left out",
                    kind.basis().name()
                ));
            }
            continue;
        }
        let of = ((kind, degree, curves::closes(table, p)), looks.at(p));
        match groups.iter_mut().find(|(shape, m, _)| (*shape, *m) == of) {
            Some((.., members)) => members.push(p),
            None => groups.push((of.0, of.1, vec![p])),
        }
    }
    for ((kind, degree, closed), material, members) in groups {
        let Some(form) = curves::form(kind, degree, closed) else {
            continue;
        };
        let made = curves::curves(value, (form, degree, closed), &members, cx.colors, warnings);
        prims.push(("Curves", NodeKind::Curves(Box::new(made)), material));
    }
    if table.is_empty() && !value.positions().is_empty() {
        let points = curves::points(value, cx.colors, warnings);
        prims.push(("", NodeKind::Points(Box::new(points)), None));
    }
    Prims { prims }
}

/// Positions as USD holds them.
pub(super) fn positions(value: &Geometry, at: impl Iterator<Item = usize>) -> Arc<Vec<[f32; 3]>> {
    let p = value.positions();
    Arc::new(at.map(|i| p[i].map(|c| c as f32)).collect())
}

/// The polygons `polygons` of `value` as a Mesh over all its points, and
/// the material every face shows when they all show one.
fn mesh(
    value: &Geometry,
    polygons: &[usize],
    (looks, cx): (&Looks<'_>, &Shaping<'_>),
    warnings: &mut Vec<String>,
) -> (Mesh, Option<usize>) {
    let table = value.primitives();
    let every = polygons.len() == table.len();
    let mut counts = Vec::with_capacity(polygons.len());
    let mut indices = Vec::with_capacity(table.vertex_count());
    let mut corners: Vec<usize> = Vec::new();
    for &p in polygons {
        let points = table.primitive(p);
        counts.push(points.len() as i32);
        indices.extend(points.iter().map(|&i| i as i32));
        if !every {
            corners.extend(table.primitive_vertices(p));
        }
    }
    let mut primvars = Vec::new();
    for (domain, name, attribute) in value.attributes().iter() {
        let (interpolation, picked) = match domain {
            Domain::Point => (Interpolation::Vertex, None),
            Domain::Vertex => (Interpolation::FaceVarying, (!every).then_some(&corners[..])),
            Domain::Primitive => (Interpolation::Uniform, (!every).then_some(polygons)),
            Domain::Detail => (Interpolation::Constant, None),
            _ => continue,
        };
        if matches!(name, standard::MATERIAL | standard::PART) {
            continue;
        }
        match primvar(name, attribute, interpolation, picked, cx.colors, warnings) {
            Some(primvar) => primvars.push(primvar),
            None => warnings.push(format!("attribute {name} has no USD primvar; left out")),
        }
    }
    let at: HashMap<usize, i32> = (polygons.iter().enumerate())
        .map(|(k, &p)| (p, k as i32))
        .collect();
    // The groups, each its faces (by their place in the mesh) or points.
    let mut groups: Vec<(&str, SubsetElement, Vec<i32>)> = Vec::new();
    for (domain, name, members) in value.attributes().groups() {
        let (element, indices): (_, Vec<i32>) = match domain {
            Domain::Primitive => (
                SubsetElement::Face,
                members
                    .iter()
                    .filter_map(|p| at.get(&(p as usize)).copied())
                    .collect(),
            ),
            Domain::Point => (
                SubsetElement::Point,
                members.iter().map(|p| p as i32).collect(),
            ),
            _ => continue,
        };
        if !name.starts_with('.') && !indices.is_empty() {
            groups.push((name, element, indices));
        }
    }
    let mut subsets = Vec::new();
    let mut names = Unique::default();
    let material = materials(polygons, looks, &mut subsets);
    // A material's subset takes the name of the group holding exactly its
    // faces, which isn't written again: a file's subsets come back once.
    for subset in &mut subsets {
        let same = groups.iter().position(|(_, element, indices)| {
            *element == SubsetElement::Face && **indices == *subset.indices
        });
        let named = match same {
            Some(k) => groups.remove(k).0.to_owned(),
            None => subset.name.to_string(),
        };
        subset.name = Token::new(&names.take(&names::identifier(&named)));
    }
    subsets.extend(parts(value, polygons, &mut names));
    for (name, element, indices) in groups {
        subsets.push(Subset {
            name: Token::new(&names.take(&names::identifier(name))),
            element,
            indices: Arc::new(indices),
            family: Token::empty(),
            material: None,
        });
    }
    // A mesh's `doubleSided` is its materials' Double Sided (and on where
    // it shows none, as Studio's default material is).
    let shown = material
        .into_iter()
        .chain(subsets.iter().filter_map(|s| s.material));
    let double_sided = shown.into_iter().all(|m| cx.double_sided[m]);
    let mesh = Mesh {
        points: positions(value, 0..value.positions().len()),
        face_vertex_counts: Arc::new(counts),
        face_vertex_indices: Arc::new(indices),
        left_handed: false,
        primvars,
        subdivision: Subdivision::default(),
        subsets,
        double_sided,
    };
    (mesh, material)
}

/// The faces' `part` as GeomSubsets of family `part`, one a part, each
/// named after it.
fn parts(value: &Geometry, polygons: &[usize], names: &mut Unique) -> Vec<Subset> {
    let Some(text) = value.attributes().text(Domain::Primitive, standard::PART) else {
        return Vec::new();
    };
    let mut parts: Vec<(&str, Vec<i32>)> = Vec::new();
    for (k, &p) in polygons.iter().enumerate() {
        let Some(part) = text.get(p).filter(|part| !part.is_empty()) else {
            continue;
        };
        match parts.iter_mut().find(|(named, _)| *named == part) {
            Some((_, faces)) => faces.push(k as i32),
            None => parts.push((part, vec![k as i32])),
        }
    }
    (parts.into_iter())
        .map(|(part, faces)| Subset {
            name: Token::new(&names.take(&names::identifier(part))),
            element: SubsetElement::Face,
            indices: Arc::new(faces),
            family: Token::new("part"),
            material: None,
        })
        .collect()
}

/// The faces' materials: the one all show, else a GeomSubset binding each
/// (family `materialBind`, named `material_<n>` until named after a group)
/// pushed to `subsets`.
fn materials(polygons: &[usize], looks: &Looks<'_>, subsets: &mut Vec<Subset>) -> Option<usize> {
    looks.text?;
    let mut faces: Vec<(usize, Vec<i32>)> = Vec::new();
    let mut unbound = false;
    for (k, &p) in polygons.iter().enumerate() {
        match looks.at(p) {
            Some(m) => match faces.iter_mut().find(|(at, _)| *at == m) {
                Some((_, members)) => members.push(k as i32),
                None => faces.push((m, vec![k as i32])),
            },
            None => unbound = true,
        }
    }
    match faces.as_slice() {
        [(m, _)] if !unbound => return Some(*m),
        _ => {}
    }
    for (m, members) in faces {
        subsets.push(Subset {
            name: Token::new(&format!("material_{m}")),
            element: SubsetElement::Face,
            indices: Arc::new(members),
            family: Token::new("materialBind"),
            material: Some(m),
        });
    }
    None
}

/// `attribute` (named `name`) as a primvar of `interpolation`, its values
/// at the elements `picked` (all, in order, when none); none for one USD
/// has no type for. Colors are converted from the working space into
/// linear Rec.709, which other applications read a primvar as whatever its
/// `colorSpace` says; where they can't be, they stay in the working space,
/// said there.
pub(super) fn primvar(
    name: &str,
    attribute: &Attribute,
    interpolation: Interpolation,
    picked: Option<&[usize]>,
    colors: &dyn ColorService,
    warnings: &mut Vec<String>,
) -> Option<Primvar> {
    let ty = attribute.ty();
    let width = usize::from(ty.width);
    let meaning = ty.meaning;
    let mut color_space = None;
    let (type_name, values, element_size) = match attribute.values() {
        Values::F32(v) if meaning == Meaning::Color && matches!(width, 3 | 4) => {
            let mut flat = v.as_slice().to_vec();
            let mut rgb: Vec<[f32; 3]> = flat
                .chunks_exact(width)
                .map(|c| [c[0], c[1], c[2]])
                .collect();
            match colors.convert_f32(colors.working(), LIN_REC709, &mut rgb) {
                Ok(()) => (flat.chunks_exact_mut(width).zip(&rgb))
                    .for_each(|(c, l)| c[..3].copy_from_slice(l)),
                Err(e) => {
                    warnings.push(format!(
                        "attribute {name}: colors kept in the working space: {e}"
                    ));
                    color_space = Some(Token::new(colors.working()));
                }
            }
            reals(&flat, width, meaning, picked)
        }
        Values::F32(v) => reals(v.as_slice(), width, meaning, picked),
        Values::F64(v) => {
            let v: Vec<f32> = v.iter().map(|&x| x as f32).collect();
            reals(&v, width, meaning, picked)
        }
        Values::I32(v) if width == 1 => {
            ("int[]", Array::Int(Arc::new(pick(v.as_slice(), picked))), 1)
        }
        Values::I64(v) if width == 1 => (
            "int64[]",
            Array::Int64(Arc::new(pick(v.as_slice(), picked))),
            1,
        ),
        Values::Text(text) => {
            let all: Vec<Token> = text.iter().map(|t| Token::new(t.unwrap_or(""))).collect();
            ("string[]", Array::String(Arc::new(pick(&all, picked))), 1)
        }
        _ => return None,
    };
    Some(Primvar {
        name: Token::new(names::primvar(name)),
        type_name: Token::new(type_name),
        interpolation,
        element_size,
        values: Usd::Array(values),
        indices: None,
        color_space,
    })
}

/// The items `picked` of `values` (all when none).
fn pick<T: Clone>(values: &[T], picked: Option<&[usize]>) -> Vec<T> {
    match picked {
        None => values.to_vec(),
        Some(picked) => picked.iter().map(|&e| values[e].clone()).collect(),
    }
}

/// Reals of `width` an item as USD's array of their meaning: its USD type,
/// the array, and the element size (wider items as several floats).
fn reals(
    flat: &[f32],
    width: usize,
    meaning: Meaning,
    picked: Option<&[usize]>,
) -> (&'static str, Array, usize) {
    fn items<const N: usize>(flat: &[f32], picked: Option<&[usize]>) -> Arc<Vec<[f32; N]>> {
        Arc::new(pick(flat.as_chunks::<N>().0, picked))
    }
    match (meaning, width) {
        (Meaning::TextureCoord, 2) => ("texCoord2f[]", Array::Vec2f(items(flat, picked)), 1),
        (Meaning::TextureCoord, 3) => ("texCoord3f[]", Array::Vec3f(items(flat, picked)), 1),
        (Meaning::Color, 3) => ("color3f[]", Array::Vec3f(items(flat, picked)), 1),
        (Meaning::Color, 4) => ("color4f[]", Array::Vec4f(items(flat, picked)), 1),
        (Meaning::Normal, 3) => ("normal3f[]", Array::Vec3f(items(flat, picked)), 1),
        (Meaning::Point, 3) => ("point3f[]", Array::Vec3f(items(flat, picked)), 1),
        (Meaning::Vector, 3) => ("vector3f[]", Array::Vec3f(items(flat, picked)), 1),
        (_, 1) => ("float[]", Array::Float(Arc::new(pick(flat, picked))), 1),
        (_, 2) => ("float2[]", Array::Vec2f(items(flat, picked)), 1),
        (_, 3) => ("float3[]", Array::Vec3f(items(flat, picked)), 1),
        (_, 4) => ("float4[]", Array::Vec4f(items(flat, picked)), 1),
        (_, n) => {
            let all: Vec<f32> = match picked {
                None => flat.to_vec(),
                Some(picked) => picked
                    .iter()
                    .flat_map(|&e| flat[e * n..(e + 1) * n].iter().copied())
                    .collect(),
            };
            ("float[]", Array::Float(Arc::new(all)), n)
        }
    }
}
