//! The files a stage names (textures, a dome's image) as the files Image
//! nodes read: a package's entries as ranges of the file read, never
//! copied; files on disk by their path, or read in with Embed Textures.
use crate::names::file_name;
use crate::usd::package::Buffer;
use crate::usd::scene::{AssetSource, Scene};
use crate::usd::sdf::Token;
use kinogaki_studio_api::geometry::image::{EmbeddedFile, ImageFile};
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
    /// Read files on disk into the project (Embed Textures).
    embed: bool,
    /// The stage was read from a file on disk, which places what it names.
    on_disk: bool,
    made: HashMap<Token, ImageFile>,
    /// What could not be read, one line each.
    pub warnings: Vec<String>,
}

impl<'a> Files<'a> {
    /// The files `scene` names; `held` the file read, from disk when
    /// `on_disk`; `embed`: files on disk are read in.
    pub fn new(scene: &'a Scene, held: &'a Held, on_disk: bool, embed: bool) -> Files<'a> {
        Files {
            scene,
            held,
            embed,
            on_disk,
            made: HashMap::new(),
            warnings: Vec::new(),
        }
    }

    /// The file the scene names by `path` (an anchored asset path).
    pub fn file(&mut self, path: &Token) -> ImageFile {
        if let Some(made) = self.made.get(path) {
            return made.clone();
        }
        let made = self.make(path);
        self.made.insert(path.clone(), made.clone());
        made
    }

    fn make(&mut self, path: &Token) -> ImageFile {
        let Some(asset) = self.scene.asset(path) else {
            return ImageFile::Path(Arc::from(path.as_str()));
        };
        let name = file_name(path);
        match &asset.source {
            // A package's entry: a range of the file read, or of another
            // package the stage read (copied, it being no file of ours).
            AssetSource::Bytes(bytes) => {
                let (buffer, range) = bytes.buffer();
                let file = match Arc::ptr_eq(buffer, &self.held.buffer) {
                    true => self.held.file.slice(name, range),
                    false => Ok(EmbeddedFile::new(name, bytes.to_vec())),
                };
                ImageFile::Embedded(file.expect("an entry lies within its package"))
            }
            AssetSource::File(file) if self.embed => match std::fs::read(file) {
                Ok(bytes) => ImageFile::Embedded(EmbeddedFile::new(name, bytes)),
                Err(e) => {
                    let shown = file.display();
                    self.warnings.push(format!("{shown}: {e}, kept as a path"));
                    ImageFile::Path(Arc::from(file.to_string_lossy().as_ref()))
                }
            },
            AssetSource::File(file) => ImageFile::Path(Arc::from(file.to_string_lossy().as_ref())),
            // Said by the scene. Kept where the stage places it, or as
            // authored where it places nothing (a file read from memory, an
            // entry a package lacks), read from the project's folder.
            AssetSource::Missing => {
                let placed = self.on_disk && !path.contains('[');
                let kept = if placed {
                    path.as_str()
                } else {
                    &asset.authored
                };
                ImageFile::Path(Arc::from(kept))
            }
        }
    }
}
