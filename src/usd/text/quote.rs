// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Quoting for usda text, after `Sdf_FileIOUtility::Quote` and
//! `_StringFromAssetPath` (`pxr/usd/sdf/fileIO_Common.cpp`): strings take
//! double quotes unless they hold a `"` and no `'`, triple quotes when they
//! hold a line end; asset paths take `@...@`, or `@@@...@@@` when they hold
//! an `@`, with an inner `@@@` escaped.

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Append `text` quoted (`triple` allows triple quotes for line ends).
pub fn quote_into(out: &mut Vec<u8>, text: &str, triple: bool) {
    let bytes = text.as_bytes();
    let has_double = bytes.contains(&b'"');
    let has_single = bytes.contains(&b'\'');
    let quote = if has_double && !has_single {
        b'\''
    } else {
        b'"'
    };
    let tripled = triple && bytes.contains(&b'\n');
    out.reserve(bytes.len() + 8);
    let open = if tripled { 3 } else { 1 };
    out.extend(std::iter::repeat_n(quote, open));
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let width = multibyte(bytes, index);
        if width > 0 {
            out.extend_from_slice(&bytes[index..index + width]);
            index += width;
            continue;
        }
        match byte {
            b'\n' if tripled => out.push(b'\n'),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\\' => out.extend_from_slice(b"\\\\"),
            _ if byte == quote => {
                out.push(b'\\');
                out.push(quote);
            }
            _ if !(32..=126).contains(&byte) => {
                out.extend_from_slice(b"\\x");
                out.push(HEX[usize::from(byte >> 4)]);
                out.push(HEX[usize::from(byte & 15)]);
            }
            _ => out.push(byte),
        }
        index += 1;
    }
    out.extend(std::iter::repeat_n(quote, open));
}

/// Append the asset path `path` delimited as usda writes it.
pub fn asset_into(out: &mut Vec<u8>, path: &str) {
    let bytes = path.as_bytes();
    let triple = bytes.contains(&b'@');
    let delimiters = if triple { 3 } else { 1 };
    out.extend(std::iter::repeat_n(b'@', delimiters));
    let mut index = 0;
    while index < bytes.len() {
        if triple && bytes[index..].starts_with(b"@@@") {
            out.extend_from_slice(b"\\@@@");
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    out.extend(std::iter::repeat_n(b'@', delimiters));
}

/// The width of a well-formed-looking UTF-8 multi-byte sequence at `index`
/// (fileIO_Common's `_IsUTF8MultiByte`: lead and continuation bits only).
fn multibyte(bytes: &[u8], index: usize) -> usize {
    let lead = bytes[index];
    let width = if lead & 0xE0 == 0xC0 {
        2
    } else if lead & 0xF0 == 0xE0 {
        3
    } else if lead & 0xF8 == 0xF0 {
        4
    } else {
        return 0;
    };
    if index + width > bytes.len() {
        return 0;
    }
    if bytes[index + 1..index + width]
        .iter()
        .any(|b| b & 0xC0 != 0x80)
    {
        return 0;
    }
    width
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quoted(text: &str) -> String {
        let mut out = Vec::new();
        quote_into(&mut out, text, true);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn quotes_as_sdf() {
        assert_eq!(quoted("plain"), "\"plain\"");
        assert_eq!(quoted("say \"hi\""), "'say \"hi\"'");
        assert_eq!(quoted("both \" '"), "\"both \\\" '\"");
        assert_eq!(quoted("two\nlines"), "\"\"\"two\nlines\"\"\"");
        assert_eq!(quoted("tab\there"), "\"tab\\there\"");
        assert_eq!(quoted("Grün\u{1}"), "\"Grün\\x01\"");
        let mut out = Vec::new();
        asset_into(&mut out, "a@b.usda");
        assert_eq!(String::from_utf8(out).unwrap(), "@@@a@b.usda@@@");
    }
}
