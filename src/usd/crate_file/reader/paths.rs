// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The path tree (`CrateFile::_ReadPaths`). From 0.4.0 it is three
//! integer-compressed columns: the path index each entry fills, its element
//! token (negative for a prim property), and a jump: -2 a leaf, -1 a child
//! only, 0 a sibling only, n > 0 a child next and a sibling n entries on.
//! Before 0.4.0 it is a stream of headers (index, element token, bits) with
//! a sibling offset when an entry has both. Elements append as
//! `SdfPath::AppendElementToken` does: `{set=sel}`, `[target]`, `.name`, or a
//! prim name.

use super::{Crate, invalid};
use crate::usd::error::Result;
use crate::usd::sdf::MOST_PATH_DEPTH as MOST_DEPTH;
use crate::usd::sdf::{Path, PathKind};

impl Crate<'_> {
    pub(super) fn read_paths(&mut self, start: u64) -> Result<()> {
        let count = self.count_at(start, 1)? as usize;
        self.charge::<Path>(count as u64)?;
        self.paths = vec![Path::empty(); count];
        if !self.since(4, 0) {
            return self.read_path_headers(start + 8);
        }
        let entries = self.u64_at(start + 8)? as usize;
        if entries > count {
            return Err(invalid("a crate path table with more entries than paths"));
        }
        let (targets, at) = self.compressed_ints::<i32>(start + 16, entries)?;
        let (elements, at) = self.compressed_ints::<i32>(at, entries)?;
        let (jumps, _) = self.compressed_ints::<i32>(at, entries)?;
        if entries == 0 {
            return Ok(());
        }
        let mut stack: Vec<(usize, Path)> = Vec::new();
        let mut index = 0;
        let mut parent = Path::empty();
        let mut steps = 0;
        loop {
            steps += 1;
            if steps > entries * 2 + 2 {
                return Err(invalid("a crate path tree that loops"));
            }
            if index >= entries {
                return Err(invalid("a crate path jump outside the table"));
            }
            let target = targets[index] as u32 as usize;
            if target >= self.paths.len() {
                return Err(invalid("a crate path index outside the table"));
            }
            let element = elements[index];
            let jump = jumps[index];
            let here = if parent.is_empty() {
                Path::absolute_root()
            } else {
                let token = u64::from(element.unsigned_abs());
                self.append_element(&parent, &self.token(token)?, element < 0)?
            };
            self.paths[target] = here.clone();
            let child = jump > 0 || jump == -1;
            let sibling = jump >= 0;
            if child {
                if sibling {
                    let next = index + jump as usize;
                    if next >= entries {
                        return Err(invalid("a crate path sibling outside the table"));
                    }
                    stack.push((next, parent.clone()));
                }
                parent = here;
                index += 1;
            } else if sibling {
                index += 1;
            } else {
                let Some((next, above)) = stack.pop() else {
                    break;
                };
                index = next;
                parent = above;
            }
        }
        Ok(())
    }

    /// The header stream of files before 0.4.0.
    fn read_path_headers(&mut self, start: u64) -> Result<()> {
        let (size, skip) = if self.since(1, 0) { (12, 0) } else { (16, 4) };
        let mut stack: Vec<(u64, Path)> = Vec::new();
        let mut at = start;
        let mut parent = Path::empty();
        let mut steps = 0;
        loop {
            steps += 1;
            if steps > self.paths.len() * 2 + 2 {
                return Err(invalid("a crate path tree that loops"));
            }
            let header = self.bytes(at + skip, 9)?;
            at += size;
            let target = u32::from_le_bytes(header[0..4].try_into().expect("4 bytes")) as usize;
            if target >= self.paths.len() {
                return Err(invalid("a crate path index outside the table"));
            }
            let element = u64::from(u32::from_le_bytes(
                header[4..8].try_into().expect("4 bytes"),
            ));
            let bits = header[8];
            let here = if parent.is_empty() {
                Path::absolute_root()
            } else {
                self.append_element(&parent, &self.token(element)?, bits & 4 != 0)?
            };
            self.paths[target] = here.clone();
            let child = bits & 1 != 0;
            let sibling = bits & 2 != 0;
            if child {
                if sibling {
                    let offset = self.i64_at(at)?;
                    at += 8;
                    let offset = u64::try_from(offset)
                        .map_err(|_| invalid("a crate path offset before the file"))?;
                    stack.push((offset, parent.clone()));
                }
                parent = here;
            } else if !sibling {
                let Some((next, above)) = stack.pop() else {
                    break;
                };
                at = next;
                parent = above;
            }
        }
        Ok(())
    }

    /// `parent` with the element `token` appended, no deeper than paths
    /// may be.
    fn append_element(&self, parent: &Path, token: &str, property: bool) -> Result<Path> {
        if parent.depth() >= MOST_DEPTH {
            return Err(invalid("a crate path deeper than 4096 elements"));
        }
        if property {
            return Ok(parent.property(token));
        }
        let Some(first) = token.bytes().next() else {
            return Err(invalid("an empty crate path element"));
        };
        match first {
            b'{' => {
                let inner = token
                    .strip_prefix('{')
                    .and_then(|rest| rest.strip_suffix('}'))
                    .ok_or_else(|| invalid("a malformed crate variant selection"))?;
                let (set, selection) = inner
                    .split_once('=')
                    .ok_or_else(|| invalid("a malformed crate variant selection"))?;
                Ok(parent.variant(set, selection))
            }
            b'[' => {
                let inner = token[1..]
                    .strip_suffix(']')
                    .ok_or_else(|| invalid("a malformed crate target element"))?;
                let target = Path::parse(inner)
                    .map_err(|_| invalid("a crate target element that is not a path"))?;
                Ok(parent.target(target))
            }
            b'.' => {
                let name = &token[1..];
                Ok(match parent.kind() {
                    PathKind::Target => parent.relational(name),
                    PathKind::Mapper => parent.mapper_arg(name),
                    _ if name == "expression" => parent.expression(),
                    _ => parent.property(name),
                })
            }
            _ => Ok(parent.child(token)),
        }
    }
}
