//! The files a written scene names. A usdz packs them: an embedded file's
//! bytes (shared, not copied) and a picture on disk's, under `textures/`,
//! equal bytes one entry whatever their names. It packs only pictures of
//! the kinds a usdz holds (PNG, JPEG, OpenEXR, AVIF, by their bytes): any
//! other file an Image node names stays a path, said. A usda or usdc names
//! them as every written document does ([`Beside`]): a file on disk by its
//! path relative to the layer, an embedded file written into
//! `<stem>_textures` beside it.
use crate::names::file_name;
use crate::usd::package::{Buffer, Bytes, packable};
use crate::usd::scene::{Asset, AssetSource};
use crate::usd::sdf::Token;
use kinogaki_studio_api::Error;
use kinogaki_studio_api::format::Unique;
use kinogaki_studio_api::format::files::Beside;
use kinogaki_studio_api::geometry::EmbeddedFile;
use kinogaki_studio_api::geometry::image::ImageFile;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// The bytes of an embedded file, as a buffer a written package reads.
struct Held(EmbeddedFile);

impl AsRef<[u8]> for Held {
    fn as_ref(&self) -> &[u8] {
        self.0.bytes()
    }
}

/// The files of a scene being written.
pub(super) enum Assets {
    /// Packed in a usdz: the scene's assets.
    Packed(Packed),
    /// Named beside a usda or usdc layer.
    Beside(Beside),
}

/// The assets a usdz packs.
pub(super) struct Packed {
    assets: Vec<Asset>,
    /// Each file on disk's asset path, made once.
    paths: HashMap<Arc<str>, Token>,
    /// Each embedded file's asset path, by its bytes' hash.
    embedded: HashMap<[u8; 32], Token>,
    /// The packed files' names, ignoring case.
    names: Unique,
    /// Files on disk left as paths: no picture a usdz holds.
    pub left: Vec<String>,
}

impl Default for Packed {
    fn default() -> Self {
        Packed {
            assets: Vec::new(),
            paths: HashMap::new(),
            embedded: HashMap::new(),
            names: Unique::files(),
            left: Vec::new(),
        }
    }
}

impl Assets {
    /// The files of a layer written at `path`: packed when it is a usdz.
    pub fn of(path: &Path) -> Assets {
        let usdz = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("usdz"));
        match usdz {
            true => Assets::Packed(Packed::default()),
            false => Assets::Beside(Beside::new(path)),
        }
    }

    /// The asset path a shader input or a dome names `file` by.
    ///
    /// # Errors
    /// An embedded file that can't be written beside the layer.
    pub fn file(&mut self, file: &ImageFile) -> Result<Token, Error> {
        match self {
            Assets::Packed(packed) => Ok(packed.file(file)),
            Assets::Beside(beside) => beside.name(file).map(|name| Token::new(&name)),
        }
    }

    /// The assets the scene holds (a usdz's, packed), and what was left
    /// unpacked, said.
    pub fn assets(self) -> (Vec<Asset>, Vec<String>) {
        match self {
            Assets::Packed(packed) => (packed.assets, packed.left),
            Assets::Beside(_) => (Vec::new(), Vec::new()),
        }
    }
}

impl Packed {
    fn file(&mut self, file: &ImageFile) -> Token {
        match file {
            ImageFile::Path(path) => {
                if let Some(token) = self.paths.get(path) {
                    return token.clone();
                }
                let token = Token::new(path);
                let source = match packable(Path::new(&**path)) {
                    true => AssetSource::File(path.to_string().into()),
                    false => {
                        self.left.push(format!(
                            "{path}: no picture a usdz holds (PNG, JPEG, OpenEXR or AVIF); kept as a path, not packed"
                        ));
                        AssetSource::Missing
                    }
                };
                self.assets.push(Asset {
                    path: token.clone(),
                    authored: token.clone(),
                    source,
                });
                self.paths.insert(path.clone(), token.clone());
                token
            }
            ImageFile::Embedded(file) => {
                if let Some(token) = self.embedded.get(file.hash()) {
                    return token.clone();
                }
                let token = Token::new(&self.names.take(file_name(file.name())));
                let buffer: Buffer = Arc::new(Held(file.clone()));
                self.assets.push(Asset {
                    path: token.clone(),
                    authored: token.clone(),
                    source: AssetSource::Bytes(Bytes::shared(buffer)),
                });
                self.embedded.insert(*file.hash(), token.clone());
                token
            }
        }
    }
}
