// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Atoms: numbers (classified as the parser classifies them), quoted strings
//! with `Sdf_EvalQuotedString`'s escapes, asset paths and path references.

use std::borrow::Cow;

use super::Reader;
use crate::usd::error::Result;
use crate::usd::sdf::Path;

/// A number literal, classified as `_GetNumericValueFromString` does:
/// integers stay `u64` (or `i64` when negative) unless they overflow;
/// anything with `.`, `e` or `E` (and inf, nan, -0) is a double.
#[derive(Clone, Copy, Debug)]
pub(super) enum Number {
    Unsigned(u64),
    Signed(i64),
    Real(f64),
}

impl Number {
    /// As a double (integers convert).
    pub(super) fn as_f64(self) -> f64 {
        match self {
            Number::Unsigned(value) => value as f64,
            Number::Signed(value) => value as f64,
            Number::Real(value) => value,
        }
    }
}

impl<'a> Reader<'a> {
    /// Whether a number starts at the cursor.
    pub(super) fn sees_number(&self) -> bool {
        let byte = self.peek();
        if byte.is_ascii_digit() || (byte == b'.' && self.peek_at(1).is_ascii_digit()) {
            return true;
        }
        if byte == b'-' {
            let next = self.peek_at(1);
            return next.is_ascii_digit()
                || next == b'.'
                || (next == b'i' && self.peek_at(2) == b'n' && self.peek_at(3) == b'f');
        }
        self.sees("inf") || self.sees("nan")
    }

    /// The number at the cursor, consumed and classified.
    pub(super) fn number(&mut self) -> Result<Number> {
        let start = self.at;
        if self.keyword("nan") {
            return Ok(Number::Real(f64::NAN));
        }
        let negative = self.peek() == b'-';
        if negative {
            self.at += 1;
        }
        if self.keyword("inf") {
            return Ok(Number::Real(if negative {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }));
        }
        let mut real = false;
        let digits_start = self.at;
        while self.peek().is_ascii_digit() {
            self.at += 1;
        }
        let whole = self.at - digits_start;
        if self.peek() == b'.' {
            if whole == 0 && !self.peek_at(1).is_ascii_digit() {
                self.at = start;
                return Err(self.error("expected a number"));
            }
            real = true;
            self.at += 1;
            while self.peek().is_ascii_digit() {
                self.at += 1;
            }
        } else if whole == 0 {
            self.at = start;
            return Err(self.error("expected a number"));
        }
        if matches!(self.peek(), b'e' | b'E') {
            real = true;
            self.at += 1;
            if matches!(self.peek(), b'+' | b'-') {
                self.at += 1;
            }
            if !self.peek().is_ascii_digit() {
                return Err(self.error("expected exponent digits"));
            }
            while self.peek().is_ascii_digit() {
                self.at += 1;
            }
        }
        let text = self.text(start, self.at);
        if text == "-0" {
            return Ok(Number::Real(-0.0));
        }
        if !real {
            let magnitude = text.trim_start_matches('-').parse::<u64>().ok();
            match (magnitude, negative) {
                (Some(magnitude), false) => return Ok(Number::Unsigned(magnitude)),
                (Some(magnitude), true) if magnitude <= 1 << 63 => {
                    return Ok(Number::Signed((magnitude as i64).wrapping_neg()));
                }
                // Out of range: reads as a double.
                _ => {}
            }
        }
        text.parse::<f64>()
            .map(Number::Real)
            .map_err(|_| self.error("not a number"))
    }

    /// Whether a quoted string starts at the cursor.
    pub(super) fn sees_string(&self) -> bool {
        matches!(self.peek(), b'"' | b'\'')
    }

