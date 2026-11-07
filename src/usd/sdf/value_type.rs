// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Value kinds and value type names, after OpenUSD's crate type enum
//! (`pxr/usd/sdf/crateDataTypes.h`) and its value type registry
//! (`pxr/usd/sdf/schema.cpp`, standard and legacy type names).
//!
//! A [`ValueKind`] is the crate file's own type number, so values read
//! from and written to usdc need no mapping. A type name (`point3f[]`,
//! `color3f`) is a kind, an array flag and a [`Role`]; the role is what
//! distinguishes `point3f` from `float3`.

/// What a value holds. The numbers are `crateDataTypes.h`'s.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum ValueKind {
    Bool = 1,
    UChar = 2,
    Int = 3,
    UInt = 4,
    Int64 = 5,
    UInt64 = 6,
    Half = 7,
    Float = 8,
    Double = 9,
    String = 10,
    Token = 11,
    Asset = 12,
    Matrix2d = 13,
    Matrix3d = 14,
    Matrix4d = 15,
    Quatd = 16,
    Quatf = 17,
    Quath = 18,
    Vec2d = 19,
    Vec2f = 20,
    Vec2h = 21,
    Vec2i = 22,
    Vec3d = 23,
    Vec3f = 24,
    Vec3h = 25,
    Vec3i = 26,
    Vec4d = 27,
    Vec4f = 28,
    Vec4h = 29,
    Vec4i = 30,
    Dictionary = 31,
    TokenListOp = 32,
    StringListOp = 33,
    PathListOp = 34,
    ReferenceListOp = 35,
    IntListOp = 36,
    Int64ListOp = 37,
    UIntListOp = 38,
    UInt64ListOp = 39,
    PathVector = 40,
    TokenVector = 41,
    Specifier = 42,
    Permission = 43,
    Variability = 44,
    VariantSelectionMap = 45,
    TimeSamples = 46,
    Payload = 47,
    DoubleVector = 48,
    LayerOffsetVector = 49,
    StringVector = 50,
    ValueBlock = 51,
    Value = 52,
    UnregisteredValue = 53,
    UnregisteredValueListOp = 54,
    PayloadListOp = 55,
    TimeCode = 56,
    PathExpression = 57,
    Relocates = 58,
    Spline = 59,
    AnimationBlock = 60,
}

impl ValueKind {
    /// The kind with crate number `number`.
    pub fn from_number(number: u8) -> Option<ValueKind> {
        use ValueKind::*;
        const ALL: [ValueKind; 60] = [
            Bool,
            UChar,
            Int,
            UInt,
            Int64,
            UInt64,
            Half,
            Float,
            Double,
            String,
            Token,
            Asset,
            Matrix2d,
            Matrix3d,
            Matrix4d,
            Quatd,
            Quatf,
            Quath,
            Vec2d,
            Vec2f,
            Vec2h,
            Vec2i,
            Vec3d,
            Vec3f,
            Vec3h,
            Vec3i,
            Vec4d,
            Vec4f,
            Vec4h,
            Vec4i,
            Dictionary,
            TokenListOp,
            StringListOp,
            PathListOp,
            ReferenceListOp,
            IntListOp,
            Int64ListOp,
            UIntListOp,
            UInt64ListOp,
            PathVector,
            TokenVector,
            Specifier,
            Permission,
            Variability,
            VariantSelectionMap,
            TimeSamples,
            Payload,
            DoubleVector,
            LayerOffsetVector,
            StringVector,
            ValueBlock,
            Value,
            UnregisteredValue,
            UnregisteredValueListOp,
            PayloadListOp,
            TimeCode,
            PathExpression,
            Relocates,
            Spline,
            AnimationBlock,
        ];
        ALL.get(usize::from(number).checked_sub(1)?).copied()
    }

    /// Scalar components per element (a matrix4d has 16); 0 for kinds that
    /// are not numeric attribute values.
    pub fn components(self) -> usize {
        use ValueKind::*;
        match self {
            Bool | UChar | Int | UInt | Int64 | UInt64 | Half | Float | Double | TimeCode => 1,
            Vec2d | Vec2f | Vec2h | Vec2i => 2,
            Vec3d | Vec3f | Vec3h | Vec3i => 3,
            Vec4d | Vec4f | Vec4h | Vec4i | Quatd | Quatf | Quath | Matrix2d => 4,
            Matrix3d => 9,
            Matrix4d => 16,
            _ => 0,
        }
    }

    /// The scalar kind of a vector, quaternion or matrix kind (itself for scalars).
    pub fn scalar(self) -> ValueKind {
        use ValueKind::*;
        match self {
            Vec2d | Vec3d | Vec4d | Quatd | Matrix2d | Matrix3d | Matrix4d => Double,
            Vec2f | Vec3f | Vec4f | Quatf => Float,
            Vec2h | Vec3h | Vec4h | Quath => Half,
            Vec2i | Vec3i | Vec4i => Int,
            other => other,
        }
    }

