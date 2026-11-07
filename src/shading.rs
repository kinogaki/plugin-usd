//! What reading and writing materials share: UsdShade's render contexts,
//! USD Preview Surface's nodes as MaterialX names them, and the values a
//! shader input holds, as USD types them and as the SDK holds them.
//!
//! A Material prim holds a network per render context, each behind its own
//! terminal outputs (DESIGN §8.2):
//! - [`KINOGAKI`]: an engine's own nodes, each `info:id` its node type's id
//!   (`kinogaki.spectre.anodized_aluminum_surface`), values by their kind;
//! - [`MTLX`]: MaterialX in USD (UsdMtlx), each `info:id` a MaterialX node
//!   definition (`ND_open_pbr_surface_surfaceshader`), values by its types;
//! - [`UNIVERSAL`]: USD Preview Surface, `info:id` its own names
//!   (`UsdPreviewSurface`, `UsdUVTexture`), which MaterialX defines too.
//!
//! In every context a MaterialX node is written by its definition and an
//! engine's by its node type, so reading tells them apart by the id alone.
use crate::usd::sdf::{Array, Token, Value as Usd};
use kinogaki_mtlx::Type;
use kinogaki_studio_api::Value;
use std::sync::Arc;

/// The render context of an engine's own nodes.
pub(crate) const KINOGAKI: &str = "kinogaki";
/// The render context of MaterialX networks.
pub(crate) const MTLX: &str = "mtlx";
/// The universal render context: USD Preview Surface.
pub(crate) const UNIVERSAL: &str = "";

/// The space USD Preview Surface's colors are in, and any color USD gives
/// without a `colorSpace`: linear Rec.709.
pub(crate) const LINEAR_REC709: &str = "lin_rec709_scene";

/// The `customData` key holding the name a material had in Kinogaki.
pub(crate) const NAME_KEY: &str = "kinogaki:name";

/// The terminal output of `context`'s `terminal` (`surface`,
/// `displacement`): `surface`, `mtlx:surface`.
pub(crate) fn terminal(context: &str, terminal: &str) -> String {
    match context {
        UNIVERSAL => terminal.to_owned(),
        context => format!("{context}:{terminal}"),
    }
}

/// One of USD Preview Surface's shaders: its `info:id`, the MaterialX
/// definition it is, and its USD output MaterialX names otherwise
/// (`result`, `out`).
struct Shader {
    id: &'static str,
    signature: &'static str,
    renamed: Option<(&'static str, &'static str)>,
}

/// USD Preview Surface's shaders.
const PREVIEW: &[Shader] = &[
    Shader {
        id: "UsdPreviewSurface",
        signature: "ND_UsdPreviewSurface_surfaceshader",
        renamed: Some(("surface", "out")),
    },
    Shader {
        id: "UsdUVTexture",
        signature: "ND_UsdUVTexture_23",
        renamed: None,
    },
    Shader {
        id: "UsdPrimvarReader_float",
        signature: "ND_UsdPrimvarReader_float",
        renamed: Some(("result", "out")),
    },
    Shader {
        id: "UsdPrimvarReader_float2",
        signature: "ND_UsdPrimvarReader_vector2",
        renamed: Some(("result", "out")),
    },
    Shader {
        id: "UsdPrimvarReader_float3",
        signature: "ND_UsdPrimvarReader_vector3",
        renamed: Some(("result", "out")),
    },
    Shader {
        id: "UsdPrimvarReader_float4",
        signature: "ND_UsdPrimvarReader_vector4",
        renamed: Some(("result", "out")),
    },
    Shader {
        id: "UsdPrimvarReader_int",
        signature: "ND_UsdPrimvarReader_integer",
        renamed: Some(("result", "out")),
    },
    Shader {
        id: "UsdPrimvarReader_string",
        signature: "ND_UsdPrimvarReader_string",
        renamed: Some(("result", "out")),
    },
    Shader {
        id: "UsdTransform2d",
        signature: "ND_UsdTransform2d",
        renamed: Some(("result", "out")),
    },
];

