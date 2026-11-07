// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! A composed stage, after `UsdStage::Open`: the root layer and its layer
//! stack (with an anonymous session layer, as UsdStage makes one), composed
//! on demand prim by prim, and flattened into one layer.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::assets;
use super::composer::{ANONYMOUS_SESSION, Composer, ForeignArc};
use super::flatten::{FlattenOptions, flatten_stage};
use super::graph::PrimIndex;
use super::layers::Layers;
use crate::usd::error::{Error, Result};
use crate::usd::package::Bytes;
use crate::usd::sdf::{LayerData, Path, Token, Value, field};

/// What a stage opens with.
#[derive(Clone, Debug, Default)]
pub struct StageOptions {
    /// Variant selections authored in the session layer before composing:
    /// paths ending in selections (`/Kitchen{look=clean}`,
    /// `/Chair{size=big}{color=red}`).
    pub variants: Vec<Path>,
    /// When not empty, payloads load only for prims whose path matches one
    /// of these globs (`*` any run, `?` one character); `-` loads none.
    pub load_payloads: Vec<String>,
    /// Payloads of prims whose path text starts with one of these are left
    /// unloaded.
    pub unload_payloads: Vec<String>,
    /// Variant fallbacks, (set, selection), preferred first, for sets with
    /// no authored selection.
    pub variant_fallbacks: Vec<(String, String)>,
    /// Raised once the stage is no longer wanted: flattening and reading
    /// its scene then stop with a [`Cancelled`](crate::usd::ErrorKind::Cancelled)
    /// error.
    pub cancel: Option<Arc<AtomicBool>>,
}

/// A composed stage.
pub struct Stage {
    composer: Composer,
    cancel: Option<Arc<AtomicBool>>,
}

