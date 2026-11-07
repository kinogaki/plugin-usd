//! Universal Scene Description: the format's own model, with no SDK in it
//! but the process's workers.
//!
//! - [`sdf`]: the data model, after OpenUSD's Sdf: a layer is specs keyed by
//!   path, each a list of fields.
//! - [`text`], [`crate_file`], [`package`]: a layer as usda text, a usdc
//!   crate, and a usdz package with the files it carries.
//! - [`compose`]: stages composed and flattened, after Pcp and UsdStage.
//! - [`scene`]: the scene a stage describes, read and authored as plain
//!   data; [`shade`]: its materials as UsdShade networks.
//!
//! The text writer prints layers exactly as OpenUSD's `usdcat` does.

pub mod compose;
pub mod crate_file;
mod error;
mod layer_file;
pub mod package;
mod parallel;
pub mod scene;
pub mod sdf;
pub mod shade;
pub mod text;

pub use compose::{FlattenOptions, Stage, StageOptions};
pub use error::{Error, ErrorKind, Result};
pub use layer_file::{
    Format, layer_bytes, open_flattened, read_layer, read_layer_bytes, write_layer,
};
