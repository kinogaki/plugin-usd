// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Variable expressions, after `SdfVariableExpression`
//! (`pxr/usd/sdf/variableExpressionParser.cpp`, `variableExpressionImpl.cpp`):
//! a backquoted expression over a layer stack's expression variables, as
//! sublayer, reference and payload asset paths and variant selections may
//! be. An expression is a variable `${NAME}`, a string (`"..."` or `'...'`,
//! with `${NAME}` substituted inside), an integer, a boolean, None, a list
//! of those in brackets, or a call of if, eq, neq, lt, leq, gt, geq, and, or,
//! not, contains, at, len or defined (`matches_regex` fails: there is no
//! regular expression engine here). A variable whose value is itself an
//! expression is evaluated in turn, once however often it is named;
//! circular ones fail, and so does a string longer than 4096 bytes.

use std::collections::HashMap;

use crate::usd::sdf::{Array, Dictionary, Token, Value};

/// An expression's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Evaluated {
    /// `None`.
    Nothing,
    /// A string.
    String(Token),
    /// A boolean.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A list of strings.
    Strings(Vec<Token>),
    /// A list of booleans.
    Bools(Vec<bool>),
    /// A list of integers.
    Ints(Vec<i64>),
    /// `[]`.
    EmptyList,
    /// Evaluation failed (a type error, an unknown variable, ...).
    Failed,
}

impl Evaluated {
    fn is_list(&self) -> bool {
        matches!(
            self,
            Evaluated::Strings(_) | Evaluated::Bools(_) | Evaluated::Ints(_) | Evaluated::EmptyList
        )
    }

    fn same_type(&self, other: &Evaluated) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// Whether `text` is an expression: backquoted.
pub fn is_expression(text: &str) -> bool {
    text.len() >= 2 && text.starts_with('`') && text.ends_with('`')
}

/// Evaluate an expression with `variables`.
pub fn evaluate(expression: &str, variables: &Dictionary) -> Evaluated {
    let mut evaluator = Evaluator {
        variables,
        stack: Vec::new(),
        evaluated: HashMap::new(),
    };
    evaluator.evaluate(expression)
}

/// The string an expression evaluates to; none when it fails or is not a
/// string (`Pcp_EvaluateVariableExpression`).
pub fn evaluate_to_string(expression: &str, variables: &Dictionary) -> Option<Token> {
    match evaluate(expression, variables) {
        Evaluated::String(text) if !text.is_empty() => Some(text),
        _ => None,
    }
}

/// The deepest nesting of calls and lists, and of variables naming
/// expressions.
const MOST_DEPTH: usize = 64;

/// The longest string an expression makes: longer than any path, and short
/// enough that variables doubling a string fail before they cost memory.
const MOST_TEXT: usize = 4096;

struct Evaluator<'a> {
    variables: &'a Dictionary,
    /// Variables being evaluated, to catch circular ones.
    stack: Vec<String>,
    /// Each variable's value, once evaluated.
    evaluated: HashMap<String, Evaluated>,
}

struct Parser<'t> {
    text: &'t [u8],
    at: usize,
    /// A syntax error: the whole expression fails.
    broken: bool,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> u8 {
        self.text.get(self.at).copied().unwrap_or(0)
    }

    fn take(&mut self, byte: u8) -> bool {
        if self.peek() != byte {
            return false;
        }
        self.at += 1;
        true
    }

    fn spaces(&mut self) {
        while self.peek() == b' ' {
            self.at += 1;
        }
    }

    fn fail(&mut self) -> Evaluated {
        self.broken = true;
        Evaluated::Failed
    }

    /// A C identifier.
    fn identifier(&mut self) -> String {
        let start = self.at;
        if !(self.peek() == b'_' || self.peek().is_ascii_alphabetic()) {
            return String::new();
        }
        while self.peek() == b'_' || self.peek().is_ascii_alphanumeric() {
            self.at += 1;
        }
        String::from_utf8_lossy(&self.text[start..self.at]).into_owned()
    }

    /// `${NAME}`: the name.
    fn variable(&mut self) -> Option<String> {
        if !self.take(b'$') || !self.take(b'{') {
            return None;
        }
        let name = self.identifier();
        if name.is_empty() || !self.take(b'}') {
            return None;
        }
        Some(name)
    }

    /// An optionally negative decimal integer.
    fn integer(&mut self) -> Option<i64> {
        let negative = self.take(b'-');
        let mut value: i64 = 0;
        let mut digits = 0;
        while self.peek().is_ascii_digit() {
            value = value
                .checked_mul(10)?
                .checked_add(i64::from(self.peek() - b'0'))?;
            self.at += 1;
            digits += 1;
        }
        if digits == 0 {
            return None;
        }
        Some(if negative { -value } else { value })
    }
}

