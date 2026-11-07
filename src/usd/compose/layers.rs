// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The layers a composition opens, each once (`SdfLayer::FindOrOpen`'s
//! registry, which finds a layer by its identifier and by its resolved
//! path), by identity: a file on disk by its canonical path, which
//! [`Named`] gives, however an identifier spells it (a link, another case
//! on a disk that ignores case); a layer inside a package
//! (`/a/b.usdz[c.usdc]`) by its package's identity and its path in it.
//! Each is read by its signature whatever its extension says. A layer that
//! cannot be read is remembered as failed, with the reason.
//!
//! Files given in memory (the file read, an embedded file, which has no
//! path) stand in for files of their name. Every other file is reached on
//! disk through the SDK's one rule for the files a file names
//! ([`Named`]: the read file's folder and below, regular files, within the
//! read's budget); a composition given none reads nothing from disk. Only a
//! `.usdz` opens as a package, and each package's file is read once (every
//! layer and asset in it is a view of its bytes).

use std::collections::HashMap;
use std::sync::Arc;

use super::assets;
use crate::usd::error::{Error, Result};
use crate::usd::package::{Bytes, Package, is_package};
use crate::usd::sdf::{LayerData, Path, Token, Value, field};
use kinogaki_studio_api::format::ReadBudget;
use kinogaki_studio_api::format::files::Named;

/// The most layers one composition opens, against hostile reference webs.
const MOST_LAYERS: usize = 1 << 16;

struct Entry {
    identifier: String,
    layer: Option<Arc<LayerData>>,
    problem: String,
}

/// Every layer a composition has opened.
pub struct Layers {
    entries: Vec<Entry>,
    /// Each layer by its identity.
    by_identity: HashMap<String, usize>,
    /// Each identifier asked for, by what it names ([`Layers::identity`]).
    identities: HashMap<String, String>,
    /// Files given in memory, by their identifier.
    held: HashMap<String, Bytes>,
    /// Each package by its identity.
    packages: HashMap<String, Package>,
    /// The files on disk it may read; none reads nothing from disk.
    disk: Option<Arc<Named>>,
    budget: ReadBudget,
}

impl Layers {
    /// Layers read from the files held and, through `disk`, from disk,
    /// within `budget`.
    pub fn new(disk: Option<Arc<Named>>, budget: ReadBudget) -> Layers {
        Layers {
            entries: Vec::new(),
            by_identity: HashMap::new(),
            identities: HashMap::new(),
            held: HashMap::new(),
            packages: HashMap::new(),
            disk,
            budget,
        }
    }

    /// What reading may spend, and its cancellation.
    pub fn budget(&self) -> &ReadBudget {
        &self.budget
    }

    /// Whether files on disk are read.
    pub fn reads_disk(&self) -> bool {
        self.disk.is_some()
    }

    /// The identifier of layer `index`.
    pub fn identifier(&self, index: usize) -> &str {
        &self.entries[index].identifier
    }

    /// Layer `index` (only opened layers have indexes callers see).
    pub fn layer(&self, index: usize) -> &LayerData {
        self.entries[index]
            .layer
            .as_deref()
            .expect("an opened layer")
    }

    /// Layer `index` to change (a session layer being authored).
    pub fn layer_mut(&mut self, index: usize) -> &mut LayerData {
        Arc::make_mut(self.entries[index].layer.as_mut().expect("an opened layer"))
    }

    /// Why the layer `identifier` names could not be read ("" when it was
    /// read, or never tried).
    pub fn problem(&self, identifier: &str) -> &str {
        (self.identities.get(identifier))
            .and_then(|identity| self.by_identity.get(identity))
            .map_or("", |&at| self.entries[at].problem.as_str())
    }

    /// The layer `identifier` names, opened now if new; none when it cannot
    /// be read.
    pub fn find_or_open(&mut self, identifier: &str) -> Result<Option<usize>> {
        let identity = self.identity(identifier);
        if let Some(&at) = self.by_identity.get(&identity) {
            return Ok(self.entries[at].layer.is_some().then_some(at));
        }
        if self.entries.len() >= MOST_LAYERS {
            return Err(Error::limit("a composition opening more than 65536 layers"));
        }
        let (layer, problem) = match self.read(identifier) {
            Ok(layer) => (Some(Arc::new(layer)), String::new()),
            Err(error) => (None, error.message),
        };
        let opened = layer.is_some();
        let at = self.entries.len();
        self.entries.push(Entry {
            identifier: identifier.to_owned(),
            layer,
            problem,
        });
        self.by_identity.insert(identity, at);
        Ok(opened.then_some(at))
    }

