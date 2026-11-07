// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! `ValueRep`: a crate field value in 64 bits. Flags (array, inlined,
//! compressed, array edit) in the top bits, the type byte below them, and a
//! 48-bit payload: an inline value or a file offset.

use crate::usd::sdf::ValueKind;

const ARRAY: u64 = 1 << 63;
const INLINED: u64 = 1 << 62;
const COMPRESSED: u64 = 1 << 61;
const ARRAY_EDIT: u64 = 1 << 60;
const PAYLOAD: u64 = (1 << 48) - 1;

/// A ValueRep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rep(pub u64);

impl Rep {
    /// A rep of `kind` with `payload` (a file offset, or the value inlined).
    pub fn new(kind: ValueKind, payload: u64, inlined: bool) -> Rep {
        Rep(((kind as u64) << 48) | (payload & PAYLOAD) | if inlined { INLINED } else { 0 })
    }

    /// An array rep of `kind` at `offset` (0 for an empty array).
    pub fn array(kind: ValueKind, offset: u64, compressed: bool) -> Rep {
        Rep(ARRAY
            | ((kind as u64) << 48)
            | (offset & PAYLOAD)
            | if compressed { COMPRESSED } else { 0 })
    }

    /// Whether the value is an array.
    pub fn is_array(self) -> bool {
        self.0 & ARRAY != 0
    }

    /// Whether the value is in the payload.
    pub fn is_inlined(self) -> bool {
        self.0 & INLINED != 0
    }

    /// Whether the array is compressed.
    pub fn is_compressed(self) -> bool {
        self.0 & COMPRESSED != 0
    }

    /// Whether the value is an array edit.
    pub fn is_array_edit(self) -> bool {
        self.0 & ARRAY_EDIT != 0
    }

    /// The type byte.
    pub fn type_byte(self) -> u8 {
        (self.0 >> 48) as u8
    }

    /// The 48-bit payload.
    pub fn payload(self) -> u64 {
        self.0 & PAYLOAD
    }

    /// The low 32 bits of the payload, where small values are inlined.
    pub fn inline_bits(self) -> u32 {
        self.0 as u32
    }
}