    /// Bytes of one element in a crate (Gf layouts); 0 for non-attribute kinds.
    pub fn element_bytes(self) -> usize {
        use ValueKind::*;
        match self {
            Bool | UChar => 1,
            Half => 2,
            Int | UInt | Float | String | Token | Asset | PathExpression => 4,
            Int64 | UInt64 | Double | TimeCode => 8,
            other if other.components() > 1 => other.components() * other.scalar().element_bytes(),
            _ => 0,
        }
    }

    /// Whether the kind is an attribute value type (and can form arrays).
    pub fn is_attribute_value(self) -> bool {
        self.element_bytes() > 0
    }

    /// Whether the kind is a quaternion (written real first, stored last).
    pub fn is_quaternion(self) -> bool {
        matches!(self, ValueKind::Quatd | ValueKind::Quatf | ValueKind::Quath)
    }

    /// Rows of a matrix kind; 0 otherwise.
    pub fn matrix_rows(self) -> usize {
        match self {
            ValueKind::Matrix2d => 2,
            ValueKind::Matrix3d => 3,
            ValueKind::Matrix4d => 4,
            _ => 0,
        }
    }

    /// The standard type name of an attribute value kind (`float3`,
    /// `matrix4d`), as dictionaries print their values' types.
    pub fn type_name(self) -> Option<&'static str> {
        TYPE_NAMES[..STANDARD_NAMES]
            .iter()
            .find(|entry| entry.kind == Some(self) && entry.role == Role::Generic)
            .map(|entry| entry.text)
    }
}

/// How geometry treats a value type's values (`SdfValueRoleNames`).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Generic,
    Point,
    Normal,
    Vector,
    Color,
    TextureCoordinate,
    Frame,
    Transform,
    PointIndex,
    Group,
}

/// One registered value type name.
#[derive(Clone, Copy, Debug)]
pub struct TypeName {
    /// The name without `[]`.
    pub text: &'static str,
    /// The kind of its values; none for `opaque` and `group`.
    pub kind: Option<ValueKind>,
    /// What its values mean.
    pub role: Role,
}

const fn entry(text: &'static str, kind: ValueKind, role: Role) -> TypeName {
    TypeName {
        text,
        kind: Some(kind),
        role,
    }
}

/// The standard names come first; then the legacy names older files use.
const STANDARD_NAMES: usize = 56;

