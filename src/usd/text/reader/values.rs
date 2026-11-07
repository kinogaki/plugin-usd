// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Values of a known type, as `parserHelpers.cpp` builds them: a scalar is
//! one atom, a vector or quaternion a tuple of atoms, a matrix a tuple of
//! row tuples; an array is a list of those. Numbers convert as its `_GetImpl`
//! does: integers range-checked, doubles narrowed to float and half, bools
//! from numbers (non-zero) or true/false/yes/no/1/0 text. Quaternions are
//! written `(real, i, j, k)` and stored imaginary first.
//!
//! Also dictionaries, and the recorded text of values of unknown type.

use std::sync::Arc;

use super::Reader;
use super::atoms::Number;
use crate::usd::error::Result;
use crate::usd::sdf::{
    Array, Dictionary, Half, Token, Value, ValueKind, attribute_kinds, find_type,
};
use crate::usd::text::number::{write_f64, write_integer, write_unsigned};
use crate::usd::text::quote::quote_into;

/// A scalar component converted from a number literal.
pub(super) trait Scalar: Sized + Copy + Default {
    fn from_number(reader: &Reader<'_>, number: Number) -> Result<Self>;
}

fn integer(reader: &Reader<'_>, number: Number, low: i64, high: i64) -> Result<i64> {
    let value = match number {
        Number::Unsigned(value) => {
            i64::try_from(value).map_err(|_| reader.error("an integer out of range"))?
        }
        Number::Signed(value) => value,
        Number::Real(value) => {
            if value.is_nan()
                || !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&value)
            {
                return Err(reader.error("an integer out of range"));
            }
            value as i64
        }
    };
    if value < low || value > high {
        return Err(reader.error("an integer out of range"));
    }
    Ok(value)
}

macro_rules! integer_scalar {
    ($($t:ty),*) => {$(
        impl Scalar for $t {
            fn from_number(reader: &Reader<'_>, number: Number) -> Result<$t> {
                Ok(integer(reader, number, <$t>::MIN as i64, <$t>::MAX as i64)? as $t)
            }
        }
    )*};
}
integer_scalar!(i32, u32, u8, i64);

impl Scalar for u64 {
    fn from_number(reader: &Reader<'_>, number: Number) -> Result<u64> {
        if let Number::Unsigned(value) = number {
            return Ok(value);
        }
        Ok(integer(reader, number, 0, i64::MAX)? as u64)
    }
}

impl Scalar for f64 {
    fn from_number(_: &Reader<'_>, number: Number) -> Result<f64> {
        Ok(number.as_f64())
    }
}

impl Scalar for f32 {
    fn from_number(_: &Reader<'_>, number: Number) -> Result<f32> {
        Ok(number.as_f64() as f32)
    }
}

impl Scalar for Half {
    fn from_number(_: &Reader<'_>, number: Number) -> Result<Half> {
        Ok(Half::from_f32(number.as_f64() as f32))
    }
}

