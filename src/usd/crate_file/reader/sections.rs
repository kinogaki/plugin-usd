// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The bootstrap, the table of contents and the structural sections
//! (`CrateFile::_ReadStructuralSections`): tokens (NUL-separated, compressed
//! from 0.4.0), strings (token indexes), fields, field sets and specs
//! (integer-compressed from 0.4.0, fixed records before).

use super::{Crate, NEWEST_MINOR, SpecRecord, invalid};
use crate::usd::crate_file::compression::decompress;
use crate::usd::crate_file::rep::Rep;
use crate::usd::error::{Error, Result};
use crate::usd::sdf::Token;

/// The largest token section accepted, decompressed.
const MOST_TOKEN_BYTES: u64 = 1 << 32;

impl Crate<'_> {
    pub(super) fn read_structure(&mut self) -> Result<()> {
        if self.data.len() < 88 {
            return Err(invalid("not a crate file: too short for its bootstrap"));
        }
        if &self.data[..8] != b"PXR-USDC" {
            return Err(invalid("not a crate file: no PXR-USDC bootstrap"));
        }
        let major = self.data[8];
        self.minor = self.data[9];
        self.patch = self.data[10];
        if major != 0 || self.minor > NEWEST_MINOR {
            return Err(Error::unsupported(format!(
                "crate version {major}.{}.{} is newer than this reader (0.{NEWEST_MINOR}.0)",
                self.minor, self.patch
            )));
        }
        let toc = self.u64_at(16)?;
        let count = self.u64_at(toc)?;
        if count > 64 {
            return Err(invalid("a crate table of contents with too many sections"));
        }
        let mut sections = Vec::new();
        for index in 0..count {
            let at = toc + 8 + index * 32;
            let name = self.bytes(at, 16)?;
            let length = name.iter().position(|&b| b == 0).unwrap_or(16);
            sections.push((name[..length].to_vec(), self.u64_at(at + 16)?));
        }
        let find = |name: &[u8]| {
            sections
                .iter()
                .find(|(section, _)| section == name)
                .map(|(_, start)| *start)
        };
        if let Some(start) = find(b"TOKENS") {
            self.read_tokens(start)?;
        }
        if let Some(start) = find(b"STRINGS") {
            self.read_strings(start)?;
        }
        if let Some(start) = find(b"FIELDS") {
            self.read_fields(start)?;
        }
        if let Some(start) = find(b"FIELDSETS") {
            self.read_field_sets(start)?;
        }
        if let Some(start) = find(b"PATHS") {
            self.read_paths(start)?;
        }
        if let Some(start) = find(b"SPECS") {
            self.read_specs(start)?;
        }
        Ok(())
    }

    fn read_tokens(&mut self, start: u64) -> Result<()> {
        let count = self.u64_at(start)?;
        let owned;
        let chars: &[u8] = if self.since(4, 0) {
            let size = self.u64_at(start + 8)?;
            let packed = self.u64_at(start + 16)?;
            if size > MOST_TOKEN_BYTES {
                return Err(invalid("a crate token section too large"));
            }
            let mut text = Vec::with_capacity(size as usize);
            decompress(self.bytes(start + 24, packed)?, &mut text, size as usize)?;
            if text.len() as u64 != size {
                return Err(invalid("a crate token section of the wrong size"));
            }
            owned = text;
            &owned
        } else {
            let size = self.u64_at(start + 8)?;
            self.bytes(start + 16, size)?
        };
        if count > chars.len() as u64 + 1 {
            return Err(invalid("a crate claims more tokens than its bytes hold"));
        }
        let mut tokens = Vec::with_capacity(count as usize);
        let mut pieces = chars.split(|&b| b == 0);
        for _ in 0..count {
            let piece = pieces
                .next()
                .ok_or_else(|| invalid("a crate token section ends early"))?;
            let text = std::str::from_utf8(piece)
                .map_err(|_| invalid("a crate token that is not UTF-8"))?;
            tokens.push(Token::new(text));
        }
        self.tokens = tokens;
        Ok(())
    }

    fn read_strings(&mut self, start: u64) -> Result<()> {
        let count = self.count_at(start, 4)?;
        let indexes = self.bytes(start + 8, count * 4)?;
        let strings = indexes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|raw| self.token(u64::from(u32::from_le_bytes(*raw))))
            .collect::<Result<Vec<_>>>()?;
        self.strings = strings;
        Ok(())
    }

    fn read_fields(&mut self, start: u64) -> Result<()> {
        let count = self.count_at(start, 1)?;
        let mut fields = Vec::with_capacity(count as usize);
        if !self.since(4, 0) {
            let records = self.bytes(start + 8, count * 16)?;
            for record in records.as_chunks::<16>().0.iter() {
                let token = u32::from_le_bytes(record[4..8].try_into().expect("4 bytes"));
                let rep = u64::from_le_bytes(record[8..16].try_into().expect("8 bytes"));
                fields.push((self.token(u64::from(token))?, Rep(rep)));
            }
            self.fields = fields;
            return Ok(());
        }
        let (tokens, at) = self.compressed_ints::<i32>(start + 8, count as usize)?;
        let reps_size = self.u64_at(at)?;
        let mut reps = Vec::with_capacity(count as usize * 8);
        if count > 0 {
            decompress(
                self.bytes(at + 8, reps_size)?,
                &mut reps,
                count as usize * 8,
            )?;
            if reps.len() != count as usize * 8 {
                return Err(invalid("crate field reps of the wrong size"));
            }
        }
        for (token, rep) in tokens.iter().zip(reps.as_chunks::<8>().0.iter()) {
            fields.push((
                self.token(*token as u32 as u64)?,
                Rep(u64::from_le_bytes(*rep)),
            ));
        }
        self.fields = fields;
        Ok(())
    }

    fn read_field_sets(&mut self, start: u64) -> Result<()> {
        let count = self.count_at(start, 1)? as usize;
        let sets: Vec<u32> = if self.since(4, 0) {
            self.compressed_ints::<i32>(start + 8, count)?
                .0
                .into_iter()
                .map(|value| value as u32)
                .collect()
        } else {
            self.bytes(start + 8, count as u64 * 4)?
                .as_chunks::<4>()
                .0
                .iter()
                .map(|raw| u32::from_le_bytes(*raw))
                .collect()
        };
        if sets.last().is_some_and(|&last| last != u32::MAX) {
            return Err(invalid("crate field sets must end with a terminator"));
        }
        self.field_sets = sets;
        Ok(())
    }

    fn read_specs(&mut self, start: u64) -> Result<()> {
        let count = self.count_at(start, 1)? as usize;
        if self.since(4, 0) {
            let (paths, at) = self.compressed_ints::<i32>(start + 8, count)?;
            let (sets, at) = self.compressed_ints::<i32>(at, count)?;
            let (kinds, _) = self.compressed_ints::<i32>(at, count)?;
            self.specs = (0..count)
                .map(|index| SpecRecord {
                    path: paths[index] as u32,
                    field_set: sets[index] as u32,
                    kind: kinds[index] as u32,
                })
                .collect();
            return Ok(());
        }
        let (size, skip) = if self.since(1, 0) { (12, 0) } else { (16, 4) };
        let records = self.bytes(start + 8, count as u64 * size)?;
        self.specs = records
            .chunks_exact(size as usize)
            .map(|record| {
                let word = |at: usize| {
                    u32::from_le_bytes(
                        record[skip + at..skip + at + 4]
                            .try_into()
                            .expect("4 bytes"),
                    )
                };
                SpecRecord {
                    path: word(0),
                    field_set: word(4),
                    kind: word(8),
                }
            })
            .collect();
        Ok(())
    }
}
