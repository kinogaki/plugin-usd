//! The files a written scene names. A usdz packs them: an embedded file's
//! bytes (shared, not copied) and a file on disk's, under `textures/`, equal
//! bytes one entry whatever their names. A usda or usdc names them as every
//! written document does ([`Beside`]): a file on disk by its path relative
//! to the layer, an embedded file written into `<stem>_textures` beside it.
use crate::names::file_name;
use crate::usd::package::{Buffer, Bytes};
use crate::usd::scene::{Asset, AssetSource};
use crate::usd::sdf::Token;
use kinogaki_studio_api::Error;
use kinogaki_studio_api::format::files::Beside;
use kinogaki_studio_api::geometry::image::{EmbeddedFile, ImageFile};
use std::collections::HashSet;
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
#[derive(Default)]
pub(super) struct Packed {
    assets: Vec<Asset>,
    /// Each embedded file's asset path, by its bytes.
    embedded: Vec<(EmbeddedFile, Token)>,
    names: HashSet<String>,
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

    /// The assets the scene holds: a usdz's, packed.
    pub fn assets(self) -> Vec<Asset> {
        match self {
            Assets::Packed(packed) => packed.assets,
            Assets::Beside(_) => Vec::new(),
        }
    }
}

impl Packed {
    fn file(&mut self, file: &ImageFile) -> Token {
        match file {
            ImageFile::Path(path) => {
                let token = Token::new(path);
                if !self.assets.iter().any(|a| a.path == token) {
                    let source = match Path::new(&**path).is_file() {
                        true => AssetSource::File(path.to_string().into()),
                        false => AssetSource::Missing,
                    };
                    self.assets.push(Asset {
                        path: token.clone(),
                        authored: token.clone(),
                        source,
                    });
                }
                token
            }
            ImageFile::Embedded(file) => {
                let same = |held: &EmbeddedFile| {
                    held.bytes().len() == file.bytes().len() && held.hash() == file.hash()
                };
                if let Some((_, token)) = self.embedded.iter().find(|(held, _)| same(held)) {
                    return token.clone();
                }
                let token = Token::new(&self.name(file_name(file.name())));
                let buffer: Buffer = Arc::new(Held(file.clone()));
                self.assets.push(Asset {
                    path: token.clone(),
                    authored: token.clone(),
                    source: AssetSource::Bytes(Bytes::shared(buffer)),
                });
                self.embedded.push((file.clone(), token.clone()));
                token
            }
        }
    }

    /// `name`, or `<stem>_<n>.<extension>` when another file took it.
    fn name(&mut self, name: &str) -> String {
        let (stem, extension) = match name.rfind('.') {
            Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
            _ => (name, ""),
        };
        let mut taken = name.to_owned();
        let mut copy = 1;
        while !self.names.insert(taken.clone()) {
            taken = format!("{stem}_{copy}{extension}");
            copy += 1;
        }
        taken
    }
}
