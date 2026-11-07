//! Universal Scene Description as a format for Kinogaki Studio.
//!
//! [`usd`] is the format's own model, parser and writer, and uses nothing of
//! the SDK but its workers: layers read and written as usda text, usdc
//! crates and usdz packages, stages composed (sublayers, references,
//! payloads, variants, inherits, specializes, relocates, instancing), and the
//! scene a stage describes (meshes, curves, points, implicit shapes,
//! instances, cameras, lights and UsdShade materials) read and authored as
//! plain data.
pub mod usd;