    /// A new empty layer named `identifier` (an anonymous layer).
    pub fn anonymous(&mut self, identifier: &str) -> usize {
        let at = self.entries.len();
        self.entries.push(Entry {
            identifier: identifier.to_owned(),
            layer: Some(Arc::new(LayerData::new())),
            problem: String::new(),
        });
        (self.identities).insert(identifier.to_owned(), identifier.to_owned());
        self.by_identity.insert(identifier.to_owned(), at);
        at
    }

    /// What `identifier` names, one name for a file however it is spelled:
    /// a file on disk its canonical path, a package's entry its package's
    /// identity and its path in it; a file held, or one no disk reaches,
    /// its identifier. Found once an identifier.
    fn identity(&mut self, identifier: &str) -> String {
        if let Some(known) = self.identities.get(identifier) {
            return known.clone();
        }
        let identity = match assets::package_split(identifier) {
            Some((outer, inner)) => format!("{}[{inner}]", self.identity(outer)),
            None => (self.on_disk(identifier))
                .map_or_else(|| identifier.to_owned(), |p| p.display().to_string()),
        };
        (self.identities).insert(identifier.to_owned(), identity.clone());
        identity
    }

    /// The layer's metadata field `name` (on its pseudo-root).
    pub fn field(&self, index: usize, name: &str) -> Option<&Value> {
        self.layer(index).root().get(name)
    }

    /// The layer's time codes per second (framesPerSecond, else 24, when
    /// unauthored), as `SdfLayer::GetTimeCodesPerSecond`.
    pub fn time_codes_per_second(&self, index: usize) -> f64 {
        self.field(index, field::TIME_CODES_PER_SECOND)
            .or_else(|| self.field(index, field::FRAMES_PER_SECOND))
            .and_then(Value::as_f64)
            .unwrap_or(24.0)
    }

    /// The token metadata field `name` of the layer.
    pub fn token_field(&self, index: usize, name: &str) -> Option<Token> {
        self.field(index, name).and_then(Value::as_token).cloned()
    }

    /// Whether the layer has a spec at `path`.
    pub fn has_spec(&self, index: usize, path: &Path) -> bool {
        self.layer(index).has_spec(path)
    }

    /// Hold `bytes` as the file `identifier` names: it is read from them,
    /// never from disk.
    pub fn hold(&mut self, identifier: &str, bytes: Bytes) {
        self.held.insert(identifier.to_owned(), bytes);
    }

    /// Whether the file an identifier names exists: held, or on disk in
    /// reach (a package's outer file).
    pub fn exists(&self, identifier: &str) -> bool {
        let outer = assets::outermost(identifier);
        self.held.contains_key(outer) || self.on_disk(outer).is_some()
    }

    /// The bytes of the file `identifier` names when they are held: a file
    /// given in memory, or an entry of a package (whose file is read once);
    /// none for a file on disk outside packages, or one that can't be read.
    pub fn bytes(&mut self, identifier: &str) -> Option<Bytes> {
        match assets::package_split(identifier) {
            None => self.held.get(identifier).cloned(),
            Some((outer, inner)) => self.package(outer).ok()?.find(inner).ok(),
        }
    }

    /// Where `identifier`, a plain file's, is on disk: a regular file in
    /// reach, by its canonical path. None when nothing on disk is read.
    fn on_disk(&self, identifier: &str) -> Option<std::path::PathBuf> {
        self.disk.as_ref()?.file(identifier, None)
    }

    /// The bytes of the plain file `identifier`: held, else read from disk.
    fn file_bytes(&self, identifier: &str) -> Result<Bytes> {
        if let Some(bytes) = self.held.get(identifier) {
            return Ok(bytes.clone());
        }
        let (Some(disk), Some(path)) = (&self.disk, self.on_disk(identifier)) else {
            return Err(Error::io("no such file"));
        };
        Ok(Bytes::from(disk.read(&path)?))
    }

    /// The package at `identifier` (an outer file, a `.usdz`), read once
    /// however it is spelled.
    fn package(&mut self, identifier: &str) -> Result<&Package> {
        let identity = self.identity(identifier);
        if !self.packages.contains_key(&identity) {
            if !is_package(identifier) {
                return Err(Error::invalid(format!(
                    "{identifier} is no usdz: only usdz files hold packaged files"
                )));
            }
            let package = Package::open(self.file_bytes(identifier)?)?;
            self.packages.insert(identity.clone(), package);
        }
        Ok(&self.packages[&identity])
    }

    /// Read the layer an identifier names.
    fn read(&mut self, identifier: &str) -> Result<LayerData> {
        let bytes = match assets::package_split(identifier) {
            Some((outer, inner)) => self.package(outer)?.find(inner)?,
            None => self.file_bytes(identifier)?,
        };
        crate::usd::read_layer_bytes(&bytes, &self.budget)
    }
}
