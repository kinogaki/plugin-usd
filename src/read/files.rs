//! The files a stage names (textures, a dome's image) as the files Image
//! nodes read: a package's entries as ranges of the file read, never
//! copied; files on disk reached through the SDK's one rule ([`Named`]):
//! by their path, or read in with Embed Textures, and nothing outside the
//! file's folder unless the user asked.
use crate::names::file_name;
use crate::usd::package::Buffer;
use crate::usd::scene::{AssetSource, Scene};
use crate::usd::sdf::Token;
use kinogaki_studio_api::format::files::Named;
use kinogaki_studio_api::geometry::EmbeddedFile;
use kinogaki_studio_api::geometry::image::ImageFile;
use std::collections::HashMap;
use std::sync::Arc;

/// The file read, as the stage holds it: what a package's entries are
/// ranges of.
pub(crate) struct Held {
    /// The file, shared.
    pub file: EmbeddedFile,
    /// The same bytes as the buffer the stage reads, which its entries
    /// name.
    pub buffer: Buffer,
}

/// The bytes of an [`EmbeddedFile`], as a buffer the stage reads.
struct Bytes(EmbeddedFile);

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        self.0.bytes()
    }
}

impl Held {
    /// `file`, the input read, as the stage's buffer.
    pub fn new(file: EmbeddedFile) -> Held {
        let buffer: Buffer = Arc::new(Bytes(file.clone()));
        Held { file, buffer }
    }
}

/// The files of a scene, each made once.
pub(crate) struct Files<'a> {
    scene: &'a Scene,
    held: &'a Held,
    named: &'a Named,
    made: HashMap<Token, Option<ImageFile>>,
}

impl<'a> Files<'a> {
    /// The files `scene` names; `held` the file read; `named` finds those
    /// on disk.
    pub fn new(scene: &'a Scene, held: &'a Held, named: &'a Named) -> Files<'a> {
        Files {
            scene,
            held,
            named,
            made: HashMap::new(),
        }
    }

    /// The file the scene names by `path` (an anchored asset path); none
    /// for one left unresolved, which `named`'s warnings list.
    pub fn file(&mut self, path: &Token) -> Option<ImageFile> {
        if let Some(made) = self.made.get(path) {
            return made.clone();
        }
        let made = self.make(path);
        self.made.insert(path.clone(), made.clone());
        made
    }

    fn make(&mut self, path: &Token) -> Option<ImageFile> {
        let Some(asset) = self.scene.asset(path) else {
            return self.named.image(path, None);
        };
        match &asset.source {
            // A package's entry: a range of the file read, or of another
            // package the stage read (copied, it being no file of ours).
            AssetSource::Bytes(bytes) => {
                let name = file_name(path);
                let (buffer, range) = bytes.buffer();
                let file = match Arc::ptr_eq(buffer, &self.held.buffer) {
                    true => self.held.file.slice(name, range),
                    false => Ok(EmbeddedFile::new(name, bytes.to_vec())),
                };
                Some(ImageFile::Embedded(
                    file.expect("an entry lies within its package"),
                ))
            }
            AssetSource::File(file) => self.named.image(&file.to_string_lossy(), None),
            // Said by the scene (an entry a package lacks, a file read from
            // memory): as authored, where the rule finds it.
            AssetSource::Missing => self.named.image(&asset.authored, None),
        }
    }
}
