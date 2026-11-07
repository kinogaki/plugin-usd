// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Attribute values of a flattened layer at a time (`UsdAttribute::Get`
//! over one layer): time samples when authored (linear interpolation of
//! floating-point values with matching shapes, held values otherwise),
//! else the default; a value block reads as no value.

use std::sync::Arc;

use crate::usd::sdf::{Array, Half, LayerData, Path, Token, Value, field};

/// When to read values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Time {
    /// The default value, falling back to the first sample.
    Default,
    /// A time code: samples interpolate, defaults are ignored when samples
    /// exist.
    Code(f64),
}

/// The value of `property` on the prim at `prim`, at `time`.
pub fn value_at(layer: &LayerData, prim: &Path, property: &str, time: Time) -> Option<Value> {
    let spec = layer.spec(&prim.property(property))?;
    let samples = spec
        .get(field::TIME_SAMPLES)
        .and_then(Value::as_time_samples)
        .filter(|s| !s.is_empty());
    let resolved = match (samples, time) {
        (Some(samples), Time::Code(time)) => sample(samples, time),
        (Some(samples), Time::Default) => match spec.get(field::DEFAULT) {
            Some(value) => Some(value.clone()),
            None => Some(samples[0].1.clone()),
        },
        (None, _) => spec.get(field::DEFAULT).cloned(),
    };
    resolved.filter(|value| !matches!(value, Value::ValueBlock | Value::AnimationBlock))
}

/// The color space `property` of the prim at `prim` is authored in: its
/// own `colorSpace` metadata, else the nearest `colorSpace:name`
/// (ColorSpaceAPI) on the prim or an ancestor. None: the file doesn't say,
/// which USD reads as linear Rec.709 (`lin_rec709_scene`).
pub fn color_space(layer: &LayerData, prim: &Path, property: &str) -> Option<Token> {
    let own = layer.get(&prim.property(property), field::COLOR_SPACE);
    if let Some(Value::Token(space)) = own {
        return Some(space.clone());
    }
    let mut at = prim.clone();
    while !at.is_empty() && !at.is_absolute_root() {
        if let Some(Value::Token(space)) = value_at(layer, &at, "colorSpace:name", Time::Default) {
            return Some(space);
        }
        at = at.parent();
    }
    None
}

/// A time-sampled value at `time`: held before the first and after the
/// last sample, linearly interpolated in between.
fn sample(samples: &[(f64, Value)], time: f64) -> Option<Value> {
    let after = samples.partition_point(|(at, _)| *at <= time);
    if after == 0 {
        return Some(samples[0].1.clone());
    }
    let (t0, v0) = &samples[after - 1];
    if after == samples.len() || *t0 == time {
        return Some(v0.clone());
    }
    let (t1, v1) = &samples[after];
    let fraction = (time - t0) / (t1 - t0);
    Some(lerp(v0, v1, fraction).unwrap_or_else(|| v0.clone()))
}

