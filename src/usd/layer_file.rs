// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Layers from and to files: the serialization recognized by its signature
//! (`PXR-USDC`, `#usda `, a ZIP local header) whatever the extension says,
//! as OpenUSD's file formats do; written by the target's extension.

use std::path::Path as FilePath;

use crate::usd::error::{Error, Result};
use crate::usd::sdf::LayerData;

/// A layer serialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Text.
    Usda,
    /// Binary crate.
    Usdc,
    /// Package.
    Usdz,
}

impl Format {
    /// The format whose signature `data` starts with.
    pub fn detect(data: &[u8]) -> Option<Format> {
        if data.starts_with(b"PXR-USDC") {
            Some(Format::Usdc)
        } else if data.starts_with(b"#usda ") {
            Some(Format::Usda)
        } else if data.starts_with(b"PK\x03\x04") {
            Some(Format::Usdz)
        } else {
            None
        }
    }

    /// The format a file of this name is written as: `.usda`, `.usdc` or
    /// `.usdz`; a bare `.usd` is written as a crate, as OpenUSD does.
    pub fn for_path(path: &FilePath) -> Option<Format> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "usda" => Some(Format::Usda),
            "usdc" | "usd" => Some(Format::Usdc),
            "usdz" => Some(Format::Usdz),
            _ => None,
        }
    }
}

/// Read a layer from its bytes, by their signature.
pub fn read_layer_bytes(data: &[u8]) -> Result<LayerData> {
    match Format::detect(data) {
        Some(Format::Usda) => crate::usd::text::read_usda(data),
        Some(Format::Usdc) => crate::usd::crate_file::read_usdc(data),
        Some(Format::Usdz) => crate::usd::package::read_usdz(data),
        None => Err(Error::invalid(
            "not a USD layer: no usda, usdc or usdz signature",
        )),
    }
}

/// Read the layer at `path` (a package's root layer for usdz).
pub fn read_layer(path: impl AsRef<FilePath>) -> Result<LayerData> {
    let path = path.as_ref();
    let name = path.display().to_string();
    let data = std::fs::read(path).map_err(|error| Error::from(error).in_file(name.clone()))?;
    read_layer_bytes(&data).map_err(|error| error.in_file(name))
}

/// The layer serialized as `format` (a usdz holds it as a crate).
pub fn layer_bytes(layer: &LayerData, format: Format) -> Result<Vec<u8>> {
    match format {
        Format::Usda => crate::usd::text::write_usda(layer),
        Format::Usdc => crate::usd::crate_file::write_usdc(layer),
        Format::Usdz => crate::usd::package::write_usdz(layer, "root.usdc", &[]),
    }
}

/// Write the layer to `path`, in the format its extension names.
pub fn write_layer(layer: &LayerData, path: impl AsRef<FilePath>) -> Result<()> {
    let path = path.as_ref();
    let name = path.display().to_string();
    let format = Format::for_path(path).ok_or_else(|| {
        Error::invalid("a USD file must end in .usda, .usdc, .usd or .usdz").in_file(name.clone())
    })?;
    let bytes = layer_bytes(layer, format).map_err(|error| error.in_file(name.clone()))?;
    std::fs::write(path, bytes).map_err(|error| Error::from(error).in_file(name))
}

/// The layer at `path` with everything it composes flattened into one
/// layer (instances expanded, each holding its own prims), and the warnings
/// composing left. A layer with nothing to compose is returned as it is.
pub fn open_flattened(path: impl AsRef<FilePath>) -> Result<(LayerData, Vec<String>)> {
    let mut stage = crate::usd::Stage::open(path)?;
    if stage.is_single_layer() {
        return Ok((stage.root_layer().clone(), stage.warnings()));
    }
    let layer = stage.flatten_with(crate::usd::FlattenOptions {
        comment: false,
        instancing: false,
    })?;
    Ok((layer, stage.warnings()))
}
