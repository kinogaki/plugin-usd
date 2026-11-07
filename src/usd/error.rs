//! Errors: what went wrong, and where when it was in a file.

use std::fmt;

/// What kind of failure an [`Error`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The input is not valid USD (text, crate or package).
    Invalid,
    /// Valid USD this library does not read or write (a feature not ported).
    Unsupported,
    /// The input exceeds a size or depth limit.
    Limit,
    /// Reading or writing a file failed.
    Io,
    /// The work was no longer wanted ([`StageOptions::cancel`](crate::usd::StageOptions::cancel)).
    Cancelled,
}

/// A failure with a message, and the file and line it happened at when known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// What kind of failure.
    pub kind: ErrorKind,
    /// What went wrong.
    pub message: String,
    /// The file, when the failure was in one.
    pub file: Option<String>,
    /// The 1-based line and column in a text layer.
    pub position: Option<(usize, usize)>,
}

/// A result with this crate's [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error {
            kind,
            message: message.into(),
            file: None,
            position: None,
        }
    }

    /// Input that is not valid USD.
    pub fn invalid(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Invalid, message)
    }

    /// USD this library does not handle.
    pub fn unsupported(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Unsupported, message)
    }

    /// Input over a size or depth limit.
    pub fn limit(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Limit, message)
    }

    /// A file system failure.
    pub fn io(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Io, message)
    }

    /// Work stopped because it was no longer wanted.
    pub fn cancelled() -> Error {
        Error::new(ErrorKind::Cancelled, "cancelled")
    }

    /// The same error, located in `file` (unless already located).
    pub fn in_file(mut self, file: impl Into<String>) -> Error {
        if self.file.is_none() {
            self.file = Some(file.into());
        }
        self
    }

    /// The same error at a line and column.
    pub fn at(mut self, line: usize, column: usize) -> Error {
        self.position = Some((line, column));
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{file}:")?;
            if let Some((line, column)) = self.position {
                write!(f, "{line}:{column}:")?;
            }
            write!(f, " ")?;
        } else if let Some((line, column)) = self.position {
            write!(f, "line {line}, column {column}: ")?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Error {
        Error::io(error.to_string())
    }
}