fn mix_f64(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn mix_slices<const N: usize, T: Copy>(
    a: &[T; N],
    b: &[T; N],
    t: f64,
    f: impl Fn(T, T, f64) -> T,
) -> [T; N] {
    std::array::from_fn(|i| f(a[i], b[i], t))
}

fn mix_f32(a: f32, b: f32, t: f64) -> f32 {
    mix_f64(f64::from(a), f64::from(b), t) as f32
}

fn mix_half(a: Half, b: Half, t: f64) -> Half {
    Half::from_f32(mix_f64(a.to_f64(), b.to_f64(), t) as f32)
}

/// Linear interpolation of floating-point values; none for other kinds or
/// arrays of different lengths (which hold instead).
fn lerp(a: &Value, b: &Value, t: f64) -> Option<Value> {
    macro_rules! arrays {
        ($x:expr, $y:expr, $variant:ident, $mix:expr) => {{
            if $x.len() != $y.len() {
                return None;
            }
            Value::Array(Array::$variant(Arc::new(
                $x.iter().zip($y.iter()).map(|(p, q)| $mix(p, q)).collect(),
            )))
        }};
    }
    Some(match (a, b) {
        (Value::Double(x), Value::Double(y)) => Value::Double(mix_f64(*x, *y, t)),
        (Value::Float(x), Value::Float(y)) => Value::Float(mix_f32(*x, *y, t)),
        (Value::Half(x), Value::Half(y)) => Value::Half(mix_half(*x, *y, t)),
        (Value::Vec3d(x), Value::Vec3d(y)) => Value::Vec3d(mix_slices(x, y, t, mix_f64)),
        (Value::Vec3f(x), Value::Vec3f(y)) => Value::Vec3f(mix_slices(x, y, t, mix_f32)),
        (Value::Vec2f(x), Value::Vec2f(y)) => Value::Vec2f(mix_slices(x, y, t, mix_f32)),
        (Value::Vec2d(x), Value::Vec2d(y)) => Value::Vec2d(mix_slices(x, y, t, mix_f64)),
        (Value::Vec4f(x), Value::Vec4f(y)) => Value::Vec4f(mix_slices(x, y, t, mix_f32)),
        (Value::Vec4d(x), Value::Vec4d(y)) => Value::Vec4d(mix_slices(x, y, t, mix_f64)),
        (Value::Matrix4d(x), Value::Matrix4d(y)) => Value::Matrix4d(mix_slices(x, y, t, mix_f64)),
        (Value::Array(Array::Float(x)), Value::Array(Array::Float(y))) => {
            arrays!(x, y, Float, |p: &f32, q: &f32| mix_f32(*p, *q, t))
        }
        (Value::Array(Array::Double(x)), Value::Array(Array::Double(y))) => {
            arrays!(x, y, Double, |p: &f64, q: &f64| mix_f64(*p, *q, t))
        }
        (Value::Array(Array::Vec3f(x)), Value::Array(Array::Vec3f(y))) => {
            arrays!(x, y, Vec3f, |p: &[f32; 3], q: &[f32; 3]| mix_slices(
                p, q, t, mix_f32
            ))
        }
        (Value::Array(Array::Vec3d(x)), Value::Array(Array::Vec3d(y))) => {
            arrays!(x, y, Vec3d, |p: &[f64; 3], q: &[f64; 3]| mix_slices(
                p, q, t, mix_f64
            ))
        }
        (Value::Array(Array::Vec2f(x)), Value::Array(Array::Vec2f(y))) => {
            arrays!(x, y, Vec2f, |p: &[f32; 2], q: &[f32; 2]| mix_slices(
                p, q, t, mix_f32
            ))
        }
        // Rotations interpolate as their angles; quaternions hold.
        _ => return None,
    })
}

/// A token-valued property (`uniform token`), or `fallback`.
pub fn token_at(layer: &LayerData, prim: &Path, property: &str, fallback: &str) -> Token {
    match value_at(layer, prim, property, Time::Default) {
        Some(Value::Token(token)) => token,
        _ => Token::new(fallback),
    }
}

/// Numeric scalars of a value, flattened (tuples component by component),
/// with the component count; none for non-numeric values.
pub fn reals(value: &Value) -> Option<(Vec<f64>, usize)> {
    macro_rules! flat {
        ($items:expr, $n:expr, $convert:expr) => {
            (
                $items
                    .iter()
                    .flat_map(|item| item.iter().map($convert))
                    .collect(),
                $n,
            )
        };
    }
    let array = match value {
        Value::Array(array) => array,
        other => {
            let single = match other {
                Value::Double(v) | Value::TimeCode(v) => return Some((vec![*v], 1)),
                Value::Float(v) => return Some((vec![f64::from(*v)], 1)),
                Value::Half(v) => return Some((vec![v.to_f64()], 1)),
                Value::Int(v) => return Some((vec![f64::from(*v)], 1)),
                Value::Vec2f(v) => Array::Vec2f(Arc::new(vec![*v])),
                Value::Vec3f(v) => Array::Vec3f(Arc::new(vec![*v])),
                Value::Vec4f(v) => Array::Vec4f(Arc::new(vec![*v])),
                Value::Vec2d(v) => Array::Vec2d(Arc::new(vec![*v])),
                Value::Vec3d(v) => Array::Vec3d(Arc::new(vec![*v])),
                Value::Vec4d(v) => Array::Vec4d(Arc::new(vec![*v])),
                Value::Vec3h(v) => Array::Vec3h(Arc::new(vec![*v])),
                _ => return None,
            };
            return reals(&Value::Array(single));
        }
    };
    Some(match array {
        Array::Float(items) => (items.iter().map(|&v| f64::from(v)).collect(), 1),
        Array::Double(items) | Array::TimeCode(items) => (items.to_vec(), 1),
        Array::Half(items) => (items.iter().map(|v| v.to_f64()).collect(), 1),
        Array::Vec2f(items) => flat!(items, 2, |v: &f32| f64::from(*v)),
        Array::Vec3f(items) => flat!(items, 3, |v: &f32| f64::from(*v)),
        Array::Vec4f(items) => flat!(items, 4, |v: &f32| f64::from(*v)),
        Array::Vec2d(items) => flat!(items, 2, |v: &f64| *v),
        Array::Vec3d(items) => flat!(items, 3, |v: &f64| *v),
        Array::Vec4d(items) => flat!(items, 4, |v: &f64| *v),
        Array::Vec2h(items) => flat!(items, 2, |v: &Half| v.to_f64()),
        Array::Vec3h(items) => flat!(items, 3, |v: &Half| v.to_f64()),
        Array::Vec4h(items) => flat!(items, 4, |v: &Half| v.to_f64()),
        // Quaternions as (i, j, k, real), as stored.
        Array::Quatf(items) => flat!(items, 4, |v: &f32| f64::from(*v)),
        Array::Quatd(items) => flat!(items, 4, |v: &f64| *v),
        _ => return None,
    })
}

/// An int array as i32s (faceVertexCounts, indices), shared.
pub fn ints(value: &Value) -> Option<Arc<Vec<i32>>> {
    match value {
        Value::Array(Array::Int(items)) => Some(items.clone()),
        _ => None,
    }
}

/// An int64 array (ids), shared; an int array converts.
pub fn int64s(value: &Value) -> Option<Arc<Vec<i64>>> {
    match value.as_array()? {
        Array::Int64(items) => Some(items.clone()),
        Array::Int(items) => Some(Arc::new(items.iter().map(|&v| i64::from(v)).collect())),
        _ => None,
    }
}

/// An array of three-component floats (points, normals, positions) as
/// shared f32 triples: `float3`-like arrays as they are, doubles and halves
/// converted.
pub fn vec3fs(value: &Value) -> Option<Arc<Vec<[f32; 3]>>> {
    match value.as_array()? {
        Array::Vec3f(items) => Some(items.clone()),
        Array::Vec3d(items) => Some(Arc::new(
            items.iter().map(|v| v.map(|c| c as f32)).collect(),
        )),
        Array::Vec3h(items) => Some(Arc::new(
            items.iter().map(|v| v.map(Half::to_f32)).collect(),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_hold_and_interpolate() {
        let samples = vec![
            (0.0, Value::Double(0.0)),
            (10.0, Value::Double(10.0)),
            (20.0, Value::Token(Token::new("x"))),
        ];
        assert_eq!(sample(&samples, -5.0), Some(Value::Double(0.0)));
        assert_eq!(sample(&samples, 2.5), Some(Value::Double(2.5)));
        assert_eq!(sample(&samples, 15.0), Some(Value::Double(10.0)));
        assert_eq!(sample(&samples, 30.0), Some(Value::Token(Token::new("x"))));
    }
}
