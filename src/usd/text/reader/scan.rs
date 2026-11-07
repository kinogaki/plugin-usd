// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Lexical helpers: the grammar's padding rules (inline, single-line and
//! multi-line, with `#`, `//` and `/* */` comments), statement and list
//! separators, keywords and identifiers.

use super::Reader;
use crate::usd::error::Result;
use crate::usd::sdf::identifier::identifier_len;

/// The grammar's reserved words: an identifier may not be one of them.
const RESERVED: [&str; 56] = [
    "add",
    "AnimationBlock",
    "append",
    "bezier",
    "class",
    "config",
    "connect",
    "curve",
    "custom",
    "customData",
    "default",
    "def",
    "delete",
    "dictionary",
    "displayUnit",
    "doc",
    "held",
    "hermite",
    "inherits",
    "kind",
    "linear",
    "loop",
    "nameChildren",
    "None",
    "none",
    "offset",
    "oscillate",
    "over",
    "payload",
    "permission",
    "post",
    "pre",
    "prefixSubstitutions",
    "prepend",
    "properties",
    "references",
    "relocates",
    "rel",
    "reorder",
    "rootPrims",
    "repeat",
    "reset",
    "scale",
    "sloped",
    "subLayers",
    "suffixSubstitutions",
    "specializes",
    "spline",
    "symmetryArguments",
    "symmetryFunction",
    "timeSamples",
    "uniform",
    "variantSet",
    "variantSets",
    "variants",
    "varying",
];