impl Evaluator<'_> {
    fn evaluate(&mut self, expression: &str) -> Evaluated {
        let mut parser = Parser {
            text: expression.as_bytes(),
            at: 0,
            broken: false,
            depth: 0,
        };
        if !parser.take(b'`') {
            return Evaluated::Failed;
        }
        let result = self.body(&mut parser, false);
        if parser.broken || !parser.take(b'`') || parser.at != parser.text.len() {
            return Evaluated::Failed;
        }
        result
    }

    fn body(&mut self, parser: &mut Parser<'_>, skip: bool) -> Evaluated {
        parser.depth += 1;
        if parser.depth > MOST_DEPTH {
            return parser.fail();
        }
        let result = if parser.peek() == b'[' {
            self.list(parser, skip)
        } else {
            self.scalar(parser, skip)
        };
        parser.depth -= 1;
        result
    }

    fn scalar(&mut self, parser: &mut Parser<'_>, skip: bool) -> Evaluated {
        let byte = parser.peek();
        if byte == b'$' {
            let Some(name) = parser.variable() else {
                return parser.fail();
            };
            if skip {
                return Evaluated::Nothing;
            }
            return self.variable(&name);
        }
        if byte == b'"' || byte == b'\'' {
            return self.quoted(parser, byte, skip);
        }
        if byte == b'-' || byte.is_ascii_digit() {
            let Some(value) = parser.integer() else {
                return parser.fail();
            };
            return Evaluated::Int(value);
        }
        let word = parser.identifier();
        match word.as_str() {
            "" => parser.fail(),
            "true" | "True" => Evaluated::Bool(true),
            "false" | "False" => Evaluated::Bool(false),
            "None" | "none" => Evaluated::Nothing,
            _ => self.function(parser, &word, skip),
        }
    }

    /// The value of variable `name` (a failure when it has none), each
    /// evaluated once.
    fn variable(&mut self, name: &str) -> Evaluated {
        if let Some(value) = self.evaluated.get(name) {
            return value.clone();
        }
        let value = self.evaluate_variable(name);
        self.evaluated.insert(name.to_owned(), value.clone());
        value
    }

    fn evaluate_variable(&mut self, name: &str) -> Evaluated {
        if self.stack.iter().any(|held| held == name) || self.stack.len() > MOST_DEPTH {
            return Evaluated::Failed;
        }
        let Some(value) = self.variables.get(name) else {
            return Evaluated::Failed;
        };
        let result = from_value(value);
        let Evaluated::String(text) = &result else {
            return result;
        };
        if !is_expression(text) {
            return result;
        }
        let text = text.clone();
        self.stack.push(name.to_owned());
        let result = self.evaluate(&text);
        self.stack.pop();
        result
    }

    /// A quoted string, with `${NAME}` substituted and escapes processed.
    fn quoted(&mut self, parser: &mut Parser<'_>, quote: u8, skip: bool) -> Evaluated {
        parser.at += 1;
        let mut text = Vec::new();
        let mut failed = false;
        loop {
            if parser.at >= parser.text.len() {
                return parser.fail();
            }
            let byte = parser.text[parser.at];
            if byte == quote {
                parser.at += 1;
                break;
            }
            if byte == b'$' && parser.text.get(parser.at + 1) == Some(&b'{') {
                let Some(name) = parser.variable() else {
                    return parser.fail();
                };
                if skip || failed {
                    continue;
                }
                if !self.variables.contains_key(name.as_str()) {
                    // An unset variable leaves its name (StringNode::Evaluate).
                    text.extend_from_slice(name.as_bytes());
                    continue;
                }
                match self.variable(&name) {
                    Evaluated::String(value) => text.extend_from_slice(value.as_bytes()),
                    Evaluated::Nothing => {}
                    _ => failed = true,
                }
                failed |= text.len() > MOST_TEXT;
                continue;
            }
            if byte == b'\\' && parser.at + 1 < parser.text.len() {
                parser.at += 1;
                unescape(parser, &mut text);
                continue;
            }
            text.push(byte);
            parser.at += 1;
        }
        if skip {
            return Evaluated::Nothing;
        }
        if failed {
            return Evaluated::Failed;
        }
        Evaluated::String(Token::from(String::from_utf8_lossy(&text).into_owned()))
    }

    /// A list of scalars of one type: spaces only around its commas, or
    /// alone in an empty list.
    fn list(&mut self, parser: &mut Parser<'_>, skip: bool) -> Evaluated {
        parser.at += 1;
        if parser.peek() == b' ' || parser.peek() == b']' {
            parser.spaces();
            if parser.take(b']') {
                return Evaluated::EmptyList;
            }
            return parser.fail();
        }
        let mut result = Evaluated::EmptyList;
        let mut failed = false;
        loop {
            let element = self.scalar(parser, skip);
            if parser.broken {
                return Evaluated::Failed;
            }
            if !skip && !failed {
                match (&mut result, element) {
                    (Evaluated::EmptyList, Evaluated::String(text)) => {
                        result = Evaluated::Strings(vec![text])
                    }
                    (Evaluated::EmptyList, Evaluated::Bool(flag)) => {
                        result = Evaluated::Bools(vec![flag])
                    }
                    (Evaluated::EmptyList, Evaluated::Int(value)) => {
                        result = Evaluated::Ints(vec![value])
                    }
                    (Evaluated::Strings(items), Evaluated::String(text)) => items.push(text),
                    (Evaluated::Bools(items), Evaluated::Bool(flag)) => items.push(flag),
                    (Evaluated::Ints(items), Evaluated::Int(value)) => items.push(value),
                    _ => failed = true,
                }
            }
            if parser.take(b']') {
                return if failed { Evaluated::Failed } else { result };
            }
            parser.spaces();
            if !parser.take(b',') {
                return parser.fail();
            }
            parser.spaces();
        }
    }

    /// A function call: its arguments, then the function.
    fn function(&mut self, parser: &mut Parser<'_>, name: &str, skip: bool) -> Evaluated {
        parser.spaces();
        if !parser.take(b'(') {
            return parser.fail();
        }
        parser.spaces();
        let mut arguments = Vec::new();
        let mut done = skip;
        // and/or stop evaluating once the answer is known.
        let short = name == "and" || name == "or";
        if !parser.take(b')') {
            loop {
                let argument = self.body(parser, done);
                if parser.broken {
                    return parser.fail();
                }
                if short
                    && !done
                    && let Evaluated::Bool(flag) = argument
                    && ((name == "and" && !flag) || (name == "or" && flag))
                {
                    done = true;
                }
                arguments.push(argument);
                parser.spaces();
                if parser.take(b')') {
                    break;
                }
                if !parser.take(b',') {
                    return parser.fail();
                }
                parser.spaces();
            }
        }
        parser.spaces();
        // An unknown function or argument count is a syntax error.
        if !known_call(name, arguments.len()) {
            return parser.fail();
        }
        if skip {
            return Evaluated::Nothing;
        }
        self.call(name, &arguments)
    }

    fn call(&self, name: &str, arguments: &[Evaluated]) -> Evaluated {
        if name == "if" {
            let Evaluated::Bool(condition) = arguments[0] else {
                return Evaluated::Failed;
            };
            if arguments.len() == 3
                && !arguments[1].same_type(&arguments[2])
                && arguments[1] != Evaluated::Nothing
                && arguments[2] != Evaluated::Nothing
            {
                return Evaluated::Failed;
            }
            if condition {
                return arguments[1].clone();
            }
            return arguments.get(2).cloned().unwrap_or(Evaluated::Nothing);
        }
        if arguments.contains(&Evaluated::Failed) {
            return Evaluated::Failed;
        }
        match name {
            "eq" | "neq" | "lt" | "leq" | "gt" | "geq" => {
                compare(name, &arguments[0], &arguments[1])
            }
            "and" | "or" => {
                let mut result = false;
                for argument in arguments {
                    let Evaluated::Bool(flag) = *argument else {
                        return Evaluated::Failed;
                    };
                    if name == "and" && !flag {
                        return Evaluated::Bool(false);
                    }
                    if name == "or" && flag {
                        return Evaluated::Bool(true);
                    }
                    result = flag;
                }
                Evaluated::Bool(result)
            }
            "not" => match arguments[0] {
                Evaluated::Bool(flag) => Evaluated::Bool(!flag),
                _ => Evaluated::Failed,
            },
            "contains" => contains(&arguments[0], &arguments[1]),
            "at" => match arguments[1] {
                Evaluated::Int(index) => element_at(&arguments[0], index),
                _ => Evaluated::Failed,
            },
            "len" => match &arguments[0] {
                Evaluated::String(text) => Evaluated::Int(text.len() as i64),
                Evaluated::Strings(items) => Evaluated::Int(items.len() as i64),
                Evaluated::Bools(items) => Evaluated::Int(items.len() as i64),
                Evaluated::Ints(items) => Evaluated::Int(items.len() as i64),
                Evaluated::EmptyList => Evaluated::Int(0),
                _ => Evaluated::Failed,
            },
            "defined" => {
                let mut all = true;
                for argument in arguments {
                    let Evaluated::String(text) = argument else {
                        return Evaluated::Failed;
                    };
                    all = all && self.variables.contains_key(text.as_str());
                }
                Evaluated::Bool(all)
            }
            _ => Evaluated::Failed,
        }
    }
}

