//! The files a written scene names: an embedded file's bytes (shared, not
//! copied) packed in a usdz or written beside a layer, a file on disk by its
//! path. Equal bytes are one asset whatever their names.
use crate::names::file_name;
use crate::usd::package::{Buffer, Bytes};
use crate::usd::scene::{Asset, AssetSource};
use crate::usd::sdf::Token;
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

/// The assets of a scene being written.
#[derive(Default)]
pub(super) struct Assets {
    pub assets: Vec<Asset>,
    /// Each embedded file's asset path, by its bytes.
    embedded: Vec<(EmbeddedFile, Token)>,
    names: HashSet<String>,
}

impl Assets {
    /// The asset path a shader input or a dome names `file` by.
    pub fn file(&mut self, file: &ImageFile) -> Token {
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
