//! Hostile files made by code: crate files assembled section by section
//! (as crates before 0.4.0 lay them out, uncompressed, or with an LZ4 token
//! section), each shaped as one probe of the 2026-11-07 audit.
use kinogaki_usd::usd::sdf::ValueKind;

const ARRAY: u64 = 1 << 63;

/// A crate file being assembled: its bootstrap, its sections, then the
/// table of contents.
pub struct Crate {
    bytes: Vec<u8>,
    sections: Vec<(&'static str, u64, u64)>,
}

impl Crate {
    /// A crate of version 0.`minor`.0, its bootstrap written.
    pub fn new(minor: u8) -> Crate {
        let mut bytes = b"PXR-USDC".to_vec();
        bytes.extend_from_slice(&[0, minor, 0, 0, 0, 0, 0, 0]);
        bytes.resize(88, 0);
        Crate {
            bytes,
            sections: Vec::new(),
        }
    }

    /// Where the next byte goes.
    pub fn at(&self) -> u64 {
        self.bytes.len() as u64
    }

    pub fn u32(&mut self, value: u32) -> &mut Crate {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn u64(&mut self, value: u64) -> &mut Crate {
        self.bytes.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn raw(&mut self, bytes: &[u8]) -> &mut Crate {
        self.bytes.extend_from_slice(bytes);
        self
    }

    /// A section named `name`, written by `write`.
    pub fn section(&mut self, name: &'static str, write: impl FnOnce(&mut Crate)) {
        let start = self.at();
        write(self);
        self.sections.push((name, start, self.at() - start));
    }

    /// Tokens (uncompressed, before 0.4.0).
    pub fn tokens(&mut self, tokens: &[&str]) {
        let text: Vec<u8> = tokens
            .iter()
            .flat_map(|t| [t.as_bytes(), b"\0"].concat())
            .collect();
        self.section("TOKENS", |c| {
            c.u64(tokens.len() as u64).u64(text.len() as u64).raw(&text);
        });
    }

    /// Strings: each a token index.
    pub fn strings(&mut self, tokens: &[u32]) {
        self.section("STRINGS", |c| {
            c.u64(tokens.len() as u64);
            tokens.iter().for_each(|&t| _ = c.u32(t));
        });
    }

    /// Fields: (token index, ValueRep) each.
    pub fn fields(&mut self, fields: &[(u32, u64)]) {
        self.section("FIELDS", |c| {
            c.u64(fields.len() as u64);
            for &(token, rep) in fields {
                c.u32(0).u32(token).u64(rep);
            }
        });
    }

    /// Field sets: field indexes, each set ended by `u32::MAX`.
    pub fn field_sets(&mut self, sets: &[u32]) {
        self.section("FIELDSETS", |c| {
            c.u64(sets.len() as u64);
            sets.iter().for_each(|&s| _ = c.u32(s));
        });
    }

    /// The path tree as headers (target, element token, bits: 1 a child
    /// next, 2 a sibling next).
    pub fn paths(&mut self, count: u64, headers: impl IntoIterator<Item = (u32, u32, u8)>) {
        self.section("PATHS", |c| {
            c.u64(count);
            for (target, element, bits) in headers {
                c.u32(target).u32(element).raw(&[bits, 0, 0, 0]);
            }
        });
    }

    /// Specs: (path, field set, spec type) each.
    pub fn specs(&mut self, specs: &[(u32, u32, u32)]) {
        self.section("SPECS", |c| {
            c.u64(specs.len() as u64);
            for &(path, set, kind) in specs {
                c.u32(path).u32(set).u32(kind);
            }
        });
    }

    /// The file: its table of contents last, named by the bootstrap.
    pub fn finish(mut self) -> Vec<u8> {
        let toc = self.at();
        let count = self.sections.len() as u64;
        self.u64(count);
        for (name, start, size) in std::mem::take(&mut self.sections) {
            let mut padded = [0u8; 16];
            padded[..name.len()].copy_from_slice(name.as_bytes());
            self.raw(&padded).u64(start).u64(size);
        }
        self.bytes[16..24].copy_from_slice(&toc.to_le_bytes());
        self.bytes
    }
}

/// A ValueRep of `kind` at file offset `at`.
pub fn rep(kind: ValueKind, at: u64) -> u64 {
    ((kind as u64) << 48) | at
}

/// An array ValueRep of `kind` at file offset `at`.
pub fn array_rep(kind: ValueKind, at: u64) -> u64 {
    ARRAY | rep(kind, at)
}

/// A crate whose pseudo-root's `customLayerData` is a dictionary of two
/// entries holding one dictionary, which holds two entries holding one
/// dictionary… `levels` deep: a few hundred bytes, `2^levels` dictionaries
/// when each reference is decoded on its own.
pub fn dictionary_dag(levels: usize) -> Vec<u8> {
    let mut c = Crate::new(3);
    // The innermost dictionary, empty, then each level over the one below.
    let mut below = c.at();
    c.u64(0);
    for _ in 0..levels {
        let here = c.at();
        c.u64(2);
        for key in 0..2 {
            c.u32(key).u64(8).u64(rep(ValueKind::Dictionary, below));
        }
        below = here;
    }
    c.tokens(&["customLayerData", "a", "b"]);
    c.strings(&[1, 2]);
    c.fields(&[(0, rep(ValueKind::Dictionary, below))]);
    c.field_sets(&[0, u32::MAX]);
    c.paths(1, [(0, 0, 0)]);
    c.specs(&[(0, 0, 7)]);
    c.finish()
}

/// A crate whose path tree is `depth` prims deep, one inside the next.
pub fn deep_paths(depth: u32) -> Vec<u8> {
    let mut c = Crate::new(3);
    c.tokens(&["a"]);
    let headers = (0..=depth).map(|at| (at, 0, u8::from(at < depth)));
    c.paths(u64::from(depth) + 1, headers);
    c.finish()
}

/// A crate of `prims` prims at the root, each one field holding the same
/// array of `doubles` doubles: one ValueRep they all share.
pub fn shared_array(prims: u32, doubles: u32) -> Vec<u8> {
    let mut c = Crate::new(3);
    let array = c.at();
    c.u32(1).u32(doubles);
    for k in 0..doubles {
        c.raw(&f64::from(k).to_le_bytes());
    }
    let names: Vec<String> = (0..prims).map(|k| format!("p{k}")).collect();
    let mut tokens = vec!["", "samples"];
    tokens.extend(names.iter().map(String::as_str));
    c.tokens(&tokens);
    c.fields(&[(1, array_rep(ValueKind::Double, array))]);
    // Set 0: the shared field; set 2: none (the pseudo-root's).
    c.field_sets(&[0, u32::MAX, u32::MAX]);
    // The root, then each prim a sibling of the one before it.
    let prim = |at: u32| (at, at + 1, if at < prims { 2 } else { 0 });
    let headers = std::iter::once((0, 0, 1)).chain((1..=prims).map(prim));
    c.paths(u64::from(prims) + 1, headers);
    let mut specs = vec![(0, 2, 7)];
    specs.extend((1..=prims).map(|at| (at, 0, 6)));
    c.specs(&specs);
    c.finish()
}

/// A crate of version 0.8.0 whose token section decompresses `packed`
/// bytes of LZ4 into zeros, every token empty: a few megabytes giving
/// hundreds of times as much. With `claim`, the section claims that many
/// bytes rather than what the LZ4 gives.
pub fn token_bomb(packed: usize, claim: Option<u64>) -> Vec<u8> {
    let (block, decoded) = zeros_lz4(packed);
    let size = claim.unwrap_or(decoded);
    let mut c = Crate::new(8);
    c.section("TOKENS", |c| {
        c.u64(size + 1).u64(size).u64(block.len() as u64 + 1);
        c.raw(&[0]).raw(&block);
    });
    c.finish()
}

/// An LZ4 block of about `packed` bytes giving zeros, and how many: a
/// literal zero, one match of it as long as the bytes allow (255 bytes a
/// length byte), then five literal zeros, as a block must end.
fn zeros_lz4(packed: usize) -> (Vec<u8>, u64) {
    let lengths = packed.max(16) - 11;
    let mut block = vec![0x1F, 0, 1, 0];
    block.resize(4 + lengths, 255);
    block.push(0);
    block.extend_from_slice(&[0x50, 0, 0, 0, 0, 0]);
    (block, 1 + 4 + 15 + 255 * lengths as u64 + 5)
}