impl Stage {
    /// Open the stage rooted at the layer `path`, every payload loaded.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Stage> {
        Stage::open_with(path, &StageOptions::default())
    }

    /// Open the stage rooted at the layer `path` with `options`.
    pub fn open_with(path: impl AsRef<std::path::Path>, options: &StageOptions) -> Result<Stage> {
        let identifier = assets::absolute(&path.as_ref().to_string_lossy());
        Stage::compose(Layers::on_disk(), &identifier, options)
    }

    /// Open the stage rooted at the layer `path` with `options`, its bytes
    /// given (read once by the caller); every other file it names is read
    /// from disk.
    pub fn open_held(
        path: impl AsRef<std::path::Path>,
        bytes: Bytes,
        options: &StageOptions,
    ) -> Result<Stage> {
        let identifier = assets::absolute(&path.as_ref().to_string_lossy());
        let mut layers = Layers::on_disk();
        layers.hold(&identifier, bytes);
        Stage::compose(layers, &identifier, options)
    }

    /// Open the stage rooted at a file given in memory, `bytes` named
    /// `name` (a usda, usdc or usdz by its signature). It has no place on
    /// disk, so it reads no file beside it: what it names outside itself
    /// (or, for a package, outside the package) is missing.
    pub fn open_memory(name: &str, bytes: Bytes, options: &StageOptions) -> Result<Stage> {
        let identifier = assets::normalize(&format!("/{name}"));
        let mut layers = Layers::in_memory();
        layers.hold(&identifier, bytes);
        Stage::compose(layers, &identifier, options)
    }

    fn compose(layers: Layers, identifier: &str, options: &StageOptions) -> Result<Stage> {
        let mut composer = Composer::new(layers, identifier, Some(ANONYMOUS_SESSION))?;
        composer.cancel = options.cancel.clone();
        for selection in &options.variants {
            composer.select_variants_at(selection)?;
        }
        for pattern in &options.load_payloads {
            composer.include_payloads_matching(pattern);
        }
        for prefix in &options.unload_payloads {
            composer.exclude_payloads(prefix);
        }
        for (set, selection) in &options.variant_fallbacks {
            composer.add_variant_fallback(set, selection);
        }
        Ok(Stage {
            composer,
            cancel: options.cancel.clone(),
        })
    }

    /// `Err` of kind [`Cancelled`](crate::usd::ErrorKind::Cancelled) once its
    /// [`StageOptions::cancel`] is raised.
    pub fn check(&self) -> Result<()> {
        cancelled(self.cancel.as_deref())
    }

    /// What raises its cancellation ([`StageOptions::cancel`]).
    pub(crate) fn cancel(&self) -> Option<Arc<AtomicBool>> {
        self.cancel.clone()
    }

    /// The bytes of the file `identifier` names, when the stage holds them:
    /// an entry of a package (a texture in a usdz), or a file given in
    /// memory. None for a file on disk outside packages, or a missing one.
    pub fn bytes(&mut self, identifier: &str) -> Option<Bytes> {
        self.composer.layers.bytes(identifier)
    }

    /// Whether the file `identifier` names exists: held, or on disk.
    pub fn exists(&self, identifier: &str) -> bool {
        self.composer.layers.exists(identifier)
    }

    /// The arcs met so far to files of a format composition does not read
    /// (a MaterialX document referenced as a layer), for their readers.
    pub fn foreign_arcs(&self) -> &[ForeignArc] {
        &self.composer.foreign
    }

    /// The root layer.
    pub fn root_layer(&self) -> &LayerData {
        self.composer.layers.layer(self.composer.root_layer_index())
    }

    /// The root layer's identifier (its normalized absolute path).
    pub fn root_identifier(&self) -> &str {
        self.composer
            .layers
            .identifier(self.composer.root_layer_index())
    }

    /// Composition errors met so far, one line each.
    pub fn warnings(&self) -> Vec<String> {
        self.composer.problems.lines().to_vec()
    }

    /// Whether the stage is its root layer alone (no sublayers, arcs or
    /// session opinions): the root layer's specs are the stage's.
    pub fn is_single_layer(&self) -> bool {
        self.composer.is_single_layer()
    }

    /// The composed stage as one layer, as `usdcat --flatten` writes it
    /// (instances referencing prototypes, the "Generated from" note).
    pub fn flatten(&mut self) -> Result<LayerData> {
        self.flatten_with(FlattenOptions {
            comment: true,
            instancing: true,
        })
    }

    /// The composed stage as one layer, with `options`.
    pub fn flatten_with(&mut self, options: FlattenOptions) -> Result<LayerData> {
        flatten_stage(&mut self.composer, options)
    }

    /// The prim index of `path` (a prim path of the stage).
    pub fn prim_index(&mut self, path: &Path) -> Result<Arc<PrimIndex>> {
        self.composer.index(path)
    }

    /// The composed child names of the prim at `path`.
    pub fn child_names(&mut self, path: &Path) -> Result<Vec<Token>> {
        let index = self.composer.index(path)?;
        Ok(self.composer.child_names(&index))
    }

    /// The root layer's metadata field `name`.
    fn layer_metadata(&self, name: &str) -> Option<&Value> {
        self.root_layer().root().get(name)
    }

    /// `upAxis` (`Y` when unauthored, as UsdGeom's fallback).
    pub fn up_axis(&self) -> Token {
        self.layer_metadata("upAxis")
            .and_then(Value::as_token)
            .cloned()
            .unwrap_or_else(|| Token::new("Y"))
    }

    /// `metersPerUnit` (0.01 when unauthored, as UsdGeom's fallback).
    pub fn meters_per_unit(&self) -> f64 {
        self.layer_metadata("metersPerUnit")
            .and_then(Value::as_f64)
            .unwrap_or(0.01)
    }

    /// `startTimeCode`, when authored.
    pub fn start_time_code(&self) -> Option<f64> {
        self.layer_metadata(field::START_TIME_CODE)
            .and_then(Value::as_f64)
    }

    /// `endTimeCode`, when authored.
    pub fn end_time_code(&self) -> Option<f64> {
        self.layer_metadata(field::END_TIME_CODE)
            .and_then(Value::as_f64)
    }

    /// `timeCodesPerSecond` (framesPerSecond, else 24, when unauthored).
    pub fn time_codes_per_second(&self) -> f64 {
        self.composer
            .layers
            .time_codes_per_second(self.composer.root_layer_index())
    }
}

/// `Err` of kind [`Cancelled`](crate::usd::ErrorKind::Cancelled) once `cancel`
/// is raised.
pub(crate) fn cancelled(cancel: Option<&AtomicBool>) -> Result<()> {
    match cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
        true => Err(Error::cancelled()),
        false => Ok(()),
    }
}