/// A backslash escape (`TfEscapeStringReplaceChar`); the parser is past the
/// backslash.
fn unescape(parser: &mut Parser<'_>, out: &mut Vec<u8>) {
    let byte = parser.peek();
    parser.at += 1;
    match byte {
        b'a' => out.push(7),
        b'b' => out.push(8),
        b'f' => out.push(12),
        b'n' => out.push(10),
        b'r' => out.push(13),
        b't' => out.push(9),
        b'v' => out.push(11),
        b'x' => {
            let mut value = 0u8;
            let mut count = 0;
            while count < 2 && parser.peek().is_ascii_hexdigit() {
                value = value
                    .wrapping_mul(16)
                    .wrapping_add((parser.peek() as char).to_digit(16).unwrap_or(0) as u8);
                parser.at += 1;
                count += 1;
            }
            out.push(value);
        }
        b'0'..=b'7' => {
            let mut value = byte - b'0';
            let mut count = 1;
            while count < 3 && (b'0'..=b'7').contains(&parser.peek()) {
                value = value.wrapping_mul(8).wrapping_add(parser.peek() - b'0');
                parser.at += 1;
                count += 1;
            }
            out.push(value);
        }
        _ => out.push(byte),
    }
}

/// Whether a function exists with that many arguments.
fn known_call(name: &str, count: usize) -> bool {
    match name {
        "if" => count == 2 || count == 3,
        "and" | "or" => count >= 2,
        "defined" => count >= 1,
        "not" | "len" => count == 1,
        "eq" | "neq" | "lt" | "leq" | "gt" | "geq" | "contains" | "matches_regex" | "at" => {
            count == 2
        }
        _ => false,
    }
}