#[rustfmt::skip]
const TYPE_NAMES: [TypeName; 85] = {
    use Role::*;
    use ValueKind as K;
    [
        entry("bool", K::Bool, Generic), entry("uchar", K::UChar, Generic), entry("int", K::Int, Generic),
        entry("uint", K::UInt, Generic), entry("int64", K::Int64, Generic), entry("uint64", K::UInt64, Generic),
        entry("half", K::Half, Generic), entry("float", K::Float, Generic), entry("double", K::Double, Generic),
        entry("timecode", K::TimeCode, Generic), entry("string", K::String, Generic), entry("token", K::Token, Generic),
        entry("asset", K::Asset, Generic),
        TypeName { text: "opaque", kind: None, role: Generic },
        TypeName { text: "group", kind: None, role: Group },
        entry("pathExpression", K::PathExpression, Generic),
        entry("double2", K::Vec2d, Generic), entry("double3", K::Vec3d, Generic), entry("double4", K::Vec4d, Generic),
        entry("float2", K::Vec2f, Generic), entry("float3", K::Vec3f, Generic), entry("float4", K::Vec4f, Generic),
        entry("half2", K::Vec2h, Generic), entry("half3", K::Vec3h, Generic), entry("half4", K::Vec4h, Generic),
        entry("int2", K::Vec2i, Generic), entry("int3", K::Vec3i, Generic), entry("int4", K::Vec4i, Generic),
        entry("point3h", K::Vec3h, Point), entry("point3f", K::Vec3f, Point), entry("point3d", K::Vec3d, Point),
        entry("vector3h", K::Vec3h, Vector), entry("vector3f", K::Vec3f, Vector), entry("vector3d", K::Vec3d, Vector),
        entry("normal3h", K::Vec3h, Normal), entry("normal3f", K::Vec3f, Normal), entry("normal3d", K::Vec3d, Normal),
        entry("color3h", K::Vec3h, Color), entry("color3f", K::Vec3f, Color), entry("color3d", K::Vec3d, Color),
        entry("color4h", K::Vec4h, Color), entry("color4f", K::Vec4f, Color), entry("color4d", K::Vec4d, Color),
        entry("quath", K::Quath, Generic), entry("quatf", K::Quatf, Generic), entry("quatd", K::Quatd, Generic),
        entry("matrix2d", K::Matrix2d, Generic), entry("matrix3d", K::Matrix3d, Generic), entry("matrix4d", K::Matrix4d, Generic),
        entry("frame4d", K::Matrix4d, Frame),
        entry("texCoord2f", K::Vec2f, TextureCoordinate), entry("texCoord2d", K::Vec2d, TextureCoordinate),
        entry("texCoord2h", K::Vec2h, TextureCoordinate), entry("texCoord3f", K::Vec3f, TextureCoordinate),
        entry("texCoord3d", K::Vec3d, TextureCoordinate), entry("texCoord3h", K::Vec3h, TextureCoordinate),
        // Legacy names.
        entry("Vec2i", K::Vec2i, Generic), entry("Vec2h", K::Vec2h, Generic), entry("Vec2f", K::Vec2f, Generic),
        entry("Vec2d", K::Vec2d, Generic), entry("Vec3i", K::Vec3i, Generic), entry("Vec3h", K::Vec3h, Generic),
        entry("Vec3f", K::Vec3f, Generic), entry("Vec3d", K::Vec3d, Generic), entry("Vec4i", K::Vec4i, Generic),
        entry("Vec4h", K::Vec4h, Generic), entry("Vec4f", K::Vec4f, Generic), entry("Vec4d", K::Vec4d, Generic),
        entry("Point", K::Vec3d, Point), entry("PointFloat", K::Vec3f, Point), entry("Normal", K::Vec3d, Normal),
        entry("NormalFloat", K::Vec3f, Normal), entry("Vector", K::Vec3d, Vector), entry("VectorFloat", K::Vec3f, Vector),
        entry("Color", K::Vec3d, Color), entry("ColorFloat", K::Vec3f, Color), entry("Quath", K::Quath, Generic),
        entry("Quatf", K::Quatf, Generic), entry("Quatd", K::Quatd, Generic), entry("Matrix2d", K::Matrix2d, Generic),
        entry("Matrix3d", K::Matrix3d, Generic), entry("Matrix4d", K::Matrix4d, Generic), entry("Frame", K::Matrix4d, Frame),
        entry("Transform", K::Matrix4d, Transform), entry("PointIndex", K::Int, PointIndex),
    ]
};

/// The type named `text` (`point3f`, `texCoord2f[]`): its entry and
/// whether it is the array form; none for an unknown name (or `opaque[]`).
pub fn find_type(text: &str) -> Option<(TypeName, bool)> {
    let (name, array) = match text.strip_suffix("[]") {
        Some(name) if !name.is_empty() => (name, true),
        _ => (text, false),
    };
    let entry = TYPE_NAMES.iter().find(|entry| entry.text == name)?;
    if array && entry.kind.is_none() {
        return None;
    }
    Some((*entry, array))
}

/// The name usda writes for the type named `text`
/// (`SdfValueTypeName::GetSerializationName`): legacy names become the
/// standard name of the same kind and role (`Vec3f` is `float3`); names
/// without a standard alias, and unknown names, stay as they are.
pub fn serialization_name(text: &str) -> std::borrow::Cow<'_, str> {
    let Some((entry, array)) = find_type(text) else {
        return text.into();
    };
    let Some(kind) = entry.kind else {
        return text.into();
    };
    match TYPE_NAMES[..STANDARD_NAMES]
        .iter()
        .find(|s| s.kind == Some(kind) && s.role == entry.role)
    {
        Some(standard) if array => format!("{}[]", standard.text).into(),
        Some(standard) => standard.text.into(),
        None => text.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_round_trip() {
        for number in 1..=60 {
            assert_eq!(ValueKind::from_number(number).unwrap() as u8, number);
        }
        assert!(ValueKind::from_number(0).is_none());
        assert!(ValueKind::from_number(61).is_none());
    }

    #[test]
    fn names_resolve() {
        let (entry, array) = find_type("point3f[]").unwrap();
        assert_eq!(
            (entry.kind, entry.role, array),
            (Some(ValueKind::Vec3f), Role::Point, true)
        );
        assert_eq!(serialization_name("Vec3f[]"), "float3[]");
        assert_eq!(serialization_name("PointFloat"), "point3f");
        assert_eq!(serialization_name("Transform"), "Transform");
        assert_eq!(serialization_name("mystery"), "mystery");
        assert!(find_type("opaque[]").is_none());
        assert_eq!(ValueKind::Matrix4d.element_bytes(), 128);
        assert_eq!(ValueKind::Vec3h.element_bytes(), 6);
        assert_eq!(ValueKind::Vec3f.type_name(), Some("float3"));
    }
}
