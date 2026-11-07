// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Path text, after `pxr/usd/sdf/pathParser.h`.

use super::{MOST_DEPTH, Path};
use crate::usd::error::{Error, Result};
use crate::usd::sdf::Token;
use crate::usd::sdf::identifier::{continue_len, identifier_len};

impl Path {
    /// Parse path text (`SdfPath`'s grammar); "" is the empty path.
    pub fn parse(text: &str) -> Result<Path> {
        if text.is_empty() {
            return Ok(Path::empty());
        }
        let mut cursor = Cursor {
            data: text.as_bytes(),
            at: 0,
            depth: 0,
        };
        let path = cursor
            .path(true)
            .map_err(|message| Error::invalid(format!("not a path: {text:?}: {message}")))?;
        if cursor.at != text.len() {
            return Err(Error::invalid(format!(
                "not a path: {text:?}: unexpected text after the path"
            )));
        }
        Ok(path)
    }
}

/// The path grammar of `pxr/usd/sdf/pathParser.h`:
///
/// ```text
/// path       = "/" [prim-elts [prop-elts]] | dotdots ["/" path-elts] | "." | path-elts
/// prim-elts  = name { "/" name | selections name } [selections]
/// selections = { "{" set "=" [variant] "}" }+
/// prop-elts  = "." property [ "[" path "]" [rel-attr] | "." mapper-or-expression ]
/// ```
struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
    depth: u32,
}

type Parse<T> = std::result::Result<T, &'static str>;

impl Cursor<'_> {
    fn peek(&self) -> u8 {
        self.data.get(self.at).copied().unwrap_or(0)
    }

    fn peek_at(&self, offset: usize) -> u8 {
        self.data.get(self.at + offset).copied().unwrap_or(0)
    }

    fn blanks(&mut self) {
        while matches!(self.peek(), b' ' | b'\t') {
            self.at += 1;
        }
    }

    fn keyword(&mut self, word: &str) -> bool {
        if !self.data[self.at..].starts_with(word.as_bytes()) {
            return false;
        }
        if continue_len(self.data, self.at + word.len()) > 0 {
            return false;
        }
        self.at += word.len();
        true
    }

    fn text(&self, start: usize) -> Token {
        // The grammar only accepts UTF-8 identifier characters here.
        Token::new(std::str::from_utf8(&self.data[start..self.at]).unwrap_or(""))
    }

    fn path(&mut self, whole: bool) -> Parse<Path> {
        self.depth += 1;
        if self.depth > 64 {
            return Err("target paths nest too deeply");
        }
        let result = self.path_inner(whole);
        self.depth -= 1;
        result
    }

    fn path_inner(&mut self, whole: bool) -> Parse<Path> {
        if self.peek() == b'/' {
            self.at += 1;
            if identifier_len(self.data, self.at) > 0 {
                let prim = self.prim_elements(Path::absolute_root())?;
                return self.property_elements(prim);
            }
            return Ok(Path::absolute_root());
        }
        if self.peek() == b'.' && self.peek_at(1) == b'.' {
            let mut at = Path::relative_root();
            loop {
                if !(self.peek() == b'.' && self.peek_at(1) == b'.') {
                    return Err("expected '..'");
                }
                self.at += 2;
                at = checked(at.child(".."))?;
                if self.peek() == b'/' && self.peek_at(1) == b'.' && self.peek_at(2) == b'.' {
                    self.at += 1;
                    continue;
                }
                break;
            }
            if self.peek() == b'/' {
                self.at += 1;
                return self.path_elements(at);
            }
            return Ok(at);
        }
        if whole && self.data.len() - self.at == 1 && self.peek() == b'.' {
            self.at += 1;
            return Ok(Path::relative_root());
        }
        self.path_elements(Path::relative_root())
    }

    fn path_elements(&mut self, parent: Path) -> Parse<Path> {
        if identifier_len(self.data, self.at) > 0 {
            let prim = self.prim_elements(parent)?;
            return self.property_elements(prim);
        }
        if self.peek() != b'.' {
            return Err("expected a prim or property name");
        }
        self.property_elements(parent)
    }

    fn prim_elements(&mut self, parent: Path) -> Parse<Path> {
        let mut at = checked(parent.child(self.identifier()?))?;
        loop {
            match self.peek() {
                b'/' => {
                    self.at += 1;
                    if identifier_len(self.data, self.at) == 0 {
                        return Err("expected a prim name after '/'");
                    }
                    at = checked(at.child(self.identifier()?))?;
                }
                b'{' => {
                    at = self.selections(at)?;
                    if identifier_len(self.data, self.at) == 0 {
                        return Ok(at);
                    }
                    at = checked(at.child(self.identifier()?))?;
                }
                _ => return Ok(at),
            }
        }
    }

    fn selections(&mut self, parent: Path) -> Parse<Path> {
        let mut at = parent;
        while self.peek() == b'{' {
            self.at += 1;
            self.blanks();
            let set_start = self.at;
            let first = identifier_len(self.data, self.at);
            if first == 0 {
                return Err("expected a variant set name");
            }
            self.at += first;
            loop {
                if self.peek() == b'-' {
                    self.at += 1;
                    continue;
                }
                let width = continue_len(self.data, self.at);
                if width == 0 {
                    break;
                }
                self.at += width;
            }
            let set = self.text(set_start);
            self.blanks();
            if self.peek() != b'=' {
                return Err("expected '=' in a variant selection");
            }
            self.at += 1;
            self.blanks();
            let name_start = self.at;
            if self.peek() == b'.' {
                self.at += 1;
            }
            loop {
                if matches!(self.peek(), b'|' | b'-') {
                    self.at += 1;
                    continue;
                }
                let width = continue_len(self.data, self.at);
                if width == 0 {
                    break;
                }
                self.at += width;
            }
            let variant = self.text(name_start);
            self.blanks();
            if self.peek() != b'}' {
                return Err("expected '}' after a variant selection");
            }
            self.at += 1;
            at = checked(at.variant(set, variant))?;
        }
        Ok(at)
    }

    fn property_elements(&mut self, parent: Path) -> Parse<Path> {
        if self.peek() != b'.' {
            return Ok(parent);
        }
        self.at += 1;
        let property = checked(parent.property(self.property_name()?))?;
        if self.peek() == b'[' {
            let target = checked(property.target(self.bracketed()?))?;
            if self.peek() != b'.' {
                return Ok(target);
            }
            self.at += 1;
            let relational = checked(target.relational(self.property_name()?))?;
            if self.peek() == b'[' {
                return checked(relational.target(self.bracketed()?));
            }
            if self.peek() == b'.' {
                self.at += 1;
                return self.mapper_or_expression(relational);
            }
            return Ok(relational);
        }
        if self.peek() == b'.' {
            self.at += 1;
            return self.mapper_or_expression(property);
        }
        Ok(property)
    }

    fn mapper_or_expression(&mut self, parent: Path) -> Parse<Path> {
        if self.keyword("expression") {
            return checked(parent.expression());
        }
        if !self.keyword("mapper") {
            return Err("expected 'mapper' or 'expression'");
        }
        let mapper = checked(parent.mapper(self.bracketed()?))?;
        if self.peek() != b'.' {
            return Ok(mapper);
        }
        self.at += 1;
        let start = self.at;
        if !(self.peek().is_ascii_alphabetic() || self.peek() == b'_') {
            return Err("expected a mapper argument name");
        }
        while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
            self.at += 1;
        }
        checked(mapper.mapper_arg(self.text(start)))
    }

    fn bracketed(&mut self) -> Parse<Path> {
        if self.peek() != b'[' {
            return Err("expected '['");
        }
        self.at += 1;
        let path = self.path(false)?;
        if self.peek() != b']' {
            return Err("expected ']'");
        }
        self.at += 1;
        Ok(path)
    }

    fn property_name(&mut self) -> Parse<Token> {
        let start = self.at;
        let first = identifier_len(self.data, self.at);
        if first == 0 {
            return Err("expected a property name");
        }
        self.at += first;
        while self.peek() == b':' {
            let next = identifier_len(self.data, self.at + 1);
            if next == 0 {
                return Err("expected a name after ':'");
            }
            self.at += 1 + next;
        }
        Ok(self.text(start))
    }

    fn identifier(&mut self) -> Parse<Token> {
        let start = self.at;
        let width = identifier_len(self.data, self.at);
        if width == 0 {
            return Err("expected a name");
        }
        self.at += width;
        Ok(self.text(start))
    }
}

