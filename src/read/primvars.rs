//! A mesh's primvar as an attribute on the domain its interpolation names,
//! in the project's space.
//!
//! A primvar of a standard attribute's name (`st` is `uv`, `displayColor`
//! `Cd`, `normals` `N`, `rest`, `transform`, `id`…) takes the standard's
//! whole type, its values converted to its precision (a `float3[]
//! primvars:rest` widened to the f64 of rest positions, `int[]` ids to
//! i64) and moved as its meaning says; any other takes the layer's own:
//! reals single, whole numbers 32-bit, text as it is, its meaning by its USD
//! type (`point3f`, `normal3f`, `color3f`, `texCoord2f`…). Points move into
//! the project as its points do, vectors are turned and scaled, normals
//! turned, orientations and placements turned as placements are, colors
//! converted from the space they say into the working space. A primvar no
//! attribute can hold (of another width than its standard's, on a domain
//! its standard isn't held on, of text where numbers go) is left out, said
//! with its prim's path: one primvar never fails a read.
use crate::names;
use crate::space;
use crate::usd::scene::{Interpolation, Matrix4d, Primvar};
use crate::usd::sdf::{Array, Path, Value as Usd};
use kinogaki_studio_api::Error;
use kinogaki_studio_api::color::LIN_REC709;
use kinogaki_studio_api::format::Conversion;
use kinogaki_studio_api::geometry::{
    Attribute, AttributeType, Attributes, Column, Domain, Meaning, Storage, Text, Values, Vec3,
    check_name, names as standard,
};

use super::geometry::Shaping;

/// Where a primvar's elements land: the faces (or a point instancer's
/// instances) kept, by their index in the file, and each kept corner's
/// corner in the file, `None` when every one is kept in order.
pub(super) struct Spread<'a> {
    pub primitives: Option<&'a [u32]>,
    pub corners: Option<&'a [u32]>,
    pub points: usize,
    pub faces: usize,
    pub vertices: usize,
}

impl Spread<'_> {
    /// How many elements `domain` holds.
    fn count(&self, domain: Domain) -> usize {
        match domain {
            Domain::Point => self.points,
            Domain::Primitive | Domain::Instance => {
                self.primitives.map_or(self.faces, <[u32]>::len)
            }
            Domain::Vertex => self.corners.map_or(self.vertices, <[u32]>::len),
            _ => 1,
        }
    }

    /// Each element of `domain` by its element in the file.
    fn from(&self, domain: Domain) -> Box<dyn Iterator<Item = usize> + '_> {
        match (domain, self.primitives, self.corners) {
            (Domain::Primitive | Domain::Instance, Some(kept), _) => {
                Box::new(kept.iter().map(|&f| f as usize))
            }
            (Domain::Vertex, _, Some(corners)) => Box::new(corners.iter().map(|&c| c as usize)),
            (Domain::Primitive | Domain::Instance, None, _) => Box::new(0..self.faces),
            (Domain::Vertex, _, None) => Box::new(0..self.vertices),
            (Domain::Point, ..) => Box::new(0..self.points),
            _ => Box::new(0..1),
        }
    }

    /// The items (of `width`) of `values` each element of `domain` holds,
    /// through `indices` when indexed: copied whole where every element is
    /// the file's own.
    fn gather<T: Clone>(
        &self,
        domain: Domain,
        values: &[T],
        width: usize,
        indices: Option<&Vec<i32>>,
    ) -> Vec<T> {
        let whole = match domain {
            Domain::Primitive | Domain::Instance => self.primitives.is_none(),
            Domain::Vertex => self.corners.is_none(),
            _ => true,
        };
        if whole && indices.is_none() {
            return values.to_vec();
        }
        let mut out = Vec::with_capacity(values.len());
        for at in self.from(domain) {
            let item = indices.map_or(at, |i| i[at] as usize);
            out.extend_from_slice(&values[item * width..(item + 1) * width]);
        }
        out
    }
}

/// The domain a primvar of `interpolation` is on.
pub(super) fn domain(interpolation: Interpolation) -> Domain {
    match interpolation {
        Interpolation::Constant => Domain::Detail,
        Interpolation::Uniform => Domain::Primitive,
        Interpolation::Vertex | Interpolation::Varying => Domain::Point,
        Interpolation::FaceVarying => Domain::Vertex,
    }
}

/// `attributes` with `primvar` of the prim at `prim`, on the domain its
/// interpolation names, its elements taken as `spread` says; a primvar no
/// attribute holds is left out, said.
pub(super) fn attribute(
    attributes: Attributes,
    primvar: &Primvar,
    spread: &Spread<'_>,
    cx: &Shaping<'_>,
    prim: &Path,
    warnings: &mut Vec<String>,
) -> Result<Attributes, Error> {
    let domain = domain(primvar.interpolation);
    attribute_on(attributes, (primvar, domain), spread, cx, prim, warnings)
}