/// The three-wide primvar readers USD names by what they read.
const PREVIEW_ALIASES: &[(&str, &str)] = &[
    ("UsdPrimvarReader_normal", "ND_UsdPrimvarReader_vector3"),
    ("UsdPrimvarReader_point", "ND_UsdPrimvarReader_vector3"),
    ("UsdPrimvarReader_vector", "ND_UsdPrimvarReader_vector3"),
];

/// The MaterialX definition of USD Preview Surface's shader `id`.
pub(crate) fn preview_signature(id: &str) -> Option<&'static str> {
    (PREVIEW.iter().map(|s| (s.id, s.signature)))
        .chain(PREVIEW_ALIASES.iter().copied())
        .find(|(known, _)| *known == id)
        .map(|(_, signature)| signature)
}

/// USD Preview Surface's `info:id` of MaterialX definition `signature`.
pub(crate) fn preview_id(signature: &str) -> Option<&'static str> {
    (PREVIEW.iter())
        .find(|s| s.signature == signature)
        .map(|s| s.id)
}

/// The MaterialX name of output `output` of USD Preview Surface's shader of
/// definition `signature`.
pub(crate) fn mtlx_output<'a>(signature: &str, output: &'a str) -> &'a str {
    match PREVIEW.iter().find(|s| s.signature == signature) {
        Some(Shader {
            renamed: Some((usd, mtlx)),
            ..
        }) if *usd == output => mtlx,
        _ => output,
    }
}

/// The USD name of output `output` of USD Preview Surface's shader of
/// definition `signature`.
pub(crate) fn usd_output<'a>(signature: &str, output: &'a str) -> &'a str {
    match PREVIEW.iter().find(|s| s.signature == signature) {
        Some(Shader {
            renamed: Some((usd, mtlx)),
            ..
        }) if *mtlx == output => usd,
        _ => output,
    }
}

/// USD UV Texture's wrap token as MaterialX's enumeration spells it
/// (`repeat` is `periodic`; `useMetadata`, which MaterialX lacks, reads as
/// `black`: D26).
pub(crate) fn mtlx_wrap(usd: &str) -> &str {
    match usd {
        "repeat" => "periodic",
        "useMetadata" => "black",
        other => other,
    }
}

/// MaterialX's wrap as USD UV Texture's token.
pub(crate) fn usd_wrap(mtlx: &str) -> &str {
    match mtlx {
        "periodic" => "repeat",
        other => other,
    }
}

/// `opacityMode`'s tokens, in MaterialX's enumeration order.
pub(crate) const OPACITY_MODES: [&str; 2] = ["transparent", "presence"];

/// USD Preview Surface's inputs USD types otherwise than UsdMtlx types
/// their MaterialX ports: (shader `info:id`, input, USD type).
const PREVIEW_TYPES: &[(&str, &str, &str)] = &[
    ("UsdPreviewSurface", "normal", "normal3f"),
    ("UsdPreviewSurface", "useSpecularWorkflow", "int"),
    ("UsdPreviewSurface", "opacityMode", "token"),
    ("UsdUVTexture", "wrapS", "token"),
    ("UsdUVTexture", "wrapT", "token"),
    ("UsdUVTexture", "fallback", "float4"),
    ("UsdUVTexture", "scale", "float4"),
    ("UsdUVTexture", "bias", "float4"),
];

/// The USD type a MaterialX port of type `ty` is written as (UsdMtlx's), or
/// USD Preview Surface's own for its shader `preview`'s input `input`.
pub(crate) fn usd_type(ty: &Type, preview: Option<(&str, &str)>) -> &'static str {
    if let Some((id, input)) = preview
        && let Some((.., usd)) = (PREVIEW_TYPES.iter()).find(|(i, n, _)| *i == id && *n == input)
    {
        return usd;
    }
    match ty {
        Type::Boolean => "bool",
        Type::Integer => "int",
        Type::Float => "float",
        Type::Color3 => "color3f",
        Type::Color4 => "color4f",
        Type::Vector2 => "float2",
        Type::Vector3 => "float3",
        Type::Vector4 => "float4",
        Type::Matrix33 => "matrix3d",
        Type::Matrix44 => "matrix4d",
        Type::String => "string",
        Type::Filename => "asset",
        Type::Named(_) => "token",
    }
}

