// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Array elements as crate bytes: little-endian scalars, Gf vectors,
//! quaternions (imaginary first, as Gf lays them out) and row-major
//! matrices.

use crate::usd::sdf::Half;

/// An attribute value element with a fixed crate layout.
pub trait CrateElement: Copy + Default + Send + Sync {
    /// Bytes per element.
    const SIZE: usize;
    /// Read one element from `SIZE` bytes.
    fn read(bytes: &[u8]) -> Self;
    /// Append one element.
    fn write(&self, out: &mut Vec<u8>);
}

macro_rules! scalar {
    ($t:ty, $size:expr) => {
        impl CrateElement for $t {
            const SIZE: usize = $size;
            fn read(bytes: &[u8]) -> $t {
                let mut raw = [0u8; $size];
                raw.copy_from_slice(&bytes[..$size]);
                <$t>::from_le_bytes(raw)
            }
            fn write(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
        }
    };
}
scalar!(u8, 1);
scalar!(i32, 4);
scalar!(u32, 4);
scalar!(i64, 8);
scalar!(u64, 8);
scalar!(f32, 4);
scalar!(f64, 8);

impl CrateElement for bool {
    const SIZE: usize = 1;
    fn read(bytes: &[u8]) -> bool {
        bytes[0] != 0
    }
    fn write(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
}

impl CrateElement for Half {
    const SIZE: usize = 2;
    fn read(bytes: &[u8]) -> Half {
        Half(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.0.to_le_bytes());
    }
}

impl<S: CrateElement, const N: usize> CrateElement for [S; N]
where
    [S; N]: Default,
{
    const SIZE: usize = S::SIZE * N;
    fn read(bytes: &[u8]) -> [S; N] {
        let mut out = [S::default(); N];
        for (index, slot) in out.iter_mut().enumerate() {
            *slot = S::read(&bytes[index * S::SIZE..]);
        }
        out
    }
    fn write(&self, out: &mut Vec<u8>) {
        for item in self {
            item.write(out);
        }
    }
}

/// `count` elements from their bytes (`count * SIZE` bytes).
// `as_chunks` needs a literal size; `T::SIZE` is generic.
#[allow(clippy::chunks_exact_to_as_chunks)]
pub fn decode<T: CrateElement>(bytes: &[u8]) -> Vec<T> {
    bytes.chunks_exact(T::SIZE).map(T::read).collect()
}

/// The bytes of `items`.
pub fn encode<T: CrateElement>(items: &[T], out: &mut Vec<u8>) {
    out.reserve(items.len() * T::SIZE);
    for item in items {
        item.write(out);
    }
}