fn is_reserved(word: &[u8]) -> bool {
    RESERVED.iter().any(|reserved| reserved.as_bytes() == word)
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

impl<'a> Reader<'a> {
    /// The byte at the cursor; 0 at the end.
    pub(super) fn peek(&self) -> u8 {
        self.data.get(self.at).copied().unwrap_or(0)
    }

    pub(super) fn peek_at(&self, offset: usize) -> u8 {
        self.data.get(self.at + offset).copied().unwrap_or(0)
    }

    pub(super) fn at_end(&self) -> bool {
        self.at >= self.data.len()
    }

    /// Skip a comment at the cursor (`single` allows `#` and `//`); whether
    /// one was there.
    fn comment(&mut self, single: bool) -> Result<bool> {
        let byte = self.peek();
        if byte == b'/' && self.peek_at(1) == b'*' {
            let start = self.at;
            self.at += 2;
            loop {
                if self.at_end() {
                    self.at = start;
                    return Err(self.error("an unterminated /* comment"));
                }
                if self.peek() == b'*' && self.peek_at(1) == b'/' {
                    self.at += 2;
                    return Ok(true);
                }
                self.at += 1;
            }
        }
        if single && (byte == b'#' || (byte == b'/' && self.peek_at(1) == b'/')) {
            while !self.at_end() && !matches!(self.peek(), b'\n' | b'\r') {
                self.at += 1;
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Spaces, tabs and `/* */` comments.
    pub(super) fn skip_inline(&mut self) -> Result<()> {
        loop {
            match self.peek() {
                b' ' | b'\t' => self.at += 1,
                _ if !self.comment(false)? => return Ok(()),
                _ => {}
            }
        }
    }

    /// Spaces, tabs and every comment, up to a line end.
    pub(super) fn skip_line(&mut self) -> Result<()> {
        loop {
            match self.peek() {
                b' ' | b'\t' => self.at += 1,
                _ if !self.comment(true)? => return Ok(()),
                _ => {}
            }
        }
    }

    /// Spaces, tabs, line ends and comments.
    pub(super) fn skip_all(&mut self) -> Result<()> {
        loop {
            match self.peek() {
                b' ' | b'\t' | b'\n' | b'\r' => self.at += 1,
                _ if !self.comment(true)? => return Ok(()),
                _ => {}
            }
        }
    }

    /// Consume `byte`, or fail saying what was expected.
    pub(super) fn expect(&mut self, byte: u8, what: &str) -> Result<()> {
        if self.peek() != byte {
            return Err(self.error(what));
        }
        self.at += 1;
        Ok(())
    }

    /// After a statement in a block closed by `close`: a `;` or a line end
    /// separates it from the next; the close may follow directly.
    pub(super) fn statement_end(&mut self, close: u8) -> Result<()> {
        self.skip_line()?;
        match self.peek() {
            b';' | b'\n' | b'\r' => {
                self.at += 1;
                Ok(())
            }
            byte if byte == close => Ok(()),
            _ => Err(self.error("expected a line end or ';' after a statement")),
        }
    }

    /// After a list item closed by `close`: a comma (true when another item
    /// follows), or the close.
    pub(super) fn list_end(&mut self, close: u8) -> Result<bool> {
        self.skip_inline()?;
        if self.peek() == b',' {
            self.at += 1;
            self.skip_all()?;
            return Ok(self.peek() != close);
        }
        self.skip_all()?;
        if self.peek() == close {
            return Ok(false);
        }
        Err(self.error("expected ',' between list items"))
    }

    /// Whether `word` is next as a whole word.
    pub(super) fn sees(&self, word: &str) -> bool {
        let rest = &self.data[self.at.min(self.data.len())..];
        rest.starts_with(word.as_bytes())
            && !rest.get(word.len()).copied().is_some_and(is_name_byte)
    }

    /// Consume `word` when it is next as a whole word.
    pub(super) fn keyword(&mut self, word: &str) -> bool {
        if self.sees(word) {
            self.at += word.len();
            return true;
        }
        false
    }

    /// The identifier (keywords included) at the cursor, consumed; "" if none.
    pub(super) fn base_identifier(&mut self) -> &'a str {
        let width = identifier_len(self.data, self.at);
        let start = self.at;
        self.at += width;
        self.text(start, self.at)
    }

    /// An Identifier: identifiers that are not keywords, joined by `::`.
    pub(super) fn identifier(&mut self) -> Result<&'a str> {
        let start = self.at;
        loop {
            let width = identifier_len(self.data, self.at);
            if width == 0 || is_reserved(&self.data[self.at..self.at + width]) {
                self.at = start;
                return Err(self.error("expected an identifier"));
            }
            self.at += width;
            if self.peek() == b':' && self.peek_at(1) == b':' {
                self.at += 2;
                continue;
            }
            return Ok(self.text(start, self.at));
        }
    }

    /// Whether an Identifier (not a keyword) starts at the cursor.
    pub(super) fn sees_identifier(&self) -> bool {
        let width = identifier_len(self.data, self.at);
        width > 0 && !is_reserved(&self.data[self.at..self.at + width])
    }

    /// A property name: identifiers joined by `:` (keywords allowed).
    pub(super) fn namespaced_name(&mut self) -> Result<&'a str> {
        let start = self.at;
        let first = identifier_len(self.data, self.at);
        if first == 0 {
            return Err(self.error("expected a property name"));
        }
        self.at += first;
        while self.peek() == b':' {
            let next = identifier_len(self.data, self.at + 1);
            if next == 0 {
                break;
            }
            self.at += 1 + next;
        }
        Ok(self.text(start, self.at))
    }

    /// `[]` after a type name (inline padding inside), consumed; whether seen.
    pub(super) fn array_brackets(&mut self) -> Result<bool> {
        let start = self.at;
        self.skip_inline()?;
        if self.peek() != b'[' {
            self.at = start;
            return Ok(false);
        }
        self.at += 1;
        self.skip_inline()?;
        if self.peek() != b']' {
            self.at = start;
            return Ok(false);
        }
        self.at += 1;
        Ok(true)
    }

    /// `=` with inline padding around it.
    pub(super) fn assignment(&mut self) -> Result<()> {
        self.skip_inline()?;
        self.expect(b'=', "expected '='")?;
        self.skip_inline()
    }

    /// The text between two offsets (identifiers are valid UTF-8 by
    /// construction).
    pub(super) fn text(&self, start: usize, end: usize) -> &'a str {
        std::str::from_utf8(&self.data[start..end]).unwrap_or("")
    }
}