/// The numbers a USD value holds, a scalar or a tuple (a bool or text: none).
pub(crate) fn numbers(value: &Usd) -> Option<Vec<f64>> {
    let (numbers, _) = crate::usd::scene::values::reals(value)?;
    (!matches!(value, Usd::Array(_))).then_some(numbers)
}

/// A USD value as a value of MaterialX type `ty` held as the SDK holds it:
/// a float a number, a color3 a color, a color4 or a vector numbers, an
/// integer or boolean as it is, a string its text (a choice when the port
/// has an enumeration). None when it doesn't read as one.
pub(crate) fn mtlx_value(value: &Usd, ty: &Type, enumerated: bool) -> Option<Value> {
    Some(match ty {
        Type::Boolean => Value::Bool(value.as_bool()?),
        Type::Integer => Value::Integer(integer(value)?),
        Type::Float => Value::Number(*numbers(value)?.first()?),
        Type::Color3 => match numbers(value)?[..] {
            [r, g, b] => Value::Color([r, g, b]),
            _ => return None,
        },
        Type::String if enumerated => Value::Choice(Arc::from(text(value)?)),
        Type::String => Value::Text(text(value)?.to_owned()),
        ty if ty.width() > 1 => {
            let n = numbers(value)?;
            (n.len() == ty.width()).then_some(Value::Numbers(n))?
        }
        _ => return None,
    })
}

/// A value of MaterialX type `ty` as a USD value of type `usd` (what
/// [`usd_type`] gives).
pub(crate) fn usd_value(value: &Value, usd: &str) -> Option<Usd> {
    let floats = |n: &[f64]| -> Vec<f32> { n.iter().map(|&v| v as f32).collect() };
    Some(match (value, usd) {
        (Value::Bool(b), _) => Usd::Bool(*b),
        (Value::Integer(n), "int") => Usd::Int(i32::try_from(*n).ok()?),
        (Value::Number(n), "float") => Usd::Float(*n as f32),
        (Value::Number(n), _) => Usd::Double(*n),
        (Value::Choice(c), "token") => Usd::Token(Token::new(c)),
        (Value::Choice(c), _) => Usd::String(Token::new(c)),
        (Value::Text(t), "token") => Usd::Token(Token::new(t)),
        (Value::Text(t), _) => Usd::String(Token::new(t)),
        (Value::Color(c), _) => Usd::Vec3f(floats(c).try_into().ok()?),
        (Value::Numbers(n), _) => match floats(n)[..] {
            [a, b] => Usd::Vec2f([a, b]),
            [a, b, c] => Usd::Vec3f([a, b, c]),
            [a, b, c, d] => Usd::Vec4f([a, b, c, d]),
            _ => return None,
        },
        _ => return None,
    })
}

/// An engine node's value as USD writes it (no schema says its types, so
/// its kind does): a number a `double`, an integer an `int64`, a toggle a
/// `bool`, a color a `color3d`, numbers a `double[]`, a choice a `token`,
/// text a `string`. None for a value of no such kind (data).
pub(crate) fn engine_usd(value: &Value) -> Option<(&'static str, Usd)> {
    Some(match value {
        Value::Number(n) => ("double", Usd::Double(*n)),
        Value::Integer(n) => ("int64", Usd::Int64(*n)),
        Value::Bool(b) => ("bool", Usd::Bool(*b)),
        Value::Color(c) => ("color3d", Usd::Vec3d(*c)),
        Value::Numbers(n) => ("double[]", Usd::Array(Array::Double(Arc::new(n.clone())))),
        Value::Choice(c) => ("token", Usd::Token(Token::new(c))),
        Value::Text(t) => ("string", Usd::String(Token::new(t))),
        _ => return None,
    })
}