/// One element of an attribute value, read as its kind spells it.
pub(super) trait Element: Sized {
    fn read(reader: &mut Reader<'_>, kind: ValueKind) -> Result<Self>;
}

macro_rules! scalar_element {
    ($($t:ty),*) => {$(
        impl Element for $t {
            fn read(reader: &mut Reader<'_>, _: ValueKind) -> Result<$t> {
                let number = reader.number_atom()?;
                <$t>::from_number(reader, number)
            }
        }
    )*};
}
scalar_element!(i32, u32, u8, i64, u64, f32, f64, Half);

impl Element for bool {
    fn read(reader: &mut Reader<'_>, _: ValueKind) -> Result<bool> {
        reader.bool_atom()
    }
}

impl Element for Token {
    fn read(reader: &mut Reader<'_>, kind: ValueKind) -> Result<Token> {
        if kind == ValueKind::Asset {
            if reader.peek() == b'@' {
                return Ok(Token::new(&reader.asset_path()?));
            }
            if reader.sees_string() {
                return Ok(Token::new(&reader.string()?));
            }
            return Err(reader.error("expected an asset path"));
        }
        if !reader.sees_string() {
            return Err(reader.error("expected a quoted string"));
        }
        Ok(Token::new(&reader.string()?))
    }
}

impl<S: Scalar, const N: usize> Element for [S; N] {
    fn read(reader: &mut Reader<'_>, kind: ValueKind) -> Result<[S; N]> {
        let mut atoms = [Number::Unsigned(0); 16];
        reader.expect(b'(', "expected '(' to start a tuple")?;
        reader.skip_all()?;
        let rows = kind.matrix_rows();
        if rows > 0 {
            for row in 0..rows {
                reader.expect(b'(', "expected '(' to start a matrix row")?;
                reader.skip_all()?;
                for column in 0..rows {
                    atoms[row * rows + column] = reader.number_atom()?;
                    if reader.list_end(b')')? != (column + 1 < rows) {
                        return Err(reader.error("a matrix row has the wrong size"));
                    }
                }
                reader.expect(b')', "expected ')' to end a matrix row")?;
                if reader.list_end(b')')? != (row + 1 < rows) {
                    return Err(reader.error("a matrix has the wrong number of rows"));
                }
            }
        } else {
            let mut taken = 0;
            loop {
                if taken == N {
                    return Err(reader.error("a tuple has too many values"));
                }
                atoms[taken] = reader.number_atom()?;
                taken += 1;
                if !reader.list_end(b')')? {
                    break;
                }
            }
            if taken != N {
                return Err(reader.error("a tuple has too few values"));
            }
        }
        reader.expect(b')', "expected ')' to end a tuple")?;
        let mut out = [S::default(); N];
        let quaternion = kind.is_quaternion();
        for (index, slot) in out.iter_mut().enumerate() {
            // Text is (real, i, j, k); memory is i, j, k, real.
            let source = if quaternion { (index + 1) % 4 } else { index };
            *slot = S::from_number(reader, atoms[source])?;
        }
        Ok(out)
    }
}

macro_rules! define_typed {
    ($(($name:ident, $element:ty)),* $(,)?) => {
        impl Reader<'_> {
            /// A value of `kind`, an array when `array` (the TypedValue rule).
            pub(super) fn typed_value(&mut self, kind: ValueKind, array: bool) -> Result<Value> {
                if array {
                    if let Some(value) = self.fast_array(kind)? {
                        return Ok(value);
                    }
                }
                match kind {
                    $(ValueKind::$name => Ok(if array {
                        Value::Array(Array::$name(Arc::new(self.array_of::<$element>(kind)?)))
                    } else {
                        Value::$name(<$element>::read(self, kind)?)
                    }),)*
                    _ => Err(self.error("not a value type")),
                }
            }
        }
    };
}
attribute_kinds!(define_typed);

impl<'a> Reader<'a> {
    /// `[` elements `]` (or `[]`).
    fn array_of<T: Element>(&mut self, kind: ValueKind) -> Result<Vec<T>> {
        self.expect(b'[', "expected '[' to start an array")?;
        self.skip_all()?;
        let mut items = Vec::new();
        if self.peek() != b']' {
            loop {
                items.push(T::read(self, kind)?);
                if !self.list_end(b']')? {
                    break;
                }
            }
        }
        self.expect(b']', "expected ']' to end an array")?;
        Ok(items)
    }

    /// A bool: a number (non-zero is true) or true/false/yes/no/1/0 as an
    /// identifier or a string (`Sdf_BoolFromString`, case-insensitive).
    pub(super) fn bool_atom(&mut self) -> Result<bool> {
        if self.sees_number() {
            return Ok(match self.number()? {
                Number::Unsigned(value) => value != 0,
                Number::Signed(value) => value != 0,
                Number::Real(value) => value != 0.0,
            });
        }
        let word = if self.sees_string() {
            self.string()?.into_owned()
        } else {
            self.base_identifier().to_owned()
        };
        if word.eq_ignore_ascii_case("true") || word.eq_ignore_ascii_case("yes") || word == "1" {
            return Ok(true);
        }
        if word.eq_ignore_ascii_case("false") || word.eq_ignore_ascii_case("no") || word == "0" {
            return Ok(false);
        }
        Err(self.error("expected a bool"))
    }

    /// A number atom; `inf`, `-inf` and `nan` may also be strings.
    pub(super) fn number_atom(&mut self) -> Result<Number> {
        if self.sees_number() {
            return self.number();
        }
        if self.sees_string() {
            let at = self.at;
            match &*self.string()? {
                "inf" => return Ok(Number::Real(f64::INFINITY)),
                "-inf" => return Ok(Number::Real(f64::NEG_INFINITY)),
                "nan" => return Ok(Number::Real(f64::NAN)),
                _ => self.at = at,
            }
        }
        Err(self.error("expected a number"))
    }

