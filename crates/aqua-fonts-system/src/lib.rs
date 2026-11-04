use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use skrifa::attribute::Style as SkrifaStyle;
use skrifa::raw::FileRef;
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ID(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    Normal,
    Italic,
    Oblique,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Weight(pub u16);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stretch(pub f32);

impl Stretch {
    pub const CONDENSED: Stretch = Stretch(0.75);
    pub const NORMAL: Stretch = Stretch(1.0);
    pub const EXPANDED: Stretch = Stretch(1.25);
}

#[derive(Clone, Debug)]
pub enum Source {
    File(PathBuf),
    Memory,
}

#[derive(Clone, Debug)]
pub struct FaceInfo {
    pub id: ID,
    pub source: Source,
    pub index: u32,
    pub families: Vec<String>,
    pub post_script_name: String,
    pub style: Style,
    pub weight: Weight,
    pub stretch: Stretch,
}

#[derive(Clone, Copy, Debug)]
pub struct Query<'a> {
    pub family: &'a str,
    pub weight: Weight,
    pub stretch: Stretch,
    pub style: Style,
}

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

struct Mapped(memmap2::Mmap);

impl AsRef<[u8]> for Mapped {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

struct Record {
    info: FaceInfo,
    data: Arc<dyn AsRef<[u8]> + Send + Sync>,
}

pub struct FontDatabase {
    records: Vec<Record>,
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
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    Some(base.join("aqua").join("font-fallback.index"))
}

pub fn system_font_directories() -> Vec<PathBuf> {
    let windows = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(|root| PathBuf::from(root).join("Fonts"));
    let user = std::env::var_os("LOCALAPPDATA").map(|base| {
        PathBuf::from(base)
            .join("Microsoft")
            .join("Windows")
            .join("Fonts")
    });
    let configured = std::env::var_os("AQUA_FONT_DIRS")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    windows.into_iter().chain(user).chain(configured).collect()
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["ttf", "otf", "ttc", "otc"]
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

fn string_of(font: &FontRef, id: StringId) -> Option<String> {
    font.localized_strings(id)
        .english_or_first()
        .map(|string| string.chars().collect())
}

fn family_names(font: &FontRef) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for id in [StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME] {
        let mut localized: Vec<(bool, String)> = font
            .localized_strings(id)
            .map(|string| {
                let english = string
                    .language()
                    .is_none_or(|language| language.starts_with("en"));
                (!english, string.chars().collect::<String>())
            })
            .collect();
        localized.sort_by_key(|(foreign, _)| *foreign);
        for (_, name) in localized {
            if !name.is_empty() && !names.iter().any(|known| known.eq_ignore_ascii_case(&name)) {
                names.push(name);
            }
        }
    }
    names
}

fn describe(font: &FontRef, index: u32, source: Source) -> Option<FaceInfo> {
    let families = family_names(font);
    if families.is_empty() {
        return None;
    }
    let attributes = font.attributes();
    let style = match attributes.style {
        SkrifaStyle::Normal => Style::Normal,
        SkrifaStyle::Italic => Style::Italic,
        SkrifaStyle::Oblique(_) => Style::Oblique,
    };
    Some(FaceInfo {
        id: ID(0),
        source,
        index,
        families,
        post_script_name: string_of(font, StringId::POSTSCRIPT_NAME).unwrap_or_default(),
        style,
        weight: Weight(attributes.weight.value().round().clamp(1.0, 1000.0) as u16),
        stretch: Stretch(attributes.stretch.ratio()),
    })
}

fn style_order(wanted: Style) -> [Style; 3] {
    match wanted {
        Style::Normal => [Style::Normal, Style::Oblique, Style::Italic],
        Style::Italic => [Style::Italic, Style::Oblique, Style::Normal],
        Style::Oblique => [Style::Oblique, Style::Italic, Style::Normal],
    }
}

fn weight_rank(wanted: u16, candidate: u16) -> (u8, u16) {
    match wanted {
        400..=500 => match candidate {
            c if c >= wanted && c <= 500 => (0, c - wanted),
            c if c < wanted => (1, wanted - c),
            c => (2, c - 500),
        },
        w if w < 400 => match candidate {
            c if c <= w => (0, w - c),
            c => (1, c - w),
        },
        w => match candidate {
            c if c >= w => (0, c - w),
            c => (1, w - c),
        },
    }
}

impl FontDatabase {
    pub fn empty() -> Self {
        Self {
            records: Vec::new(),
            fingerprints: HashMap::new(),
        }
    }