/// [`attribute`] onto `domain`: a point instancer's per-instance primvar
/// onto the Instance domain.
pub(super) fn attribute_on(
    attributes: Attributes,
    (primvar, domain): (&Primvar, Domain),
    spread: &Spread<'_>,
    cx: &Shaping<'_>,
    prim: &Path,
    warnings: &mut Vec<String>,
) -> Result<Attributes, Error> {
    let name = names::attribute(&primvar.name);
    let read = Read {
        primvar,
        domain,
        spread,
        cx,
        prim,
    };
    let why = match read.values(name, warnings) {
        Ok((ty, values)) => {
            let items = match ty.storage {
                Storage::Text => values.len(),
                _ => values.len() / usize::from(ty.width),
            };
            let elements = spread.count(domain);
            match Attribute::new(ty, values) {
                Ok(_) if items != elements => {
                    format!("holds {items} values for {elements} elements")
                }
                Ok(attribute) => return attributes.with(domain, name, attribute),
                Err(e) => e.message().to_owned(),
            }
        }
        Err(why) => why,
    };
    let primvar = &primvar.name;
    warnings.push(format!("{prim}: primvar {primvar} {why}; left out"));
    Ok(attributes)
}

/// One primvar being read: of the prim at `prim`, onto `domain`.
struct Read<'p> {
    primvar: &'p Primvar,
    domain: Domain,
    spread: &'p Spread<'p>,
    cx: &'p Shaping<'p>,
    prim: &'p Path,
}

impl Read<'_> {
    /// The type and values of the attribute `name`, what converting them
    /// couldn't do said in `warnings`; why no attribute can hold them.
    fn values(
        &self,
        name: &str,
        warnings: &mut Vec<String>,
    ) -> Result<(AttributeType, Values), String> {
        let primvar = self.primvar;
        let Some((flat, width)) = flat(&primvar.values) else {
            return Err("holds no numbers".into());
        };
        let width = width * primvar.element_size;
        let standard_type = standard::standard_type(name);
        if let Some((_, domains)) = standard_type
            && !domains.contains(&self.domain)
        {
            let interpolation = primvar.interpolation.token();
            return Err(format!("is {interpolation}; {name} isn't held there"));
        }
        let ty = match (standard_type, &flat) {
            (Some((ty, _)), _) => ty,
            (None, Flat::Texts(_)) => AttributeType::TEXT,
            (None, Flat::Integers(_)) => {
                AttributeType::new(Storage::I32, width.min(255) as u8, Meaning::None)
            }
            (None, _) => match u8::try_from(width).ok().filter(|w| (1..=16).contains(w)) {
                Some(w) => AttributeType::new(Storage::F32, w, meaning(primvar)),
                None => return Err("is too wide".into()),
            },
        };
        if check_name(name).is_err() || ty.check().is_err() {
            return Err("can't be an attribute".into());
        }
        let no = || format!("is no {name}");
        if ty.storage != Storage::Text && usize::from(ty.width) != width {
            return Err(no());
        }
        let values = match (flat, ty.storage) {
            (Flat::Singles(v), Storage::F32) => {
                Values::F32(self.singles(self.gather(v, width), ty, warnings))
            }
            (Flat::Doubles(v), Storage::F32) => {
                let narrowed = self.gather(&v, width).iter().map(|&x| x as f32).collect();
                Values::F32(self.singles(narrowed, ty, warnings))
            }
            (Flat::Singles(v), Storage::F64) => {
                let widened = self
                    .gather(v, width)
                    .iter()
                    .map(|&x| f64::from(x))
                    .collect();
                Values::F64(self.doubles(widened, ty))
            }
            (Flat::Doubles(v), Storage::F64) => {
                Values::F64(self.doubles(self.gather(&v, width), ty))
            }
            (Flat::Integers(v), Storage::I32) => Values::I32(Column::new(self.gather(v, width))),
            (Flat::Integers(v), Storage::I64) => {
                let widened = self.gather(v, width).into_iter().map(i64::from).collect();
                Values::I64(Column::new(widened))
            }
            (Flat::Texts(v), Storage::Text) => Values::Text(Text::from_strs(self.gather(&v, 1))),
            _ => return Err(no()),
        };
        Ok((ty, values))
    }

    /// The items each element of the attribute's domain holds.
    fn gather<T: Clone>(&self, values: &[T], width: usize) -> Vec<T> {
        let indices = self.primvar.indices.as_deref();
        self.spread.gather(self.domain, values, width, indices)
    }

    /// Single reals of type `ty` in the project's space, colors in its
    /// working space (as they are, said, where they can't be converted).
    fn singles(
        &self,
        mut out: Vec<f32>,
        ty: AttributeType,
        warnings: &mut Vec<String>,
    ) -> Column<f32> {
        moved(&mut out, ty, self.cx.space);
        let width = usize::from(ty.width);
        if ty.meaning == Meaning::Color && matches!(width, 3 | 4) {
            let colors = self.cx.colors;
            let from = self.primvar.color_space.as_deref().unwrap_or(LIN_REC709);
            let mut rgb: Vec<[f32; 3]> = (out.chunks_exact(width))
                .map(|c| [c[0], c[1], c[2]])
                .collect();
            match colors.convert_f32(from, colors.working(), &mut rgb) {
                Ok(()) => (out.chunks_exact_mut(width).zip(&rgb))
                    .for_each(|(c, w)| c[..3].copy_from_slice(w)),
                Err(e) => warnings.push(format!(
                    "{}: primvar {}: colors kept as they are: {e}",
                    self.prim, self.primvar.name
                )),
            }
        }
        Column::new(out)
    }

    /// Double reals of type `ty` in the project's space.
    fn doubles(&self, mut out: Vec<f64>, ty: AttributeType) -> Column<f64> {
        moved(&mut out, ty, self.cx.space);
        Column::new(out)
    }
}