    /// `{ type key = value ... }`: a dictionary, nested with
    /// `dictionary key = {...}`.
    pub(super) fn dictionary_value(&mut self) -> Result<Dictionary> {
        self.enter()?;
        self.expect(b'{', "expected '{' to start a dictionary")?;
        let mut dictionary = Dictionary::new();
        self.skip_all()?;
        while self.peek() != b'}' {
            if self.keyword("dictionary") {
                self.skip_inline()?;
                let key = self.dictionary_key()?;
                self.assignment()?;
                let value = self.dictionary_value()?;
                dictionary.insert(key, Value::Dictionary(Arc::new(value)));
            } else {
                let type_text = self.identifier()?;
                let array = self.array_brackets()?;
                let kind = match find_type(type_text) {
                    Some((entry, _)) => entry.kind,
                    None => {
                        return Err(self.error(format!(
                            "an unknown value type '{type_text}' in a dictionary"
                        )));
                    }
                };
                let Some(kind) = kind else {
                    return Err(self.error("a value type a dictionary cannot hold"));
                };
                self.skip_inline()?;
                let key = self.dictionary_key()?;
                self.assignment()?;
                let value = self.typed_value(kind, array)?;
                dictionary.insert(key, value);
            }
            self.statement_end(b'}')?;
            self.skip_all()?;
        }
        self.at += 1;
        self.leave();
        Ok(dictionary)
    }

    /// A dictionary key: a quoted string or an identifier (keywords too).
    fn dictionary_key(&mut self) -> Result<Token> {
        if self.sees_string() {
            return Ok(Token::new(&self.string()?));
        }
        let word = self.base_identifier();
        if word.is_empty() {
            return Err(self.error("expected a dictionary key"));
        }
        Ok(Token::new(word))
    }

    /// The text of a value of unknown type, as Sdf records it: atoms
    /// re-printed (numbers as `TfStringify` gives them, strings quoted),
    /// lists and tuples with ", " between items.
    pub(super) fn recorded_value(&mut self) -> Result<Token> {
        let mut out = Vec::new();
        if self.keyword("None") {
            return Ok(Token::new("None"));
        }
        self.recorded_item(&mut out)?;
        Ok(Token::from(
            String::from_utf8(out).map_err(|_| self.error("a value that is not UTF-8"))?,
        ))
    }

    pub(super) fn recorded_item(&mut self, out: &mut Vec<u8>) -> Result<()> {
        let byte = self.peek();
        if byte == b'[' || byte == b'(' {
            self.enter()?;
            let close = if byte == b'[' { b']' } else { b')' };
            self.at += 1;
            out.push(byte);
            self.skip_all()?;
            let mut first = true;
            while self.peek() != close {
                if !first {
                    out.extend_from_slice(b", ");
                }
                first = false;
                self.recorded_item(out)?;
                if !self.list_end(close)? {
                    break;
                }
            }
            self.expect(close, "expected the end of a list or tuple")?;
            out.push(close);
            self.leave();
            return Ok(());
        }
        if self.sees_number() {
            match self.number()? {
                Number::Unsigned(value) => write_unsigned(out, value),
                Number::Signed(value) => write_integer(out, value),
                Number::Real(value) => write_f64(out, value),
            }
            return Ok(());
        }
        if self.sees_string() {
            let text = self.string()?;
            quote_into(out, &text, true);
            return Ok(());
        }
        if byte == b'@' {
            let text = self.asset_path()?;
            out.push(b'@');
            out.extend_from_slice(text.as_bytes());
            out.push(b'@');
            return Ok(());
        }
        if byte == b'<' {
            let start = self.at;
            self.path_ref()?;
            out.extend_from_slice(&self.data[start..self.at]);
            return Ok(());
        }
        let word = self.identifier()?;
        out.extend_from_slice(word.as_bytes());
        Ok(())
    }

    /// `{ "key": "value", ... }` into a dictionary of strings.
    pub(super) fn string_dictionary(&mut self) -> Result<Dictionary> {
        self.expect(b'{', "expected '{'")?;
        let mut dictionary = Dictionary::new();
        self.skip_all()?;
        if self.peek() != b'}' {
            loop {
                let key = Token::new(&self.string()?);
                self.skip_inline()?;
                self.expect(b':', "expected ':' in a string dictionary")?;
                self.skip_inline()?;
                let text = Token::new(&self.string()?);
                dictionary.insert(key, Value::String(text));
                if !self.list_end(b'}')? {
                    break;
                }
            }
        }
        self.expect(b'}', "expected '}'")?;
        Ok(dictionary)
    }
}