/// An expression variable's value as an expression value: strings,
/// booleans and integers, and arrays of them.
fn from_value(value: &Value) -> Evaluated {
    match value {
        Value::String(text) | Value::Token(text) => Evaluated::String(text.clone()),
        Value::Bool(flag) => Evaluated::Bool(*flag),
        Value::Int(v) => Evaluated::Int(i64::from(*v)),
        Value::Int64(v) => Evaluated::Int(*v),
        Value::UInt(v) => Evaluated::Int(i64::from(*v)),
        Value::UInt64(v) => Evaluated::Int(*v as i64),
        Value::UChar(v) => Evaluated::Int(i64::from(*v)),
        Value::Array(array) if array.is_empty() => Evaluated::EmptyList,
        Value::Array(Array::String(items) | Array::Token(items)) => {
            Evaluated::Strings(items.to_vec())
        }
        Value::Array(Array::Bool(items)) => Evaluated::Bools(items.to_vec()),
        Value::Array(Array::Int(items)) => {
            Evaluated::Ints(items.iter().map(|&v| i64::from(v)).collect())
        }
        Value::StringVector(items) | Value::TokenVector(items) => {
            if items.is_empty() {
                Evaluated::EmptyList
            } else {
                Evaluated::Strings(items.to_vec())
            }
        }
        _ => Evaluated::Failed,
    }
}

/// eq, neq, lt, leq, gt and geq of two scalars of one type (None equals
/// None and is otherwise not comparable).
fn compare(name: &str, a: &Evaluated, b: &Evaluated) -> Evaluated {
    if !a.same_type(b) || a.is_list() {
        return Evaluated::Failed;
    }
    let order = match (a, b) {
        (Evaluated::Nothing, _) => {
            return match name {
                "eq" => Evaluated::Bool(true),
                "neq" => Evaluated::Bool(false),
                _ => Evaluated::Failed,
            };
        }
        (Evaluated::String(x), Evaluated::String(y)) => x.as_bytes().cmp(y.as_bytes()),
        (Evaluated::Int(x), Evaluated::Int(y)) => x.cmp(y),
        (Evaluated::Bool(x), Evaluated::Bool(y)) => x.cmp(y),
        _ => return Evaluated::Failed,
    };
    use std::cmp::Ordering::*;
    Evaluated::Bool(match name {
        "eq" => order == Equal,
        "neq" => order != Equal,
        "lt" => order == Less,
        "leq" => order != Greater,
        "gt" => order == Greater,
        _ => order != Less,
    })
}

