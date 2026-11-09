//! Universal Scene Description as a format plugin for Kinogaki Studio:
//! usda, usdc and usdz files read into the standard values and written from
//! them.
//!
//! [`usd`] is the format's own model, parser and writer, and uses nothing of
//! the SDK but its workers and MaterialX's documents: layers read and
//! written as usda text, usdc crates and usdz packages, stages composed
//! (sublayers, references, payloads, variants, inherits, specializes,
//! relocates, instancing, MaterialX documents as UsdMtlx reads them), and the
//! scene a stage describes (meshes, curves, points, implicit shapes,
//! instances, cameras, lights and UsdShade materials) read and authored as
//! plain data.
//!
//! The format converts between that scene and the standard values at its
//! boundary alone: reading and writing (`read`, `write`) in the project's
//! units (`space`), materials in every render context (`shading`), lights
//! and cameras (`lights`).
pub mod usd;

mod lights;
mod names;
mod read;
mod shading;
mod space;
mod subdivision;
mod write;

use kinogaki_studio_api::format::options;
use kinogaki_studio_api::params::Schema;
use kinogaki_studio_api::{
    DataTypeId, Error, FileFormat, FormatId, FormatOptions, FormatWriter, Imported, Input, Package,
    PackageId, Purpose, Reading, Value, standard,
};

/// The package's id.
pub fn id() -> PackageId {
    PackageId::declared("kinogaki.usd")
}

/// The format's id.
pub fn format() -> FormatId {
    FormatId::declared("kinogaki.format.usd")
}

/// The package: the format alone.
pub fn package() -> Package {
    Package::new(id(), "USD").with_format(Usd)
}

/// What a USD file holds: materials, polygons and curves, instances,
/// lights and cameras.
const TYPES: &[DataTypeId] = &[
    standard::materials().id(),
    standard::geometry().id(),
    standard::instances().id(),
    standard::light().id(),
    standard::camera().id(),
];

/// The format.
pub struct Usd;

impl FileFormat for Usd {
    fn id(&self) -> FormatId {
        format()
    }
    fn label(&self) -> &str {
        "Universal Scene Description"
    }
    fn extensions(&self) -> &[&str] {
        &["usd", "usda", "usdc", "usdz"]
    }
    /// A usda or usdc layer by its signature (a usdz is a ZIP, which its
    /// extension tells apart).
    fn detects(&self, head: &[u8]) -> bool {
        matches!(
            usd::Format::detect(head),
            Some(usd::Format::Usda | usd::Format::Usdc)
        )
    }
    fn produces(&self) -> &[DataTypeId] {
        TYPES
    }
    /// Textures on disk stay paths unless embedded; a package's are
    /// embedded whatever it says.
    fn options(&self) -> FormatOptions {
        FormatOptions::from([(options::EMBED.to_owned(), Value::Bool(false))])
    }
    fn schema(&self, chosen: &FormatOptions, _: Purpose) -> Schema {
        let on = options::option(chosen, options::EMBED, Value::bool).unwrap_or(false);
        options::embed(Schema::default(), on)
    }
    fn read(
        &self,
        input: &Input<'_>,
        options: &FormatOptions,
        reading: &Reading<'_>,
    ) -> Result<Imported, Error> {
        read::read(input, options, reading)
    }
    fn writer(&self) -> Option<&dyn FormatWriter> {
        Some(&write::Writer)
    }
}
