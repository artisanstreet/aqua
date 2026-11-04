use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

pub use fontdb::{FaceInfo, Family, ID, Query, Stretch, Style, Weight};

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

pub struct FontDatabase {
    db: fontdb::Database,
    shared: HashMap<ID, FaceBytes>,
}

impl FontDatabase {
    pub fn empty() -> Self {
        Self {
            db: fontdb::Database::new(),
            shared: HashMap::new(),
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
