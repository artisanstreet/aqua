use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

pub use fontdb::{FaceInfo, Family, Query, Stretch, Style, Weight, ID};

#[derive(Clone)]
pub struct FaceBytes {
    data: Arc<dyn AsRef<[u8]> + Send + Sync>,
    index: u32,
}

impl FaceBytes {
    pub fn bytes(&self) -> &[u8] {
        (*self.data).as_ref()
    }

    pub fn index(&self) -> u32 {
        self.index
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FaceFingerprint {
    pub path: String,
    pub index: u32,
    pub len: u64,
    pub modified: u64,
}

pub struct FontDatabase {
    db: fontdb::Database,
    shared: HashMap<ID, FaceBytes>,
    fingerprints: HashMap<ID, Option<FaceFingerprint>>,
}

pub fn read_cache_file(path: impl AsRef<Path>) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

pub fn write_cache_file(path: impl AsRef<Path>, bytes: &[u8]) -> std::io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let staging = path.with_extension("tmp");
    std::fs::write(&staging, bytes)?;
    std::fs::rename(&staging, path)
}

pub fn default_fallback_index_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(base.join("aqua").join("font-fallback.index"))
}

impl FontDatabase {
    pub fn empty() -> Self {
        Self {
            db: fontdb::Database::new(),
            shared: HashMap::new(),
            fingerprints: HashMap::new(),
        }
    }

    pub fn system() -> Self {
        let mut database = Self::empty();
        database.db.load_system_fonts();
        database
    }

    pub fn load_bytes(&mut self, data: Vec<u8>) {
        self.db.load_font_data(data);
    }

    pub fn load_file(&mut self, path: impl AsRef<Path>) -> Result<(), std::io::Error> {
        self.db.load_font_file(path)
    }

    pub fn set_generic_families(&mut self, sans: &str, serif: &str, monospace: &str) {
        self.db.set_sans_serif_family(sans);
        self.db.set_serif_family(serif);
        self.db.set_monospace_family(monospace);
    }

    pub fn face_count(&self) -> usize {
        self.db.len()
    }

    pub fn faces(&self) -> impl Iterator<Item = &FaceInfo> + '_ {
        self.db.faces()
    }

    pub fn face(&self, id: ID) -> Option<&FaceInfo> {
        self.db.face(id)
    }

    pub fn query(&self, query: &Query) -> Option<ID> {
        self.db.query(query)
    }

    pub fn family_name<'a>(&'a self, family: &'a Family<'a>) -> &'a str {
        self.db.family_name(family)
    }

    pub fn fingerprint(&mut self, id: ID) -> Option<FaceFingerprint> {
        if let Some(found) = self.fingerprints.get(&id) {
            return found.clone();
        }
        let computed = self.db.face(id).and_then(|face| {
            let path = match &face.source {
                fontdb::Source::File(path) => path,
                fontdb::Source::SharedFile(path, _) => path,
                fontdb::Source::Binary(_) => return None,
            };
            let metadata = std::fs::metadata(path).ok()?;
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |elapsed| elapsed.as_secs());
            Some(FaceFingerprint {
                path: path.to_string_lossy().into_owned(),
                index: face.index,
                len: metadata.len(),
                modified,
            })
        });
        self.fingerprints.insert(id, computed.clone());
        computed
    }

    pub fn face_bytes(&mut self, id: ID) -> Option<FaceBytes> {
        if let Some(found) = self.shared.get(&id) {
            return Some(found.clone());
        }
        let (data, index) = unsafe { self.db.make_shared_face_data(id) }?;
        let bytes = FaceBytes { data, index };
        self.shared.insert(id, bytes.clone());
        Some(bytes)
    }
}