    pub fn system() -> Self {
        let mut database = Self::empty();
        for directory in system_font_directories() {
            database.load_directory(&directory);
        }
        database
    }

    fn load_directory(&mut self, directory: &Path) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| is_font_file(path))
            .collect();
        paths.sort();
        for path in paths {
            let _ = self.load_file(&path);
        }
    }

    fn insert(&mut self, data: Arc<dyn AsRef<[u8]> + Send + Sync>, source: Source) -> usize {
        let before = self.records.len();
        let faces: Vec<(u32, FaceInfo)> = {
            let bytes: &[u8] = (*data).as_ref();
            match FileRef::new(bytes) {
                Ok(FileRef::Font(font)) => describe(&font, 0, source.clone())
                    .map(|info| (0, info))
                    .into_iter()
                    .collect(),
                Ok(FileRef::Collection(collection)) => (0..collection.len())
                    .filter_map(|index| {
                        let font = collection.get(index).ok()?;
                        describe(&font, index, source.clone()).map(|info| (index, info))
                    })
                    .collect(),
                Err(_) => Vec::new(),
            }
        };
        for (_, mut info) in faces {
            info.id = ID(self.records.len() as u32);
            self.records.push(Record {
                info,
                data: data.clone(),
            });
        }
        self.records.len() - before
    }

    pub fn load_bytes(&mut self, data: Vec<u8>) {
        self.insert(Arc::new(data), Source::Memory);
    }

    pub fn load_file(&mut self, path: impl AsRef<Path>) -> Result<(), std::io::Error> {
        let path = path.as_ref();
        let file = std::fs::File::open(path)?;
        let mapped = unsafe { memmap2::Mmap::map(&file) }?;
        let added = self.insert(Arc::new(Mapped(mapped)), Source::File(path.to_path_buf()));
        match added {
            0 => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "no usable font face",
            )),
            _ => Ok(()),
        }
    }

    pub fn face_count(&self) -> usize {
        self.records.len()
    }

    pub fn faces(&self) -> impl Iterator<Item = &FaceInfo> + '_ {
        self.records.iter().map(|record| &record.info)
    }

    pub fn face(&self, id: ID) -> Option<&FaceInfo> {
        self.records.get(id.0 as usize).map(|record| &record.info)
    }

    pub fn query(&self, query: &Query) -> Option<ID> {
        let mut candidates: Vec<&FaceInfo> = self
            .faces()
            .filter(|face| {
                face.families
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(query.family))
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let stretch_distance = |face: &FaceInfo| (face.stretch.0 - query.stretch.0).abs();
        let best_stretch = candidates
            .iter()
            .map(|face| stretch_distance(face))
            .fold(f32::MAX, f32::min);
        candidates.retain(|face| stretch_distance(face) <= best_stretch + 1e-4);
        let style = style_order(query.style)
            .into_iter()
            .find(|style| candidates.iter().any(|face| face.style == *style))?;
        candidates.retain(|face| face.style == style);
        candidates
            .into_iter()
            .min_by_key(|face| (weight_rank(query.weight.0, face.weight.0), face.id))
            .map(|face| face.id)
    }

    pub fn fingerprint(&mut self, id: ID) -> Option<FaceFingerprint> {
        if let Some(found) = self.fingerprints.get(&id) {
            return found.clone();
        }
        let computed = self.face(id).and_then(|face| {
            let Source::File(path) = &face.source else {
                return None;
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
        let record = self.records.get(id.0 as usize)?;
        Some(FaceBytes {
            data: record.data.clone(),
            index: record.info.index,
        })
    }
}

pub fn weight_distance_order(wanted: u16, candidates: &[u16]) -> Vec<u16> {
    let mut ordered = candidates.to_vec();
    ordered.sort_by_key(|candidate| weight_rank(wanted, *candidate));
    ordered
}
