//! The files a scene names (textures, a dome's image, MaterialX
//! documents): a package's entries held as their bytes, files on disk by
//! their path.

use std::path::PathBuf;

use crate::usd::package::Bytes;
use crate::usd::sdf::Token;

/// A file a scene's materials or lights name.
#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    /// The path the scene names it by: an anchored identifier on reading
    /// (`/dir/a.usdz[textures/wood.png]`).
    pub path: Token,
    /// The path as the layer wrote it (`textures/wood.png`): what a writer
    /// writes for a missing file.
    pub authored: Token,
    /// Where its bytes are.
    pub source: AssetSource,
}

/// Where an asset's bytes are.
#[derive(Clone, Debug, PartialEq)]
pub enum AssetSource {
    /// Held: a package's entry, or bytes given to write.
    Bytes(Bytes),
    /// A file on disk, by its path: on reading, where the stage places it,
    /// for whoever reads it to find there.
    File(PathBuf),
    /// Nowhere it could be found.
    Missing,
}
