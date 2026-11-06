use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use aqua_fonts_system::{FaceBytes, FontDatabase, Query, Stretch, Style, Weight, ID};
use skrifa::raw::TableProvider;
use skrifa::MetadataProvider;
use unicode_segmentation::UnicodeSegmentation;

use crate::fallback::{Coverage, FallbackIndex};
use crate::shape_cache::{ShapeCache, ShapeCacheStats};
use crate::style::{FontFamily, FontSlant, FontStretch, TextStyle};

pub type FontId = ID;

pub const SYNTHETIC_BOLD_FROM: u16 = 600;

const COLOR_FONT_POSTSCRIPT: [&str; 5] = [
    "NotoColorEmoji",
    "SegoeUIEmoji",
    "AppleColorEmoji",
    ".AppleColorEmojiUI",
    "TwemojiMozilla",
];

const EMOJI_FAMILIES: [&str; 4] = [
    "Segoe UI Emoji",
    "Noto Color Emoji",
    "Apple Color Emoji",
    "Twemoji Mozilla",
];

const PLATFORM_FALLBACK_FAMILIES: [&str; 12] = [
    "Segoe UI",
    "Segoe UI Symbol",
    "Segoe UI Historic",
    "Microsoft YaHei UI",
    "Yu Gothic UI",
    "Malgun Gothic",
    "Nirmala UI",
    "Ebrima",
    "Gadugi",
    "Noto Sans",
    "DejaVu Sans",
    "Arial Unicode MS",
];

const SANS_FAMILIES: [&str; 9] = [
    "Segoe UI",
    "Inter",
    "Open Sans",
    "Noto Sans",
    "DejaVu Sans",
    "Arial",
    "Liberation Sans",
    "Helvetica",
    "Ubuntu Sans",
];
const SERIF_FAMILIES: [&str; 6] = [
    "Times New Roman",
    "Georgia",
    "Noto Serif",
    "DejaVu Serif",
    "Liberation Serif",
    "Cambria",
];
const MONOSPACE_FAMILIES: [&str; 8] = [
    "Cascadia Mono",
    "Consolas",
    "Menlo",
    "Noto Sans Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Courier New",
    "Ubuntu Mono",
];
const CURSIVE_FAMILIES: [&str; 3] = ["Segoe Script", "Comic Sans MS", "Z003"];
const FANTASY_FAMILIES: [&str; 2] = ["Impact", "Papyrus"];

