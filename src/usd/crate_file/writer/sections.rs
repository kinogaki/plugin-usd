// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The structural sections, the table of contents and the bootstrap
//! (`CrateFile::_Write` and its `_Write*` helpers), each section integer-
//! compressed as files from 0.4.0 are. The path tree is the paths sorted in
//! SdfPath order and walked depth first (`_BuildCompressedPathDataRecursive`):
//! each entry's path index, element token (negated for a prim property) and
//! jump (-2 leaf, -1 child only, 0 sibling only, n: sibling n entries on).

use super::{CrateWriter, element_token};
use crate::usd::crate_file::compression::{compress, compress_integers};
use crate::usd::error::Result;
use crate::usd::sdf::{Path, PathKind};

/// The path tree's three columns.
#[derive(Default)]
struct Tree {
    indexes: Vec<i32>,
    elements: Vec<i32>,
    jumps: Vec<i32>,
}

impl CrateWriter {
    pub(super) fn write_sections(&mut self) -> Result<()> {
        let mut sections: Vec<(&str, u64, u64)> = Vec::with_capacity(6);
        let mut at = self.tell();
        self.write_tokens()?;
        sections.push(("TOKENS", at, self.tell() - at));
        at = self.tell();
        self.put_u64(self.strings.len() as u64);
        for index in self.strings.clone() {
            self.put_u32(index);
        }
        sections.push(("STRINGS", at, self.tell() - at));
        at = self.tell();
        self.write_fields()?;
        sections.push(("FIELDS", at, self.tell() - at));
        at = self.tell();
        self.put_u64(self.field_sets.len() as u64);
        let sets: Vec<i32> = self.field_sets.iter().map(|&index| index as i32).collect();
        self.compressed_ints(&sets)?;
        sections.push(("FIELDSETS", at, self.tell() - at));
        at = self.tell();
        self.write_paths()?;
        sections.push(("PATHS", at, self.tell() - at));
        at = self.tell();
        self.write_specs()?;
        sections.push(("SPECS", at, self.tell() - at));
        // The table of contents, then the bootstrap in the reserved bytes.
        let toc = self.tell();
        self.put_u64(sections.len() as u64);
        for (name, start, size) in sections {
            let mut field = [0u8; 16];
            field[..name.len()].copy_from_slice(name.as_bytes());
            self.out.extend_from_slice(&field);
            self.put_u64(start);
            self.put_u64(size);
        }
        let boot = &mut self.out[..super::BOOTSTRAP];
        boot.fill(0);
        boot[..8].copy_from_slice(b"PXR-USDC");
        boot[9] = self.minor;
        boot[16..24].copy_from_slice(&toc.to_le_bytes());
        Ok(())
    }

    fn write_tokens(&mut self) -> Result<()> {
        let mut text = Vec::new();
        for token in &self.tokens {
            text.extend_from_slice(token.as_bytes());
            text.push(0);
        }
        self.put_u64(self.tokens.len() as u64);
        self.put_u64(text.len() as u64);
        let mut packed = Vec::new();
        compress(&text, &mut packed)?;
        self.put_u64(packed.len() as u64);
        self.out.extend_from_slice(&packed);
        Ok(())
    }

    fn write_fields(&mut self) -> Result<()> {
        self.put_u64(self.fields.len() as u64);
        let tokens: Vec<i32> = self.fields.iter().map(|(token, _)| *token as i32).collect();
        self.compressed_ints(&tokens)?;
        let mut reps = Vec::with_capacity(self.fields.len() * 8);
        for (_, rep) in &self.fields {
            reps.extend_from_slice(&rep.0.to_le_bytes());
        }
        let mut packed = Vec::new();
        compress(&reps, &mut packed)?;
        self.put_u64(packed.len() as u64);
        self.out.extend_from_slice(&packed);
        Ok(())
    }

    fn write_specs(&mut self) -> Result<()> {
        self.put_u64(self.specs.len() as u64);
        let paths: Vec<i32> = self.specs.iter().map(|spec| spec.0 as i32).collect();
        let sets: Vec<i32> = self.specs.iter().map(|spec| spec.1 as i32).collect();
        let kinds: Vec<i32> = self.specs.iter().map(|spec| spec.2 as i32).collect();
        self.compressed_ints(&paths)?;
        self.compressed_ints(&sets)?;
        self.compressed_ints(&kinds)
    }

    /// The path table's size, then the tree's three compressed columns.
    fn write_paths(&mut self) -> Result<()> {
        self.put_u64(self.paths.len() as u64);
        // The tree holds every non-empty path, in SdfPath order.
        let mut order: Vec<u32> = (0..self.paths.len() as u32)
            .filter(|&i| !self.paths[i as usize].is_empty())
            .collect();
        order.sort_by(|a, b| self.paths[*a as usize].cmp(&self.paths[*b as usize]));
        self.put_u64(order.len() as u64);
        let mut tree = Tree::default();
        let ordered: Vec<Path> = order
            .iter()
            .map(|&index| self.paths[index as usize].clone())
            .collect();
        self.build_tree(&order, &ordered, 0, &mut tree);
        self.compressed_ints(&tree.indexes)?;
        self.compressed_ints(&tree.elements)?;
        self.compressed_ints(&tree.jumps)
    }

    /// Emit the entries from `start` on that share its parent, each followed
    /// by its subtree; the position after them.
    fn build_tree(
        &mut self,
        order: &[u32],
        paths: &[Path],
        start: usize,
        tree: &mut Tree,
    ) -> usize {
        let mut current = start;
        while current < paths.len() {
            let path = &paths[current];
            // The subtree of `current` ends at the first path it does not prefix.
            let mut subtree_end = current + 1;
            while subtree_end < paths.len() && paths[subtree_end].has_prefix(path) {
                subtree_end += 1;
            }
            let has_child = current + 1 < subtree_end && paths[current + 1].parent() == *path;
            let has_sibling =
                subtree_end < paths.len() && paths[subtree_end].parent() == path.parent();
            let this = tree.indexes.len();
            tree.indexes.push(order[current] as i32);
            let token = self.token(&element_token(path)) as i32;
            tree.elements.push(if path.kind() == PathKind::Property {
                -token
            } else {
                token
            });
            tree.jumps.push(0);
            let mut after = current + 1;
            if has_child {
                after = self.build_tree(order, paths, current + 1, tree);
            }
            tree.jumps[this] = match (has_child, has_sibling) {
                (true, true) => (tree.indexes.len() - this) as i32,
                (false, true) => 0,
                (true, false) => -1,
                (false, false) => -2,
            };
            if !has_sibling {
                return after;
            }
            current = after;
        }
        paths.len()
    }

    /// Integers encoded and compressed: a u64 size, then the bytes
    /// (`_WriteCompressedInts`).
    fn compressed_ints(&mut self, values: &[i32]) -> Result<()> {
        if values.is_empty() {
            self.put_u64(0);
            return Ok(());
        }
        let packed = compress_integers(values)?;
        self.put_u64(packed.len() as u64);
        self.out.extend_from_slice(&packed);
        Ok(())
    }
}
