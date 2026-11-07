//! Names USD and Kinogaki spell differently: the primvars that are standard
//! attributes (`st` is `uv`), which a material's primvar readers name too,
//! prim names, which are identifiers (made unique by the SDK's
//! `format::Unique`), and the file names asset paths end in.
use std::borrow::Cow;

/// Primvars that are a standard attribute of another name: (primvar,
/// attribute).
const RENAMED: [(&str, &str); 4] = [
    ("st", "uv"),
    ("displayColor", "Cd"),
    ("displayOpacity", "Alpha"),
    ("normals", "N"),
];

/// The attribute primvar `primvar` is read as.
pub(crate) fn attribute(primvar: &str) -> &str {
    (RENAMED.iter())
        .find(|(p, _)| *p == primvar)
        .map_or(primvar, |(_, a)| a)
}

/// The primvar attribute `attribute` is written as.
pub(crate) fn primvar(attribute: &str) -> &str {
    (RENAMED.iter())
        .find(|(_, a)| *a == attribute)
        .map_or(attribute, |(p, _)| p)
}

/// `name` as a prim name, an identifier: each character an identifier can't
/// hold an `_`, an `_` before a leading digit, `Prim` for nothing.
pub(crate) fn identifier(name: &str) -> Cow<'_, str> {
    let fits = |c: char| c == '_' || c.is_ascii_alphanumeric();
    let leading = name.chars().next().is_some_and(|c| c.is_ascii_digit());
    if !name.is_empty() && !leading && name.chars().all(fits) {
        return Cow::Borrowed(name);
    }
    if name.is_empty() {
        return Cow::Borrowed("Prim");
    }
    let mut out = String::with_capacity(name.len() + 1);
    if leading {
        out.push('_');
    }
    out.extend(name.chars().map(|c| if fits(c) { c } else { '_' }));
    Cow::Owned(out)
}

/// The file name an asset path ends in (inside a package too).
pub(crate) fn file_name(path: &str) -> &str {
    let path = path.trim_end_matches(']');
    let start = path.rfind(['/', '[', '\\']).map_or(0, |at| at + 1);
    match &path[start..] {
        "" => "image",
        name => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_primvars_are_renamed_both_ways_and_others_kept() {
        for (p, a) in RENAMED {
            assert_eq!((attribute(p), primvar(a)), (a, p));
        }
        assert_eq!((attribute("UVMap"), primvar("UVMap")), ("UVMap", "UVMap"));
    }

    #[test]
    fn file_names_end_asset_paths_inside_packages_too() {
        assert_eq!(file_name("/dir/a.usdz[textures/wood.png]"), "wood.png");
        assert_eq!(file_name("/dir/tex/wood.png"), "wood.png");
        assert_eq!(file_name("wood.png"), "wood.png");
        assert_eq!(file_name("/dir/"), "image");
    }

    #[test]
    fn prim_names_are_identifiers() {
        assert_eq!(
            identifier("Red Anodized, Polished"),
            "Red_Anodized__Polished"
        );
        assert_eq!(identifier("2nd"), "_2nd");
        assert_eq!(identifier(""), "Prim");
        assert!(matches!(identifier("Chair_01"), Cow::Borrowed("Chair_01")));
    }
}
