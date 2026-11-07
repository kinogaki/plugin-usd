// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Identifier characters and the name orders OpenUSD sorts with.
//!
//! Identifiers start with `_` or an XID_Start character and continue with
//! XID_Continue characters (UAX #31), as `pxr/base/tf/unicodeUtils` reads
//! them. [`dictionary_less`] is `TfDictionaryLessThan`, the order metadata,
//! properties and dictionary keys print in.

use std::cmp::Ordering;

use super::identifier_tables::{CONTINUE, START};

/// Whether `c` may start an identifier.
pub fn is_start(c: char) -> bool {
    let scalar = c as u32;
    if scalar < 128 {
        return c.is_ascii_alphabetic() || c == '_';
    }
    within(START, scalar)
}

/// Whether `c` may continue an identifier.
pub fn is_continue(c: char) -> bool {
    let scalar = c as u32;
    if scalar < 128 {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    within(CONTINUE, scalar)
}

fn within(ranges: &[(u32, u32)], scalar: u32) -> bool {
    ranges
        .binary_search_by(|&(first, last)| {
            if scalar < first {
                Ordering::Greater
            } else if scalar > last {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

/// The byte length of the identifier starting at `at` in `text`, or 0.
pub fn identifier_len(text: &[u8], at: usize) -> usize {
    let Some(&first) = text.get(at) else { return 0 };
    let mut end;
    if first < 128 {
        if !(first.is_ascii_alphabetic() || first == b'_') {
            return 0;
        }
        end = at + 1;
    } else {
        match decode(text, at) {
            Some((c, width)) if is_start(c) => end = at + width,
            _ => return 0,
        }
    }
    while let Some(&byte) = text.get(end) {
        if byte < 128 {
            if byte.is_ascii_alphanumeric() || byte == b'_' {
                end += 1;
                continue;
            }
            break;
        }
        match decode(text, end) {
            Some((c, width)) if is_continue(c) => end += width,
            _ => break,
        }
    }
    end - at
}

/// The width of a continue character at `at` (ASCII or not), or 0.
pub fn continue_len(text: &[u8], at: usize) -> usize {
    let Some(&byte) = text.get(at) else { return 0 };
    if byte < 128 {
        return usize::from(byte.is_ascii_alphanumeric() || byte == b'_');
    }
    match decode(text, at) {
        Some((c, width)) if is_continue(c) => width,
        _ => 0,
    }
}

/// The scalar value and byte width of the UTF-8 sequence at `at`.
fn decode(text: &[u8], at: usize) -> Option<(char, usize)> {
    let lead = text[at];
    let width = match lead {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return None,
    };
    let bytes = text.get(at..at + width)?;
    let c = std::str::from_utf8(bytes).ok()?.chars().next()?;
    Some((c, width))
}

/// Whether `text` is a whole identifier (Unicode rules, as prim names).
pub fn is_identifier(text: &str) -> bool {
    !text.is_empty() && identifier_len(text.as_bytes(), 0) == text.len()
}

/// `TfIsValidIdentifier`: ASCII letters, digits and `_`, not starting
/// with a digit. Dictionary keys print bare when they are one.
pub fn is_ascii_identifier(text: &str) -> bool {
    let bytes = text.as_bytes();
    match bytes.first() {
        Some(first) if first.is_ascii_alphabetic() || *first == b'_' => {}
        _ => return false,
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// `TfDictionaryLessThan`: letters compare ignoring case (`_` before
/// letters), digit runs compare as numbers, and case and leading zeros break
/// ties.
pub fn dictionary_less(left: &str, right: &str) -> bool {
    let l = left.as_bytes();
    let r = right.as_bytes();
    let (l_size, r_size) = (l.len(), r.len());
    let common = l_size.min(r_size);
    let mut l_at = mismatch(l, r, 0, common);
    let mut r_at = l_at;
    if l_at == common && l_size == r_size {
        return false;
    }
    // Both positions advance together until digit runs of different lengths
    // shift them apart.
    let mut l_end = common;
    loop {
        if l_at == l_end {
            break;
        }
        let (lb, rb) = (l[l_at], r[r_at]);
        let both_ascii = lb < 0x80 && rb < 0x80;
        let differs = (lb & 0xDF) != (rb & 0xDF);
        let letters = lb >= 0x40 && rb >= 0x40;
        if both_ascii && differs && letters {
            return (lb.wrapping_add(5) & 31) < (rb.wrapping_add(5) & 31);
        }
        if lb.is_ascii_digit() || rb.is_ascii_digit() {
            if lb.is_ascii_digit() && rb.is_ascii_digit() {
                let mut l_start = l_at;
                let mut r_start = r_at;
                while l_start > 0 && l[l_start - 1].is_ascii_digit() {
                    l_start -= 1;
                }
                while r_start > 0 && r[r_start - 1].is_ascii_digit() {
                    r_start -= 1;
                }
                let mut l_digits = l_at;
                let mut r_digits = r_at;
                while l_digits < l_size && l[l_digits].is_ascii_digit() {
                    l_digits += 1;
                }
                while r_digits < r_size && r[r_digits].is_ascii_digit() {
                    r_digits += 1;
                }
                while l_start != l_digits && l[l_start] == b'0' {
                    l_start += 1;
                }
                while r_start != r_digits && r[r_start] == b'0' {
                    r_start += 1;
                }
                while l_start != l_digits && r_start != r_digits && l[l_start] == r[r_start] {
                    l_start += 1;
                    r_start += 1;
                }
                if (l_start == l_digits) != (r_start == r_digits) {
                    return l_start == l_digits && r_start != r_digits;
                }
                if l_start != l_digits && r_start != r_digits {
                    let l_count = l_digits - l_start;
                    let r_count = r_digits - r_start;
                    if l_count != r_count {
                        return l_count < r_count;
                    }
                    return l[l_start] < r[r_start];
                }
                l_at = l_digits;
                r_at = r_digits;
                l_end = l_at + (l_size - l_at).min(r_size - r_at);
            } else {
                if l_at == 0 {
                    return lb < rb;
                }
                let previous = l[l_at - 1];
                return if previous.is_ascii_digit() {
                    rb.is_ascii_digit()
                } else {
                    lb.is_ascii_digit()
                };
            }
        } else if !lb.is_ascii_alphabetic() || !rb.is_ascii_alphabetic() {
            return lb < rb;
        } else {
            l_at += 1;
            r_at += 1;
        }
        while l_at < l_end && l[l_at] == r[r_at] {
            l_at += 1;
            r_at += 1;
        }
    }
    if l_at != l_size || r_at != r_size {
        return l_at == l_size;
    }
    let first = mismatch(l, r, 0, common);
    let lb = l.get(first).copied().unwrap_or(0);
    let rb = r.get(first).copied().unwrap_or(0);
    rb == b'0' || (lb != b'0' && lb < rb)
}

fn mismatch(a: &[u8], b: &[u8], start: usize, end: usize) -> usize {
    let mut at = start;
    while at < end && a[at] == b[at] {
        at += 1;
    }
    at
}

/// [`dictionary_less`] as an [`Ordering`], for sorting.
pub fn dictionary_order(left: &str, right: &str) -> Ordering {
    if dictionary_less(left, right) {
        Ordering::Less
    } else if dictionary_less(right, left) {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_follow_xid() {
        assert!(is_identifier("points"));
        assert!(is_identifier("_a9"));
        assert!(is_identifier("München"));
        assert!(!is_identifier("9lives"));
        assert!(!is_identifier("a-b"));
        assert!(!is_identifier(""));
        assert!(is_ascii_identifier("abc_1"));
        assert!(!is_ascii_identifier("Mün"));
    }

    #[test]
    fn dictionary_order_matches_tf() {
        let mut names = vec!["b10", "B2", "a", "_x", "b2", "A", "b02", "z"];
        names.sort_by(|a, b| dictionary_order(a, b));
        assert_eq!(names, ["_x", "A", "a", "B2", "b2", "b02", "b10", "z"]);
        assert!(dictionary_less("primvars:st", "primvars:st1"));
        assert!(!dictionary_less("x", "x"));
    }
}