/// An engine node's USD value of type `type_name` as the SDK holds it: the
/// kinds [`engine_usd`] writes, and what other writers spell them as.
pub(crate) fn engine_value(value: &Usd, type_name: &str) -> Option<Value> {
    if type_name.starts_with("color3") {
        return match numbers(value)?[..] {
            [r, g, b] => Some(Value::Color([r, g, b])),
            _ => None,
        };
    }
    Some(match value {
        Usd::Bool(b) => Value::Bool(*b),
        Usd::Token(t) => Value::Choice(Arc::from(t.as_str())),
        Usd::String(t) => Value::Text(t.to_string()),
        Usd::Array(_) => Value::Numbers(crate::usd::scene::values::reals(value)?.0),
        _ => match integer(value) {
            Some(n) => Value::Integer(n),
            None => match numbers(value)?[..] {
                [n] => Value::Number(n),
                ref many => Value::Numbers(many.to_vec()),
            },
        },
    })
}

/// A whole-number USD value.
fn integer(value: &Usd) -> Option<i64> {
    Some(match value {
        Usd::Int(n) => i64::from(*n),
        Usd::Int64(n) => *n,
        Usd::UInt(n) => i64::from(*n),
        Usd::UChar(n) => i64::from(*n),
        Usd::UInt64(n) => i64::try_from(*n).ok()?,
        _ => return None,
    })
}

/// A text-valued USD value (a token or a string).
pub(crate) fn text(value: &Usd) -> Option<&str> {
    match value {
        Usd::Token(t) | Usd::String(t) => Some(t.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_shaders_are_materialx_definitions_and_back() {
        for shader in PREVIEW {
            let (id, signature) = (shader.id, shader.signature);
            assert_eq!(preview_signature(id), Some(signature));
            assert_eq!(preview_id(signature), Some(id));
            assert!(
                kinogaki_mtlx::library().def(signature).is_some(),
                "{signature}"
            );
            if let Some((usd, mtlx)) = shader.renamed {
                assert_eq!(mtlx_output(signature, usd), mtlx);
                assert_eq!(usd_output(signature, mtlx), usd);
            }
        }
        assert_eq!(
            preview_signature("UsdPrimvarReader_normal"),
            Some("ND_UsdPrimvarReader_vector3")
        );
    }

    #[test]
    fn engine_values_read_back_as_their_kind() {
        let values = [
            Value::Number(0.85),
            Value::Integer(i64::MAX),
            Value::Bool(true),
            Value::Color([0.62, 0.04, 0.03]),
            Value::Numbers(vec![1.0, 2.0, 3.5]),
            Value::Choice(Arc::from("polished")),
            Value::Text("a note".into()),
        ];
        for value in values {
            let (ty, usd) = engine_usd(&value).unwrap();
            assert_eq!(engine_value(&usd, ty), Some(value));
        }
    }

    #[test]
    fn materialx_values_read_back_through_their_usd_types() {
        let cases = [
            (Type::Float, Value::Number(0.5)),
            (Type::Color3, Value::Color([0.25, 0.5, 1.0])),
            (Type::Vector2, Value::Numbers(vec![2.0, 0.5])),
            (Type::Color4, Value::Numbers(vec![0.0, 0.0, 0.0, 1.0])),
            (Type::Integer, Value::Integer(3)),
            (Type::Boolean, Value::Bool(false)),
            (Type::String, Value::Text("st".into())),
        ];
        for (ty, value) in cases {
            let usd = usd_value(&value, usd_type(&ty, None)).unwrap();
            assert_eq!(mtlx_value(&usd, &ty, false), Some(value), "{ty}");
        }
        let periodic = Value::Choice(Arc::from("periodic"));
        let usd = usd_value(
            &periodic,
            usd_type(&Type::String, Some(("UsdUVTexture", "wrapS"))),
        );
        assert_eq!(usd, Some(Usd::Token(Token::new("periodic"))));
        assert_eq!(
            mtlx_value(&usd.unwrap(), &Type::String, true),
            Some(periodic)
        );
    }
}
