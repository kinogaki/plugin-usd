//! [`Bytes`]: a file's bytes as a view of a shared buffer, so the entries of
//! a package (and the package itself) are passed on without copies.

use std::fmt;
use std::ops::{Deref, Range};
use std::sync::Arc;

/// A buffer bytes are views of: a `Vec<u8>` read here, or bytes another
/// owner holds (a file a host keeps in memory), shared as they are.
pub type Buffer = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// Bytes held in a shared buffer: a whole file, or a range of one (a
/// package's entry). Cloning shares the buffer.
#[derive(Clone)]
pub struct Bytes {
    buffer: Buffer,
    range: Range<usize>,
}

impl Bytes {
    /// All of `buffer`.
    pub fn shared(buffer: Buffer) -> Bytes {
        let range = 0..(*buffer).as_ref().len();
        Bytes { buffer, range }
    }

    /// The bytes at `range` of these, sharing the buffer.
    ///
    /// # Panics
    /// When `range` is not within these bytes.
    pub fn slice(&self, range: Range<usize>) -> Bytes {
        assert!(range.start <= range.end && range.end <= self.range.len());
        Bytes {
            buffer: self.buffer.clone(),
            range: self.range.start + range.start..self.range.start + range.end,
        }
    }

    /// The buffer these bytes are a view of, and where in it they lie: what
    /// a holder of the whole buffer shares instead of copying.
    pub fn buffer(&self) -> (&Buffer, Range<usize>) {
        (&self.buffer, self.range.clone())
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Bytes {
        Bytes::shared(Arc::new(bytes))
    }
}

impl Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &(*self.buffer).as_ref()[self.range.clone()]
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl PartialEq for Bytes {
    /// Bytes are equal when they hold the same bytes, wherever they lie.
    fn eq(&self, other: &Bytes) -> bool {
        **self == **other
    }
}

impl Eq for Bytes {}

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes({} bytes)", self.len())
    }
}
