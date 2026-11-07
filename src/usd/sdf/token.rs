// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Shared immutable strings: names, keys, tokens and string values.
//!
//! OpenUSD interns every name as a `TfToken`; here a [`Token`] is a
//! reference-counted string, so copies are a counter bump and values can be
//! shared between layers and threads without a global table.

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::sync::{Arc, LazyLock};

/// An immutable, cheaply cloned string.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Token(Arc<str>);

/// The shared empty token, so empty values allocate nothing.
static EMPTY: LazyLock<Token> = LazyLock::new(|| Token(Arc::from("")));

impl Token {
    /// A token holding `text`.
    pub fn new(text: &str) -> Self {
        if text.is_empty() {
            return Self::empty();
        }
        Token(Arc::from(text))
    }

    /// The empty token.
    pub fn empty() -> Self {
        EMPTY.clone()
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for Token {
    fn default() -> Self {
        Self::empty()
    }
}

impl Deref for Token {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for Token {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Token {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Token {
    fn from(text: &str) -> Self {
        Token::new(text)
    }
}

impl From<String> for Token {
    fn from(text: String) -> Self {
        if text.is_empty() {
            return Token::empty();
        }
        Token(Arc::from(text))
    }
}

impl PartialEq<str> for Token {
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<&str> for Token {
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == *other
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}