/// Whether a string holds a substring, or a list an element.
fn contains(within: &Evaluated, part: &Evaluated) -> Evaluated {
    match (within, part) {
        (Evaluated::EmptyList, _) => Evaluated::Bool(false),
        (Evaluated::String(text), Evaluated::String(wanted)) => {
            Evaluated::Bool(text.contains(wanted.as_str()))
        }
        (Evaluated::Strings(items), Evaluated::String(wanted)) => {
            Evaluated::Bool(items.contains(wanted))
        }
        (Evaluated::Bools(items), Evaluated::Bool(wanted)) => {
            Evaluated::Bool(items.contains(wanted))
        }
        (Evaluated::Ints(items), Evaluated::Int(wanted)) => Evaluated::Bool(items.contains(wanted)),
        _ => Evaluated::Failed,
    }
}

/// Element `index` of a list or string (negative from the end).
fn element_at(source: &Evaluated, index: i64) -> Evaluated {
    let size = match source {
        Evaluated::String(text) => text.len(),
        Evaluated::Strings(items) => items.len(),
        Evaluated::Bools(items) => items.len(),
        Evaluated::Ints(items) => items.len(),
        _ => return Evaluated::Failed,
    } as i64;
    let position = if index < 0 { index + size } else { index };
    if position < 0 || position >= size {
        return Evaluated::Failed;
    }
    let at = position as usize;
    match source {
        Evaluated::String(text) => match text.as_bytes().get(at) {
            Some(&byte) => {
                Evaluated::String(Token::from(String::from_utf8_lossy(&[byte]).into_owned()))
            }
            None => Evaluated::Failed,
        },
        Evaluated::Strings(items) => Evaluated::String(items[at].clone()),
        Evaluated::Bools(items) => Evaluated::Bool(items[at]),
        Evaluated::Ints(items) => Evaluated::Int(items[at]),
        _ => Evaluated::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn variables() -> Dictionary {
        let mut variables = Dictionary::new();
        variables.insert(Token::new("NAME"), Value::String(Token::new("chair")));
        variables.insert(Token::new("COUNT"), Value::Int64(3));
        variables.insert(
            Token::new("NESTED"),
            Value::String(Token::new("`\"${NAME}_x\"`")),
        );
        variables.insert(Token::new("LOOP"), Value::String(Token::new("`${LOOP}`")));
        variables.insert(
            Token::new("LIST"),
            Value::Array(Array::String(Arc::new(vec![
                Token::new("a"),
                Token::new("b"),
            ]))),
        );
        variables
    }

    #[test]
    fn evaluates_expressions() {
        let v = variables();
        assert_eq!(
            evaluate("`\"${NAME}.usda\"`", &v),
            Evaluated::String(Token::new("chair.usda"))
        );
        assert_eq!(
            evaluate("`${NESTED}`", &v),
            Evaluated::String(Token::new("chair_x"))
        );
        assert_eq!(evaluate("`${LOOP}`", &v), Evaluated::Failed);
        assert_eq!(
            evaluate("`if(eq(${COUNT}, 3), \"y\", \"n\")`", &v),
            Evaluated::String(Token::new("y"))
        );
        assert_eq!(
            evaluate("`contains(${LIST}, \"b\")`", &v),
            Evaluated::Bool(true)
        );
        assert_eq!(
            evaluate("`and(false, ${MISSING})`", &v),
            Evaluated::Bool(false)
        );
        assert_eq!(evaluate("`len([1, 2, 3])`", &v), Evaluated::Int(3));
        assert_eq!(
            evaluate("`at(\"abc\", -1)`", &v),
            Evaluated::String(Token::new("c"))
        );
        assert_eq!(evaluate("`nope(1)`", &v), Evaluated::Failed);
        assert_eq!(
            evaluate("`\"${UNSET}\"`", &v),
            Evaluated::String(Token::new("UNSET"))
        );
        assert!(!is_expression("plain"));
    }
}
