// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! usdz packages, after OpenUSD's usdz file format (`usdzFileFormat.cpp`,
//! `zipFile.cpp`) and usdUtils' packaging: a ZIP whose entries are stored
//! and 64-byte aligned, the first entry being the root layer (usdc or usda),
//! the others the files its layers name (textures, other layers, MaterialX
//! documents). Entries are read in place: a [`Bytes`] of an entry shares
//! the archive's buffer, so a package's textures cost no copy.

mod bytes;
mod crc32;
pub mod zip;

pub use bytes::{Buffer, Bytes};

use crate::usd::error::{Error, Result};
use crate::usd::sdf::LayerData;

/// The alignment usdz requires of every entry's data.
pub const ALIGNMENT: usize = 64;

/// The deepest nesting of packages inside packages accepted.
const MOST_NESTING: usize = 8;

/// A package's entries, read in place from its bytes.
#[derive(Clone, Debug)]
pub struct Package {
    bytes: Bytes,
    entries: Vec<zip::Entry>,
}

impl Package {
    /// The package `bytes` holds: its central directory read, every entry
    /// checked to lie within the archive.
    pub fn open(bytes: Bytes) -> Result<Package> {
        let entries = zip::entries(&bytes)?;
        Ok(Package { bytes, entries })
    }

    /// The entries' names, in archive order (the root layer first).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.name.as_str())
    }

    /// The bytes of entry `name`; fails for a missing or compressed entry.
    pub fn entry(&self, name: &str) -> Result<Bytes> {
        let found = (self.entries.iter())
            .find(|entry| entry.name == name)
            .ok_or_else(|| Error::invalid(format!("the package has no entry {name}")))?;
        self.data(found)
    }

    /// The bytes of `entry`.
    fn data(&self, entry: &zip::Entry) -> Result<Bytes> {
        if entry.method != 0 {
            return Err(Error::invalid(format!(
                "the package entry {} is compressed; usdz entries must be stored",
                entry.name
            )));
        }
        Ok(self.bytes.slice(entry.offset..entry.offset + entry.size))
    }

    /// The bytes `path` names inside the package: an entry (`tex/a.png`),
    /// or an entry of a package inside it (`inner.usdz[tex/a.png]`).
    pub fn find(&self, path: &str) -> Result<Bytes> {
        self.find_nested(path, 0)
    }

    fn find_nested(&self, path: &str, depth: usize) -> Result<Bytes> {
        if depth > MOST_NESTING {
            return Err(Error::limit("packages nested too deeply"));
        }
        let Some(bracket) = path.find('[') else {
            return self.entry(path);
        };
        let inner = path[bracket + 1..]
            .strip_suffix(']')
            .ok_or_else(|| Error::invalid(format!("not a package path: {path}")))?;
        let nested = Package::open(self.entry(&path[..bracket])?)?;
        nested.find_nested(inner, depth + 1)
    }

    /// The root layer: the first entry, read by its signature.
    pub fn root_layer(&self) -> Result<LayerData> {
        read_usdz(&self.bytes)
    }
}

/// Read the root layer of the package `data`.
pub fn read_usdz(data: &[u8]) -> Result<LayerData> {
    root_of(data, 0)
}

/// The root layer of `data`, a package first in a package opened in turn.
fn root_of(data: &[u8], depth: usize) -> Result<LayerData> {
    if depth > MOST_NESTING {
        return Err(Error::limit("packages nested too deeply"));
    }
    let entries = zip::entries(data)?;
    let root = entries
        .first()
        .ok_or_else(|| Error::invalid("an empty usdz package"))?;
    let bytes = zip::stored(data, root)?;
    if root.name.ends_with(".usdz") {
        return root_of(bytes, depth + 1);
    }
    crate::usd::layer_file::read_layer_bytes(bytes)
        .map_err(|error| error.in_file(root.name.clone()))
}

/// A package holding `layer` as its root entry `root_name` (a crate when
/// it ends in `.usdc`, text when `.usda`), then `files` (name, bytes).
pub fn write_usdz(layer: &LayerData, root_name: &str, files: &[(&str, &[u8])]) -> Result<Vec<u8>> {
    let root = if root_name.ends_with(".usdc") {
        crate::usd::crate_file::write_usdc(layer)?
    } else if root_name.ends_with(".usda") {
        crate::usd::text::write_usda(layer)?
    } else {
        return Err(Error::invalid(
            "a usdz root layer must be named .usdc or .usda",
        ));
    };
    let mut entries = vec![(root_name, root.as_slice())];
    entries.extend_from_slice(files);
    zip::write_stored(&entries, ALIGNMENT)
}