    /// The quoted string at the cursor with its escapes evaluated.
    pub(super) fn string(&mut self) -> Result<Cow<'a, str>> {
        let quote = self.peek();
        if quote != b'"' && quote != b'\'' {
            return Err(self.error("expected a quoted string"));
        }
        let open = self.at;
        let triple = self.peek_at(1) == quote && self.peek_at(2) == quote;
        let width = if triple { 3 } else { 1 };
        self.at += width;
        let start = self.at;
        loop {
            if self.at_end() {
                self.at = open;
                return Err(self.error("an unterminated string"));
            }
            let byte = self.peek();
            if byte == b'\\' {
                self.at += 2;
                continue;
            }
            if !triple && matches!(byte, b'\n' | b'\r') {
                return Err(self.error("a line end inside a string"));
            }
            if byte == quote && (!triple || (self.peek_at(1) == quote && self.peek_at(2) == quote))
            {
                break;
            }
            self.at += 1;
        }
        let raw = &self.data[start..self.at];
        self.at += width;
        let text = unescape(raw);
        match text {
            Cow::Borrowed(bytes) => std::str::from_utf8(bytes).map(Cow::Borrowed),
            Cow::Owned(bytes) => String::from_utf8(bytes)
                .map(Cow::Owned)
                .map_err(|error| error.utf8_error()),
        }
        .map_err(|_| self.error("a string that is not UTF-8"))
    }

    /// The asset path at the cursor: `@path@`, or `@@@path@@@` (closed by
    /// three to five `@`, the last three closing) with `\@@@` unescaped.
    pub(super) fn asset_path(&mut self) -> Result<Cow<'a, str>> {
        if self.peek() != b'@' {
            return Err(self.error("expected an asset path"));
        }
        let open = self.at;
        if self.peek_at(1) == b'@' && self.peek_at(2) == b'@' {
            self.at += 3;
            let start = self.at;
            let mut escaped = false;
            loop {
                if self.at_end() || matches!(self.peek(), b'\n' | b'\r') {
                    self.at = open;
                    return Err(self.error("an unterminated asset path"));
                }
                if self.data[self.at..].starts_with(b"\\@@@") {
                    escaped = true;
                    self.at += 4;
                    continue;
                }
                if self.data[self.at..].starts_with(b"@@@") {
                    // Up to two more @ belong to the path.
                    let mut extra = 0;
                    while extra < 2 && self.peek_at(3 + extra) == b'@' {
                        extra += 1;
                    }
                    self.at += extra;
                    break;
                }
                self.at += 1;
            }
            let raw = &self.data[start..self.at];
            self.at += 3;
            let text = if escaped {
                let mut plain = Vec::with_capacity(raw.len());
                let mut index = 0;
                while index < raw.len() {
                    if raw[index..].starts_with(b"\\@@@") {
                        index += 1;
                        continue;
                    }
                    plain.push(raw[index]);
                    index += 1;
                }
                Cow::Owned(
                    String::from_utf8(plain)
                        .map_err(|_| self.error("an asset path that is not UTF-8"))?,
                )
            } else {
                Cow::Borrowed(
                    std::str::from_utf8(raw)
                        .map_err(|_| self.error("an asset path that is not UTF-8"))?,
                )
            };
            return Ok(text);
        }
        self.at += 1;
        let start = self.at;
        while self.peek() != b'@' {
            if self.at_end() || matches!(self.peek(), b'\n' | b'\r') {
                self.at = open;
                return Err(self.error("an unterminated asset path"));
            }
            self.at += 1;
        }
        let raw = &self.data[start..self.at];
        self.at += 1;
        std::str::from_utf8(raw)
            .map(Cow::Borrowed)
            .map_err(|_| self.error("an asset path that is not UTF-8"))
    }

    /// The path reference `<path>` at the cursor; the empty path for `<>`.
    pub(super) fn path_ref(&mut self) -> Result<Path> {
        if self.peek() != b'<' {
            return Err(self.error("expected a path reference"));
        }
        let open = self.at;
        self.at += 1;
        let start = self.at;
        while self.peek() != b'>' {
            if self.at_end() || self.peek() == b'\n' {
                self.at = open;
                return Err(self.error("an unterminated path reference"));
            }
            self.at += 1;
        }
        let raw = &self.data[start..self.at];
        self.at += 1;
        let text = unescape(raw);
        let text =
            std::str::from_utf8(&text).map_err(|_| self.error("a path that is not UTF-8"))?;
        Path::parse(text).map_err(|error| self.error(error.message))
    }
}

/// `Sdf_EvalQuotedString`'s escapes: `\\ \a \b \f \n \r \t \v`, `\x` with up
/// to two hex digits, up to three octal digits; any other escaped byte is
/// itself.
fn unescape(raw: &[u8]) -> Cow<'_, [u8]> {
    if !raw.contains(&b'\\') {
        return Cow::Borrowed(raw);
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        let byte = raw[index];
        if byte != b'\\' || index + 1 >= raw.len() {
            out.push(byte);
            index += 1;
            continue;
        }
        index += 1;
        let code = raw[index];
        match code {
            b'a' => out.push(7),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'n' => out.push(10),
            b'r' => out.push(13),
            b't' => out.push(9),
            b'v' => out.push(11),
            b'x' => {
                let mut value = 0u32;
                let mut count = 0;
                while count < 2 && index + 1 < raw.len() && raw[index + 1].is_ascii_hexdigit() {
                    index += 1;
                    value = value * 16 + (raw[index] as char).to_digit(16).unwrap_or(0);
                    count += 1;
                }
                out.push(value as u8);
            }
            b'0'..=b'7' => {
                let mut value = 0u32;
                let mut count = 0;
                index -= 1;
                while count < 3 && index + 1 < raw.len() && (b'0'..=b'7').contains(&raw[index + 1])
                {
                    index += 1;
                    value = value * 8 + u32::from(raw[index] - b'0');
                    count += 1;
                }
                out.push(value as u8);
            }
            _ => out.push(code),
        }
        index += 1;
    }
    Cow::Owned(out)
}
