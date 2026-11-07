// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Asset paths resolved as OpenUSD's default resolver and Sdf's layer
//! utilities do (`pxr/usd/ar/defaultResolver.cpp`, `pxr/usd/sdf/layerUtils.cpp`):
//! an absolute path stays as it is; a relative one is anchored at the
//! directory of the layer that authored it; inside a package layer
//! (`/a/b.usdz[c/d.usda]`) a relative path names another file of the
//! package (`/a/b.usdz[c/e.usda]`). Paths are normalized lexically, and a
//! layer's identity is its normalized absolute path.
//!
//! Identifiers separate with `/`; on Windows a backslash is read as one, as
//! OpenUSD's resolver does (`TfNormPath`), and drive (`C:/`) and UNC
//! (`//server/share/`) roots are kept whole.

/// Whether `byte` separates path components here.
fn separator(byte: u8) -> bool {
    byte == b'/' || (cfg!(windows) && byte == b'\\')
}

/// The root a path starts with: `/`, `C:/`, `C:`, `//server/share/`, or "".
fn root(path: &str) -> &str {
    let bytes = path.as_bytes();
    if cfg!(windows) && bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return if bytes.len() >= 3 && separator(bytes[2]) {
            &path[..3]
        } else {
            &path[..2]
        };
    }
    if cfg!(windows) && bytes.len() >= 2 && separator(bytes[0]) && separator(bytes[1]) {
        // //server/share/
        let mut at = 2;
        for _ in 0..2 {
            while at < bytes.len() && !separator(bytes[at]) {
                at += 1;
            }
            if at < bytes.len() {
                at += 1;
            }
        }
        return &path[..at];
    }
    if bytes.first().is_some_and(|&b| separator(b)) {
        return &path[..1];
    }
    ""
}

/// Whether `path` starts at a root: an asset path anchored nowhere.
pub fn rooted(path: &str) -> bool {
    !root(path).is_empty()
}

/// `path` with `.` and `..` components and repeated separators removed,
/// `/` between components; its root kept and never crossed. Leading `..`
/// components of a relative path stay.
pub fn normalize(path: &str) -> String {
    let text: String = path
        .chars()
        .map(|c| {
            if c.is_ascii() && separator(c as u8) {
                '/'
            } else {
                c
            }
        })
        .collect();
    let prefix = root(&text).to_owned();
    let rooted_path = !prefix.is_empty();
    let drive_only = prefix.len() == 2 && prefix.as_bytes()[1] == b':';
    let mut parts: Vec<&str> = Vec::new();
    for part in text[prefix.len()..].split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if parts.last().is_some_and(|last| *last != "..") {
                parts.pop();
                continue;
            }
            if rooted_path {
                continue;
            }
        }
        parts.push(part);
    }
    let mut out = prefix;
    for (index, part) in parts.iter().enumerate() {
        let needs_slash = !out.is_empty() && !out.ends_with('/') && !(drive_only && index == 0);
        if needs_slash {
            out.push('/');
        }
        out.push_str(part);
    }
    out
}

/// Everything before the last `/` ("" for none); a root stays whole.
pub fn directory_of(path: &str) -> &str {
    let prefix = root(path);
    match path[prefix.len()..].rfind('/') {
        Some(at) => &path[..prefix.len() + at],
        None => prefix,
    }
}

/// A package identifier's outer file and inner path, split at its last
/// top-level bracket (`a.usdz[b.usda]` gives `a.usdz` and `b.usda`).
pub fn package_split(identifier: &str) -> Option<(&str, &str)> {
    if identifier.len() < 3 || !identifier.ends_with(']') {
        return None;
    }
    let bytes = identifier.as_bytes();
    let mut depth = 0usize;
    for at in (0..bytes.len()).rev() {
        match bytes[at] {
            b']' => depth += 1,
            b'[' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((&identifier[..at], &identifier[at + 1..identifier.len() - 1]));
                }
            }
            _ => {}
        }
    }
    None
}

/// The identifier `asset` names when authored in the layer `anchor` (an
/// absolute path, or a package path `outer[inner]`). A package's own layer
/// (`/a.usdz`, `/a.usdz[b.usdz]`) names the package's files.
pub fn anchored(asset: &str, anchor: &str) -> String {
    if rooted(asset) {
        return normalize(asset);
    }
    within(asset, anchor)
}

/// The relative `asset` anchored at `anchor`, an identifier or a path
/// inside a package.
fn within(asset: &str, anchor: &str) -> String {
    if let Some((outer, inner)) = package_split(anchor) {
        return format!("{outer}[{}]", within(asset, inner));
    }
    if anchor.len() > 5 && anchor[anchor.len() - 5..].eq_ignore_ascii_case(".usdz") {
        return format!("{anchor}[{}]", normalize(asset));
    }
    match directory_of(anchor) {
        "" => normalize(asset),
        directory => normalize(&format!("{directory}/{asset}")),
    }
}

/// `path` made absolute from the working directory and normalized.
pub fn absolute(path: &str) -> String {
    if rooted(path) {
        return normalize(path);
    }
    match std::env::current_dir() {
        Ok(here) => normalize(&format!("{}/{path}", here.display())),
        Err(_) => normalize(path),
    }
}

/// The file an identifier's bytes are in: a package's outermost file, or
/// the identifier itself.
pub fn outermost(identifier: &str) -> &str {
    let mut target = identifier;
    while let Some((outer, _)) = package_split(target) {
        target = outer;
    }
    target
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_anchors() {
        assert_eq!(normalize("/a/./b//../c"), "/a/c");
        assert_eq!(normalize("../x/../../y"), "../../y");
        assert_eq!(normalize("/../a"), "/a");
        assert_eq!(anchored("b.usda", "/dir/a.usda"), "/dir/b.usda");
        assert_eq!(anchored("../b.usda", "/dir/sub/a.usda"), "/dir/b.usda");
        assert_eq!(anchored("/abs/b.usda", "/dir/a.usda"), "/abs/b.usda");
        assert_eq!(
            anchored("c.usda", "/p/x.usdz[sub/a.usda]"),
            "/p/x.usdz[sub/c.usda]"
        );
        assert_eq!(anchored("./tex/a.png", "/p/x.usdz"), "/p/x.usdz[tex/a.png]");
        assert_eq!(
            anchored("a.png", "/p/x.usdz[in.usdz]"),
            "/p/x.usdz[in.usdz[a.png]]"
        );
        assert_eq!(
            package_split("/a.usdz[b.usdz[c.usda]]"),
            Some(("/a.usdz", "b.usdz[c.usda]"))
        );
        assert_eq!(package_split("/a.usda"), None);
        assert_eq!(directory_of("/a"), "/");
    }
}
