//! Primvars (`UsdGeomPrimvar`): `primvars:<name>` attributes with an
//! interpolation, an element size and optional `:indices`, kept as
//! authored: the values are the layer's own array, shared.

use std::sync::Arc;

use super::values::{Time, color_space, ints, value_at};
use crate::usd::sdf::{LayerData, Path, Token, Value, field};

/// How a primvar's values spread over a prim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    /// One value for the whole prim.
    Constant,
    /// One value per face (per curve, for curves).
    Uniform,
    /// One value per point, interpolated linearly across faces.
    Varying,
    /// One value per point.
    Vertex,
    /// One value per face vertex.
    FaceVarying,
}

impl Interpolation {
    /// The interpolation named `token` (`UsdGeomTokens`).
    pub fn parse(token: &str) -> Option<Interpolation> {
        Some(match token {
            "constant" => Interpolation::Constant,
            "uniform" => Interpolation::Uniform,
            "varying" => Interpolation::Varying,
            "vertex" => Interpolation::Vertex,
            "faceVarying" => Interpolation::FaceVarying,
            _ => return None,
        })
    }

    /// The token.
    pub fn token(self) -> &'static str {
        match self {
            Interpolation::Constant => "constant",
            Interpolation::Uniform => "uniform",
            Interpolation::Varying => "varying",
            Interpolation::Vertex => "vertex",
            Interpolation::FaceVarying => "faceVarying",
        }
    }
}

/// One primvar.
#[derive(Clone, Debug, PartialEq)]
pub struct Primvar {
    /// The name without `primvars:` (`st`, `displayColor`); `normals`,
    /// `widths` and `velocities` for those attributes.
    pub name: Token,
    /// The value type name as authored (`texCoord2f[]`, `color3f[]`).
    pub type_name: Token,
    /// How values spread over the prim.
    pub interpolation: Interpolation,
    /// Values per element (`elementSize`).
    pub element_size: usize,
    /// The values: the layer's array (a scalar where one was authored).
    pub values: Value,
    /// Indices into the values (`primvars:<name>:indices`), when indexed.
    pub indices: Option<Arc<Vec<i32>>>,
    /// The color space the values are authored in (`colorSpace`, else the
    /// nearest ColorSpaceAPI's); none: the file doesn't say.
    pub color_space: Option<Token>,
}

impl Primvar {
    /// Values held: an array's elements, or one for a scalar.
    pub fn len(&self) -> usize {
        match &self.values {
            Value::Array(array) => array.len(),
            _ => 1,
        }
    }

    /// Whether no value is held.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether it holds `elements` elements of its element size: through
    /// its indices (each naming a value) when indexed.
    pub fn fits(&self, elements: usize) -> bool {
        let needed = elements * self.element_size;
        match &self.indices {
            Some(indices) => {
                let values = self.len();
                indices.len() == needed && indices.iter().all(|&i| i >= 0 && (i as usize) < values)
            }
            None => self.len() == needed,
        }
    }
}

/// The primvar (or the plain attribute `normals`, `widths` or
/// `velocities`) named by the property `property` of the prim at `prim`;
/// none when unauthored or not an attribute value.
pub fn read_primvar(
    layer: &LayerData,
    prim: &Path,
    property: &str,
    name: &str,
    time: Time,
) -> Option<Primvar> {
    let spec = layer.spec(&prim.property(property))?;
    let type_name = (spec.get(field::TYPE_NAME))
        .and_then(Value::as_token)
        .cloned()
        .unwrap_or_default();
    let fallback = match property {
        "normals" | "widths" | "velocities" => Interpolation::Vertex,
        _ => Interpolation::Constant,
    };
    let interpolation = match spec.get("interpolation") {
        Some(Value::Token(token)) => Interpolation::parse(token)?,
        _ => fallback,
    };
    let element_size = match spec.get("elementSize") {
        Some(Value::Int(size)) if *size >= 1 => *size as usize,
        _ => 1,
    };
    let values = value_at(layer, prim, property, time)?;
    values.attribute_kind()?;
    let indices =
        value_at(layer, prim, &format!("{property}:indices"), time).and_then(|value| ints(&value));
    Some(Primvar {
        name: Token::new(name),
        type_name,
        interpolation,
        element_size,
        values,
        indices,
        color_space: color_space(layer, prim, property),
    })
}

/// Every `primvars:` primvar of the prim at `prim` in name order, with the
/// plain attributes `extra` names (`normals`, `widths`) where no primvar of
/// that name takes their place.
pub fn read_primvars(layer: &LayerData, prim: &Path, extra: &[&str], time: Time) -> Vec<Primvar> {
    let mut primvars: Vec<Primvar> = Vec::new();
    for property in layer
        .spec(prim)
        .map_or(&[][..], |spec| spec.tokens(field::PROPERTIES))
    {
        let Some(name) = property.strip_prefix("primvars:") else {
            continue;
        };
        if name.ends_with(":indices") || primvars.iter().any(|p| p.name.as_str() == name) {
            continue;
        }
        primvars.extend(read_primvar(layer, prim, property, name, time));
    }
    for name in extra {
        if !primvars.iter().any(|p| p.name.as_str() == *name) {
            primvars.extend(read_primvar(layer, prim, name, name, time));
        }
    }
    primvars.sort_by(|a, b| a.name.cmp(&b.name));
    primvars
}