fn checked(path: Path) -> Parse<Path> {
    if path.depth() > MOST_DEPTH {
        return Err("a path deeper than 4096 elements");
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usd::sdf::PathKind;

    fn p(text: &str) -> Path {
        Path::parse(text).unwrap()
    }

    #[test]
    fn parses_and_prints_every_element() {
        for text in [
            "/",
            ".",
            "/A",
            "/A/B",
            "/A{v=x}B",
            "/A{v=x}{w=}.attr",
            "/A.rel[/B.c]",
            "/A.rel[/B].ra",
            "/A.a:b:c",
            "../A",
            "../..",
            "../.prop",
            "A/B.c",
            ".c",
            "/A.p.mapper[/B.q].arg",
            "/A.p.expression",
            "/Grün",
        ] {
            assert_eq!(p(text).text(), text, "{text}");
        }
        assert_eq!(p("/A{ v = x }").text(), "/A{v=x}");
        assert!(p("").is_empty());
        for bad in ["//", "/A/", "/A.", "/A{v}", "/9", "/A.b[", "A..b", "/A b"] {
            assert!(Path::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn structure_queries() {
        let path = p("/A{v=x}B.rel[/C].ra");
        assert_eq!(path.kind(), PathKind::Relational);
        assert_eq!(path.parent().text(), "/A{v=x}B.rel[/C]");
        assert_eq!(path.prim_path().text(), "/A{v=x}B");
        assert!(path.has_variant_selection());
        assert_eq!(path.strip_variants().text(), "/A/B.rel[/C].ra");
        assert_eq!(p("/A").child("B").property("x"), p("/A/B.x"));
        assert!(p("/A/B.x").has_prefix(&p("/A")));
        assert!(!p("/AB").has_prefix(&p("/A")));
        assert_eq!(
            p("/A/B.x").replace_prefix(&p("/A"), &p("/Z/Y")),
            p("/Z/Y/B.x")
        );
        assert_eq!(
            p("/R.rel[/A/B]").replace_prefix(&p("/A"), &p("/Q")),
            p("/R.rel[/Q/B]")
        );
        assert_eq!(p("../C.x").make_absolute(&p("/A/B")).unwrap(), p("/A/C.x"));
        assert!(p("../../..").make_absolute(&p("/A")).is_err());
        assert_eq!(
            p("/A/C.x").make_relative(&p("/A/B")).unwrap().text(),
            "../C.x"
        );
        assert_eq!(p("/A/B/C").make_relative(&p("/A")).unwrap().text(), "B/C");
    }

    #[test]
    fn order_is_sdf_order() {
        let mut paths = [p("/B"), p("/A.x"), p("/A/C"), p("/A"), p("x"), p("/A.a")];
        paths.sort();
        let texts: Vec<_> = paths.iter().map(Path::text).collect();
        assert_eq!(texts, ["/A", "/A.a", "/A.x", "/A/C", "/B", "x"]);
    }
}
