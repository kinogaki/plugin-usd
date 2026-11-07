// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Layers from and to files: the serialization recognized by its signature
//! (`PXR-USDC`, `#usda `, a ZIP local header) whatever the extension says,
//! as OpenUSD's file formats do; written by the target's extension.

use std::path::Path as FilePath;

use crate::usd::error::{Error, Result};
use crate::usd::sdf::LayerData;
use kinogaki_studio_api::format::ReadBudget;

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

/// Read a layer from its bytes, by their signature, within `budget`.
pub fn read_layer_bytes(data: &[u8], budget: &ReadBudget) -> Result<LayerData> {
    match Format::detect(data) {
        Some(Format::Usda) => crate::usd::text::read_usda(data, budget),
        Some(Format::Usdc) => crate::usd::crate_file::read_usdc(data, budget),
        Some(Format::Usdz) => crate::usd::package::read_usdz(data, budget),
        None => Err(Error::invalid(
            "not a USD layer: no usda, usdc or usdz signature",
        )),
    }
}

/// The layer serialized as `format` (a usdz holds it as a crate).
pub fn layer_bytes(layer: &LayerData, format: Format) -> Result<Vec<u8>> {
    match format {
        Format::Usda => crate::usd::text::write_usda(layer),
        Format::Usdc => crate::usd::crate_file::write_usdc(layer),
        Format::Usdz => crate::usd::package::write_usdz(layer, "root.usdc", &[]),
    }
}

/// Write the layer to `path`, in the format its extension names, whole or
/// not at all (the SDK's `files::replace`).
pub fn write_layer(layer: &LayerData, path: impl AsRef<FilePath>) -> Result<()> {
    let path = path.as_ref();
    let name = path.display().to_string();
    let format = Format::for_path(path).ok_or_else(|| {
        Error::invalid("a USD file must end in .usda, .usdc, .usd or .usdz").in_file(name.clone())
    })?;
    let bytes = layer_bytes(layer, format).map_err(|error| error.in_file(name.clone()))?;
    let all = |file: &mut std::fs::File| std::io::Write::write_all(file, &bytes);
    kinogaki_studio_api::format::files::replace(path, all)
        .map_err(|error| Error::from(error).in_file(name))
}