/// A primvar's meaning by its USD type.
fn meaning(primvar: &Primvar) -> Meaning {
    match primvar.type_name.as_str().trim_end_matches("[]") {
        "texCoord2f" | "texCoord2d" | "texCoord2h" | "texCoord3f" => Meaning::TextureCoord,
        t if t.starts_with("color") => Meaning::Color,
        t if t.starts_with("normal") => Meaning::Normal,
        t if t.starts_with("point") => Meaning::Point,
        t if t.starts_with("vector") => Meaning::Vector,
        _ => Meaning::None,
    }
}

/// A primvar's values, flattened, and how many an item: single reals as
/// the layer holds them, other reals as doubles, whole numbers and text as
/// they are.
enum Flat<'a> {
    Singles(&'a [f32]),
    Doubles(Vec<f64>),
    Integers(&'a [i32]),
    Texts(Vec<&'a str>),
}

fn flat(values: &Usd) -> Option<(Flat<'_>, usize)> {
    Some(match values {
        Usd::Array(Array::Float(v)) => (Flat::Singles(v), 1),
        Usd::Array(Array::Vec2f(v)) => (Flat::Singles(v.as_flattened()), 2),
        Usd::Array(Array::Vec3f(v)) => (Flat::Singles(v.as_flattened()), 3),
        Usd::Array(Array::Vec4f(v)) => (Flat::Singles(v.as_flattened()), 4),
        Usd::Array(Array::Int(v)) => (Flat::Integers(v), 1),
        Usd::Array(Array::String(v) | Array::Token(v)) => {
            (Flat::Texts(v.iter().map(|t| t.as_str()).collect()), 1)
        }
        Usd::String(t) | Usd::Token(t) => (Flat::Texts(vec![t.as_str()]), 1),
        values => {
            let (reals, width) = crate::usd::scene::values::reals(values)?;
            (Flat::Doubles(reals), width)
        }
    })
}

/// Reals at the precision an attribute holds them in.
trait Real: Copy {
    fn wide(self) -> f64;
    fn narrow(v: f64) -> Self;
}

impl Real for f32 {
    fn wide(self) -> f64 {
        f64::from(self)
    }
    fn narrow(v: f64) -> f32 {
        v as f32
    }
}

impl Real for f64 {
    fn wide(self) -> f64 {
        self
    }
    fn narrow(v: f64) -> f64 {
        v
    }
}

/// `out`'s items, of type `ty`, from the stage's space into the project's
/// (`space`): points moved, vectors turned and scaled, normals turned,
/// orientations (x y z w) and placements (USD's matrices, row vectors, to
/// the standard's rows) turned as placements are.
fn moved<T: Real>(out: &mut [T], ty: AttributeType, space: &Conversion) {
    let width = usize::from(ty.width);
    // An item is at most 16 reals (a placement): widened on the stack.
    let each = |out: &mut [T], change: &dyn Fn(&mut [f64])| {
        let mut wide = [0.0; 16];
        for item in out.chunks_exact_mut(width) {
            let wide = &mut wide[..width];
            wide.iter_mut().zip(&*item).for_each(|(w, v)| *w = v.wide());
            change(wide);
            item.iter_mut()
                .zip(&*wide)
                .for_each(|(v, &w)| *v = T::narrow(w));
        }
    };
    let three = |v: &[f64]| Vec3::new(v[0], v[1], v[2]);
    match (ty.meaning, width) {
        // A vector is of lengths: turned and scaled, as a point is.
        (Meaning::Point | Meaning::Vector, 3) => each(out, &|v| {
            v.copy_from_slice(&space.point(three(v)).to_array())
        }),
        (Meaning::Normal, 3) => each(out, &|v| {
            v.copy_from_slice(&space.direction(three(v)).to_array())
        }),
        // A turn's axis turns with the space, and flips where it mirrors.
        (Meaning::Rotation, 4) => each(out, &|v| {
            let sign = if space.mirrors() { -1.0 } else { 1.0 };
            let axis = space.direction(three(v)) * sign;
            v[..3].copy_from_slice(&axis.to_array());
        }),
        (Meaning::Transform, 16) => each(out, &|v| {
            let usd = Matrix4d(std::array::from_fn(|k| v[k]));
            let placed = space.placement(&space::xform(&usd));
            for (r, row) in placed.rows().iter().enumerate() {
                v[4 * r..4 * r + 4].copy_from_slice(row);
            }
            v[12..].copy_from_slice(&[0.0, 0.0, 0.0, 1.0]);
        }),
        _ => {}
    }
}