fn default_ignorable(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
            | '\u{e0100}'..='\u{e01ef}'
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntryKey {
    id: ID,
    weight: u16,
    stretch: FontStretch,
    fake_italic: bool,
}

pub struct PrewarmJob {
    key: EntryKey,
    revision: u64,
    face_weight: u16,
    postscript_name: String,
    bytes: FaceBytes,
}

#[derive(Clone)]
pub struct PrewarmedEntry {
    key: EntryKey,
    revision: u64,
    entry: Arc<FontEntry>,
}

impl PrewarmJob {
    pub fn face(&self) -> ID {
        self.key.id
    }

    pub fn build(self) -> Option<PrewarmedEntry> {
        let entry = FontEntry::build(
            self.key.id,
            self.bytes,
            self.key.weight,
            self.face_weight,
            self.key.stretch,
            self.key.fake_italic,
            self.postscript_name,
        )?;
        (0x20u8..0x7f).for_each(|byte| {
            entry.glyph_for(char::from(byte));
        });
        crate::shape::warm_entry(&entry);
        Some(PrewarmedEntry {
            key: self.key,
            revision: self.revision,
            entry: Arc::new(entry),
        })
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct PlanKey {
    direction: harfrust::Direction,
    script: harfrust::Script,
    features: Vec<(u32, u32, u32, u32)>,
}

impl PlanKey {
    pub(crate) fn new(
        direction: harfrust::Direction,
        script: harfrust::Script,
        features: &[harfrust::Feature],
    ) -> Self {
        Self {
            direction,
            script,
            features: features
                .iter()
                .map(|feature| {
                    (
                        u32::from_be_bytes(feature.tag.to_be_bytes()),
                        feature.value,
                        feature.start,
                        feature.end,
                    )
                })
                .collect(),
        }
    }
}

pub struct FontEntry {
    id: ID,
    weight: u16,
    face_weight: u16,
    synthetic_bold: bool,
    stretch: FontStretch,
    fake_italic: bool,
    bytes: FaceBytes,
    shaper_data: harfrust::ShaperData,
    shaper_instance: Option<harfrust::ShaperInstance>,
    location: skrifa::instance::Location,
    units_per_em: f32,
    ascent: f32,
    descent: f32,
    max_width: f32,
    space: f32,
    color: bool,
    postscript_name: String,
    glyphs: Mutex<HashMap<char, u16>>,
    plans: Mutex<HashMap<PlanKey, Arc<harfrust::ShapePlan>>>,
}

impl FontEntry {
    fn build(
        id: ID,
        bytes: FaceBytes,
        weight: u16,
        face_weight: u16,
        stretch: FontStretch,
        fake_italic: bool,
        postscript_name: String,
    ) -> Option<Self> {
        let font = skrifa::FontRef::from_index(bytes.bytes(), bytes.index()).ok()?;
        let axes = font.axes();
        let mut settings: Vec<(skrifa::Tag, f32)> = Vec::new();
        if axes.get_by_tag(skrifa::Tag::new(b"wght")).is_some() {
            settings.push((skrifa::Tag::new(b"wght"), f32::from(weight)));
        }
        if axes.get_by_tag(skrifa::Tag::new(b"wdth")).is_some() {
            let width = match stretch {
                FontStretch::Condensed => 75.0,
                FontStretch::Normal => 100.0,
                FontStretch::Expanded => 125.0,
            };
            settings.push((skrifa::Tag::new(b"wdth"), width));
        }
        let variable_weight = axes.get_by_tag(skrifa::Tag::new(b"wght")).is_some();
        let synthetic_bold =
            !variable_weight && weight >= SYNTHETIC_BOLD_FROM && face_weight < SYNTHETIC_BOLD_FROM;
        let location = axes.location(settings.iter().copied());
        let metrics = font.metrics(skrifa::instance::Size::unscaled(), &location);
        let units_per_em = f32::from(metrics.units_per_em.max(1));
        let harf_font = harfrust::FontRef::from_index(bytes.bytes(), bytes.index()).ok()?;
        let shaper_data = harfrust::ShaperData::new(&harf_font);
        let shaper_instance = (!settings.is_empty()).then(|| {
            harfrust::ShaperInstance::from_variations(
                &harf_font,
                settings.iter().map(|(tag, value)| harfrust::Variation {
                    tag: harfrust::Tag::new(&tag.to_be_bytes()),
                    value: *value,
                }),
            )
        });
        let space = font
            .charmap()
            .map(' ')
            .and_then(|glyph| {
                font.glyph_metrics(skrifa::instance::Size::unscaled(), &location)
                    .advance_width(glyph)
            })
            .map_or(0.25, |advance| advance / units_per_em);
        let color = [b"COLR", b"CBDT", b"sbix"]
            .iter()
            .any(|tag| font.data_for_tag(skrifa::Tag::new(tag)).is_some());
        Some(Self {
            id,
            weight,
            face_weight,
            synthetic_bold,
            stretch,
            fake_italic,
            shaper_data,
            shaper_instance,
            location,
            units_per_em,
            ascent: metrics.ascent / units_per_em,
            descent: -metrics.descent / units_per_em,
            max_width: metrics.max_width.unwrap_or(0.0) / units_per_em,
            space,
            color,
            postscript_name,
            glyphs: Mutex::new(HashMap::new()),
            plans: Mutex::new(HashMap::new()),
            bytes,
        })
    }

    pub fn id(&self) -> ID {
        self.id
    }

    pub fn weight(&self) -> u16 {
        self.weight
    }

    pub fn face_weight(&self) -> u16 {
        self.face_weight
    }

    pub fn synthetic_bold(&self) -> bool {
        self.synthetic_bold
    }

    pub fn stretch(&self) -> FontStretch {
        self.stretch
    }

    pub fn fake_italic(&self) -> bool {
        self.fake_italic
    }

    pub fn units_per_em(&self) -> f32 {
        self.units_per_em
    }

    pub fn ascent_em(&self) -> f32 {
        self.ascent
    }

    pub fn descent_em(&self) -> f32 {
        self.descent
    }

    pub fn max_width_em(&self) -> f32 {
        self.max_width
    }

    pub fn space_em(&self) -> f32 {
        self.space
    }

    pub fn has_color(&self) -> bool {
        self.color
    }

    pub fn postscript_name(&self) -> &str {
        &self.postscript_name
    }

    pub fn is_emoji(&self) -> bool {
        COLOR_FONT_POSTSCRIPT.contains(&self.postscript_name.as_str())
    }

    pub fn data(&self) -> &[u8] {
        self.bytes.bytes()
    }

    pub fn face_index(&self) -> u32 {
        self.bytes.index()
    }

    pub fn location(&self) -> &skrifa::instance::Location {
        &self.location
    }

    pub fn glyph_for(&self, character: char) -> u16 {
        let mut cache = self.glyphs.lock().expect("glyph cache lock poisoned");
        if let Some(found) = cache.get(&character) {
            return *found;
        }
        let glyph = skrifa::FontRef::from_index(self.bytes.bytes(), self.bytes.index())
            .ok()
            .and_then(|font| font.charmap().map(character))
            .map_or(0, |glyph| glyph.to_u32() as u16);
        if cache.len() > 8192 {
            cache.clear();
        }
        cache.insert(character, glyph);
        glyph
    }

    pub fn covers(&self, character: char) -> bool {
        default_ignorable(character) || self.glyph_for(character) != 0
    }

    pub(crate) fn with_shaper<R>(&self, run: impl FnOnce(&harfrust::Shaper<'_>) -> R) -> Option<R> {
        let font = harfrust::FontRef::from_index(self.bytes.bytes(), self.bytes.index()).ok()?;
        let shaper = self
            .shaper_data
            .shaper(&font)
            .instance(self.shaper_instance.as_ref())
            .build();
        Some(run(&shaper))
    }

    pub(crate) fn shape_plan(
        &self,
        shaper: &harfrust::Shaper<'_>,
        key: PlanKey,
        features: &[harfrust::Feature],
    ) -> Arc<harfrust::ShapePlan> {
        let mut plans = self
            .plans
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(plan) = plans.get(&key) {
            return Arc::clone(plan);
        }
        let plan = Arc::new(harfrust::ShapePlan::new(
            shaper,
            key.direction,
            Some(key.script),
            None,
            features,
        ));
        if plans.len() >= PLAN_CACHE_LIMIT {
            plans.clear();
        }
        plans.insert(key, Arc::clone(&plan));
        plan
    }

    pub fn with_font<R>(&self, run: impl FnOnce(&skrifa::FontRef<'_>) -> R) -> Option<R> {
        let font = skrifa::FontRef::from_index(self.bytes.bytes(), self.bytes.index()).ok()?;
        Some(run(&font))
    }
}

const PLAN_CACHE_LIMIT: usize = 64;
const CANDIDATE_CACHE_LIMIT: usize = 512;
const COVERAGE_CACHE_LIMIT: usize = 16_384;
const SCAN_CACHE_LIMIT: usize = 8_192;

pub(crate) fn insert_bounded<K: std::hash::Hash + Eq, V>(
    map: &mut HashMap<K, V>,
    limit: usize,
    key: K,
    value: V,
) {
    if map.len() >= limit && !map.contains_key(&key) {
        map.clear();
    }
    map.insert(key, value);
}

type CandidateKey = (Vec<FontFamily>, u16, FontStretch, FontSlant);

pub struct FontManager {
    database: FontDatabase,
    revision: u64,
    entries: HashMap<EntryKey, Option<Arc<FontEntry>>>,
    family_cache: HashMap<(FontFamily, u16, FontStretch, FontSlant), Option<ID>>,
    formation_cache: HashMap<(ID, String), bool>,
    scan_cache: HashMap<char, Option<ID>>,
    face_coverage: HashMap<(ID, char), bool>,
    candidate_cache: HashMap<CandidateKey, Vec<Arc<FontEntry>>>,
    shape_buffer: Option<harfrust::UnicodeBuffer>,
    fallback_index: Option<FallbackIndex>,
    shapes: ShapeCache,
    revision_mirror: Arc<AtomicU64>,
}

impl FontManager {
    pub fn new() -> Self {
        Self::with_database(FontDatabase::system())
    }

    pub fn new_without_system_fonts() -> Self {
        Self::with_database(FontDatabase::empty())
    }

    fn with_database(database: FontDatabase) -> Self {
        Self {
            database,
            revision: 0,
            entries: HashMap::new(),
            family_cache: HashMap::new(),
            formation_cache: HashMap::new(),
            scan_cache: HashMap::new(),
            face_coverage: HashMap::new(),
            candidate_cache: HashMap::new(),
            shape_buffer: None,
            fallback_index: None,
            shapes: ShapeCache::default(),
            revision_mirror: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn shapes_mut(&mut self) -> &mut ShapeCache {
        &mut self.shapes
    }

    pub fn shape_cache_stats(&self) -> ShapeCacheStats {
        self.shapes.stats()
    }

    pub fn revision_handle(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.revision_mirror)
    }

    fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.revision_mirror.store(self.revision, Ordering::Release);
        self.shapes.clear();
        self.entries.clear();
        self.family_cache.clear();
        self.formation_cache.clear();
        self.scan_cache.clear();
        self.face_coverage.clear();
        self.candidate_cache.clear();
    }

    pub fn font_revision(&self) -> u64 {
        self.revision
    }

    pub fn load_font_bytes(&mut self, data: Vec<u8>) {
        self.database.load_bytes(data);
        self.invalidate();
    }

    pub fn load_font_static(&mut self, data: &'static [u8]) {
        self.database.load_static(data);
        self.invalidate();
    }

    pub fn load_font_file(
        &mut self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<(), std::io::Error> {
        let loaded = self.database.load_file(path);
        if loaded.is_ok() {
            self.invalidate();
        }
        loaded
    }

    pub fn face_count(&self) -> usize {
        self.database.face_count()
    }

    pub fn has_family(&self, family: &str) -> bool {
        self.database.faces().any(|face| {
            face.families
                .iter()
                .any(|name| name.eq_ignore_ascii_case(family))
        })
    }

    pub fn family_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .database
            .faces()
            .flat_map(|face| face.families.iter().cloned())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn export_face_data(&mut self, face: ID) -> Option<Vec<u8>> {
        self.database
            .face_bytes(face)
            .map(|bytes| bytes.bytes().to_vec())
    }

    pub fn face_for_family(&self, family: &str) -> Option<ID> {
        self.query_name(family, 400, FontStretch::Normal, FontSlant::Normal)
    }

    pub fn first_face(&self) -> Option<ID> {
        self.database.faces().next().map(|face| face.id)
    }

    pub fn postscript_name(&self, id: ID) -> Option<&str> {
        self.database
            .face(id)
            .map(|face| face.post_script_name.as_str())
    }

    pub(crate) fn take_buffer(&mut self) -> harfrust::UnicodeBuffer {
        self.shape_buffer.take().unwrap_or_default()
    }

    pub(crate) fn return_buffer(&mut self, buffer: harfrust::UnicodeBuffer) {
        self.shape_buffer = Some(buffer);
    }

    fn query_name(
        &self,
        name: &str,
        weight: u16,
        stretch: FontStretch,
        slant: FontSlant,
    ) -> Option<ID> {
        let query = Query {
            family: name,
            weight: Weight(weight),
            stretch: query_stretch(stretch),
            style: query_style(slant),
        };
        let found = self.database.query(&query)?;
        let matches = self.database.face(found).is_some_and(|face| {
            face.families
                .iter()
                .any(|family| family.eq_ignore_ascii_case(name))
        });
        matches.then_some(found)
    }

    fn resolve_family(
        &mut self,
        family: &FontFamily,
        weight: u16,
        stretch: FontStretch,
        slant: FontSlant,
    ) -> Option<ID> {
        let key = (family.clone(), weight, stretch, slant);
        if let Some(found) = self.family_cache.get(&key) {
            return *found;
        }
        let names: Vec<&str> = match family {
            FontFamily::Named(name) => vec![name.as_str()],
            FontFamily::SansSerif => SANS_FAMILIES.to_vec(),
            FontFamily::Serif => SERIF_FAMILIES.to_vec(),
            FontFamily::Monospace => MONOSPACE_FAMILIES.to_vec(),
            FontFamily::Cursive => CURSIVE_FAMILIES.to_vec(),
            FontFamily::Fantasy => FANTASY_FAMILIES.to_vec(),
        };
        let found = names
            .into_iter()
            .find_map(|name| self.query_name(name, weight, stretch, slant));
        self.family_cache.insert(key, found);
        found
    }

    fn entry_job(
        &mut self,
        id: ID,
        weight: u16,
        stretch: FontStretch,
        slant: FontSlant,
    ) -> Option<PrewarmJob> {
        let face = self.database.face(id)?;
        let fake_italic = slant != FontSlant::Normal && face.style == Style::Normal;
        let key = EntryKey {
            id,
            weight,
            stretch,
            fake_italic,
        };
        if self.entries.contains_key(&key) {
            return None;
        }
        let (face_weight, postscript_name) = (face.weight.0, face.post_script_name.clone());
        let bytes = self.database.face_bytes(id)?;
        Some(PrewarmJob {
            key,
            revision: self.revision,
            face_weight,
            postscript_name,
            bytes,
        })
    }

    pub fn prewarm_jobs(&mut self, styles: &[TextStyle]) -> Vec<PrewarmJob> {
        let mut jobs: Vec<PrewarmJob> = Vec::new();
        for style in styles {
            let mut ids: Vec<ID> = self.primary_id(style).into_iter().collect();
            for name in Self::fallback_names(style) {
                ids.extend(self.query_name(&name, style.weight.0, style.stretch, style.slant));
            }
            for id in ids {
                let job = self.entry_job(id, style.weight.0, style.stretch, style.slant);
                let fresh = job.filter(|job| jobs.iter().all(|queued| queued.key != job.key));
                jobs.extend(fresh);
            }
        }
        jobs
    }

    pub fn install_prewarmed(&mut self, prewarmed: PrewarmedEntry) -> bool {
        let current =
            prewarmed.revision == self.revision && !self.entries.contains_key(&prewarmed.key);
        if current {
            self.entries.insert(prewarmed.key, Some(prewarmed.entry));
        }
        current
    }

    fn entry_for(
        &mut self,
        id: ID,
        weight: u16,
        stretch: FontStretch,
        slant: FontSlant,
    ) -> Option<Arc<FontEntry>> {
        let face_style = self.database.face(id)?.style;
        let fake_italic = slant != FontSlant::Normal && face_style == Style::Normal;
        let key = EntryKey {
            id,
            weight,
            stretch,
            fake_italic,
        };
        if let Some(found) = self.entries.get(&key) {
            return found.clone();
        }
        let postscript_name = self.database.face(id)?.post_script_name.clone();
        let face_weight = self.database.face(id)?.weight.0;
        let built = self.database.face_bytes(id).and_then(|bytes| {
            FontEntry::build(
                id,
                bytes,
                weight,
                face_weight,
                stretch,
                fake_italic,
                postscript_name,
            )
        });
        let built = built.map(Arc::new);
        self.entries.insert(key, built.clone());
        built
    }

    pub fn entry_with(
        &mut self,
        id: ID,
        weight: u16,
        stretch: FontStretch,
        synthetic_italic: bool,
    ) -> Option<Arc<FontEntry>> {
        let slant = if synthetic_italic {
            FontSlant::Italic
        } else {
            FontSlant::Normal
        };
        self.entry_for(id, weight, stretch, slant)
    }

    pub fn entry(&mut self, id: ID, style: &TextStyle) -> Option<Arc<FontEntry>> {
        self.entry_for(id, style.weight.0, style.stretch, style.slant)
    }

    pub fn primary(&mut self, style: &TextStyle) -> Option<Arc<FontEntry>> {
        let id = self.primary_id(style)?;
        self.entry(id, style)
    }

    fn primary_id(&mut self, style: &TextStyle) -> Option<ID> {
        let default = [FontFamily::SansSerif];
        let families = if style.families.is_empty() {
            &default[..]
        } else {
            &style.families[..]
        };
        families
            .iter()
            .find_map(|family| {
                self.resolve_family(family, style.weight.0, style.stretch, style.slant)
            })
            .or_else(|| {
                self.resolve_family(
                    &FontFamily::SansSerif,
                    style.weight.0,
                    style.stretch,
                    style.slant,
                )
            })
            .or_else(|| self.first_face())
    }

    fn fallback_candidates(&mut self, style: &TextStyle) -> Vec<Arc<FontEntry>> {
        let key = (
            style.families.clone(),
            style.weight.0,
            style.stretch,
            style.slant,
        );
        if let Some(found) = self.candidate_cache.get(&key) {
            return found.clone();
        }
        let candidates = self.build_fallback_candidates(style);
        insert_bounded(
            &mut self.candidate_cache,
            CANDIDATE_CACHE_LIMIT,
            key,
            candidates.clone(),
        );
        candidates
    }

    fn fallback_names(style: &TextStyle) -> Vec<String> {
        let mut names: Vec<String> = style
            .families
            .iter()
            .skip(1)
            .filter_map(|family| match family {
                FontFamily::Named(name) => Some(name.clone()),
                _ => None,
            })
            .collect();
        if style.families.len() <= 1 {
            names.extend(EMOJI_FAMILIES.iter().map(|name| (*name).to_owned()));
            names.extend(
                PLATFORM_FALLBACK_FAMILIES
                    .iter()
                    .map(|name| (*name).to_owned()),
            );
        }
        names
    }

    fn build_fallback_candidates(&mut self, style: &TextStyle) -> Vec<Arc<FontEntry>> {
        let names = Self::fallback_names(style);
        let mut candidates: Vec<Arc<FontEntry>> = Vec::new();
        for name in names {
            let Some(id) = self.query_name(&name, style.weight.0, style.stretch, style.slant)
            else {
                continue;
            };
            if candidates.iter().any(|entry| entry.id == id) {
                continue;
            }
            if let Some(entry) = self.entry(id, style) {
                candidates.push(entry);
            }
        }
        candidates
    }

    pub fn use_fallback_index(&mut self, persisted: Option<&[u8]>) {
        self.fallback_index = Some(
            persisted
                .and_then(FallbackIndex::decode)
                .unwrap_or_default(),
        );
        self.face_coverage.clear();
        self.scan_cache.clear();
    }

    pub fn fallback_index_snapshot(&mut self) -> Option<Vec<u8>> {
        self.fallback_index
            .as_mut()
            .and_then(FallbackIndex::take_snapshot)
    }

    pub fn fallback_index_len(&self) -> usize {
        self.fallback_index.as_ref().map_or(0, FallbackIndex::len)
    }

    fn indexed_cover(&mut self, id: ID, character: char) -> Option<bool> {
        self.fallback_index.as_ref()?;
        let fingerprint = self.database.fingerprint(id)?;
        if let Some(coverage) = self
            .fallback_index
            .as_ref()
            .and_then(|index| index.lookup(&fingerprint))
        {
            return Some(coverage.contains(character as u32));
        }
        let bytes = self.database.face_bytes(id)?;
        let font = skrifa::FontRef::from_index(bytes.bytes(), bytes.index()).ok()?;
        let coverage =
            Coverage::from_codepoints(font.charmap().mappings().map(|(codepoint, _)| codepoint));
        let covered = coverage.contains(character as u32);
        self.fallback_index.as_mut()?.insert(&fingerprint, coverage);
        Some(covered)
    }

    fn face_covers(&mut self, id: ID, character: char) -> bool {
        if let Some(found) = self.face_coverage.get(&(id, character)) {
            return *found;
        }
        let indexed = self.indexed_cover(id, character);
        let covered = indexed.unwrap_or_else(|| {
            self.database.face_bytes(id).is_some_and(|bytes| {
                skrifa::FontRef::from_index(bytes.bytes(), bytes.index())
                    .ok()
                    .and_then(|font| font.charmap().map(character))
                    .is_some()
            })
        });
        insert_bounded(
            &mut self.face_coverage,
            COVERAGE_CACHE_LIMIT,
            (id, character),
            covered,
        );
        covered
    }

    fn scan_for(&mut self, character: char, style: &TextStyle) -> Option<Arc<FontEntry>> {
        if let Some(found) = self.scan_cache.get(&character).copied() {
            return found.and_then(|id| self.entry(id, style));
        }
        let mut ranked: Vec<(ID, u16, bool, String)> = self
            .database
            .faces()
            .map(|face| {
                (
                    face.id,
                    face.weight.0.abs_diff(style.weight.0),
                    face.style == Style::Normal,
                    face.post_script_name.clone(),
                )
            })
            .collect();
        ranked.sort_by(|a, b| (!a.2, a.1, &a.3).cmp(&(!b.2, b.1, &b.3)));
        let found = ranked
            .into_iter()
            .map(|(id, ..)| id)
            .find(|id| self.face_covers(*id, character));
        insert_bounded(&mut self.scan_cache, SCAN_CACHE_LIMIT, character, found);
        found.and_then(|id| self.entry(id, style))
    }

    pub(crate) fn font_runs(
        &mut self,
        text: &str,
        style: &TextStyle,
    ) -> Vec<(std::ops::Range<usize>, Option<Arc<FontEntry>>, bool)> {
        let Some(primary) = self.primary(style) else {
            return vec![(0..text.len(), None, false)];
        };
        if text.is_ascii() {
            return vec![(0..text.len(), Some(primary), false)];
        }
        let candidates = self.fallback_candidates(style);
        let mut runs: Vec<(std::ops::Range<usize>, Option<Arc<FontEntry>>, bool)> = Vec::new();
        for (index, grapheme) in text.grapheme_indices(true) {
            let slot = if grapheme.is_ascii() || grapheme.chars().all(|ch| primary.covers(ch)) {
                None
            } else {
                select_cluster(
                    grapheme,
                    candidates.len(),
                    |ch| primary.covers(ch),
                    |slot, ch| candidates[slot].covers(ch),
                    |slot| self.forms_single(&candidates[slot], grapheme, style),
                )
            };
            let (entry, fallback) = match slot {
                Some(slot) => (candidates[slot].clone(), true),
                None => {
                    let first = grapheme.chars().find(|ch| !default_ignorable(*ch));
                    let scanned = first
                        .filter(|ch| !primary.covers(*ch))
                        .and_then(|ch| self.scan_for(ch, style))
                        .filter(|entry| grapheme.chars().all(|ch| entry.covers(ch)));
                    match scanned {
                        Some(entry) => (entry, true),
                        None => (primary.clone(), false),
                    }
                }
            };
            let end = index + grapheme.len();
            match runs.last_mut() {
                Some((range, existing, flag))
                    if existing.as_ref().is_some_and(|e| Arc::ptr_eq(e, &entry))
                        && *flag == fallback =>
                {
                    range.end = end;
                }
                _ => runs.push((index..end, Some(entry), fallback)),
            }
        }
        runs
    }

    fn forms_single(&mut self, entry: &Arc<FontEntry>, grapheme: &str, style: &TextStyle) -> bool {
        let key = (entry.id, grapheme.to_owned());
        if let Some(value) = self.formation_cache.get(&key) {
            return *value;
        }
        let glyphs = crate::shape::shape_run(
            self,
            entry,
            grapheme,
            &crate::shape::ShapeParams {
                size_px: style.size_px.max(1.0),
                rtl: false,
                features: &[],
                letter_spacing_px: 0.0,
            },
        );
        let emoji = entry.is_emoji();
        let mut count = 0usize;
        let mut valid = true;
        for glyph in &glyphs {
            if glyph.glyph_id == 0 {
                valid = false;
                break;
            }
            if emoji && glyph.glyph_id == 3 {
                continue;
            }
            count += 1;
        }
        let result = valid && count == 1;
        if self.formation_cache.len() < 512 {
            self.formation_cache.insert(key, result);
        }
        result
    }
}

fn query_stretch(stretch: FontStretch) -> Stretch {
    match stretch {
        FontStretch::Condensed => Stretch::CONDENSED,
        FontStretch::Normal => Stretch::NORMAL,
        FontStretch::Expanded => Stretch::EXPANDED,
    }
}

fn query_style(slant: FontSlant) -> Style {
    match slant {
        FontSlant::Normal => Style::Normal,
        FontSlant::Italic => Style::Italic,
        FontSlant::Oblique => Style::Oblique,
    }
}

impl Default for FontManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct SharedFonts {
    inner: Arc<Mutex<FontManager>>,
    revision: Arc<AtomicU64>,
}

impl Default for SharedFonts {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedFonts {
    pub fn new() -> Self {
        Self::wrap(FontManager::new())
    }

    pub fn without_system_fonts() -> Self {
        Self::wrap(FontManager::new_without_system_fonts())
    }

    pub fn wrap(manager: FontManager) -> Self {
        Self {
            revision: manager.revision_handle(),
            inner: Arc::new(Mutex::new(manager)),
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, FontManager> {
        self.inner.lock().expect("font manager lock poisoned")
    }
}

pub fn select_cluster(
    grapheme: &str,
    candidate_count: usize,
    primary_covers: impl Fn(char) -> bool,
    candidate_covers: impl Fn(usize, char) -> bool,
    mut forms_single: impl FnMut(usize) -> bool,
) -> Option<usize> {
    if grapheme.is_ascii() || grapheme.chars().all(&primary_covers) {
        return None;
    }
    let full: Vec<_> = (0..candidate_count)
        .filter(|index| grapheme.chars().all(|ch| candidate_covers(*index, ch)))
        .collect();
    if let Some(&first) = full.first() {
        if grapheme.chars().count() == 1 {
            return Some(first);
        }
        return full
            .into_iter()
            .find(|index| forms_single(*index))
            .or(Some(first));
    }
    let first = grapheme.chars().next()?;
    if primary_covers(first) {
        None
    } else {
        (0..candidate_count).find(|index| candidate_covers(*index, first))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_insert_never_exceeds_the_limit_and_keeps_the_newest() {
        let mut map = HashMap::new();
        (0..100u32).for_each(|n| insert_bounded(&mut map, 8, n, n));
        assert!(map.len() <= 8);
        assert_eq!(map.get(&99), Some(&99));
        insert_bounded(&mut map, 8, 99, 7);
        assert_eq!(map.get(&99), Some(&7));
    }

    #[test]
    fn empty_manager_has_no_faces() {
        let mgr = FontManager::new_without_system_fonts();
        assert_eq!(mgr.face_count(), 0);
    }

    #[test]
    fn shared_fonts_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SharedFonts>();
        assert_send_sync::<FontManager>();
        assert_send_sync::<FontEntry>();
    }

    #[test]
    fn font_loads_bump_revision() {
        let mut mgr = FontManager::new_without_system_fonts();
        assert_eq!(mgr.font_revision(), 0);
        let r0 = mgr.font_revision();
        mgr.load_font_file("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
            .expect("test host needs DejaVuSans");
        assert_eq!(mgr.font_revision(), r0.wrapping_add(1));
        assert_eq!(mgr.face_count(), 1);
        let data = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
            .expect("test host needs DejaVuSans");
        mgr.load_font_bytes(data);
        assert_eq!(mgr.font_revision(), r0.wrapping_add(2));
    }
}
