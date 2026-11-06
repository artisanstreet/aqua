use std::sync::Arc;

use aqua_media::hash::FastMap;

use aqua_graph::slug::STORE_WIDTH;
pub use aqua_graph::slug::{pack_glyph, PathCommand, SlugCurve, SlugGlyph};
use aqua_graph::{Extent, TextureFormat, TextureHandle};
use aqua_protocol::{TextureOp, TextureRegion, TextureUpload, UploadBatch};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{GlyphId, MetadataProvider};

use crate::font::{FontId, FontManager};
use crate::raster::RasterKey;
use crate::style::FontStretch;

const ITALIC_SKEW: f32 = 0.2126;

struct EmPen {
    scale: f32,
    skew: f32,
    commands: Vec<PathCommand>,
}

impl EmPen {
    fn map(&self, x: f32, y: f32) -> [f32; 2] {
        [(x + self.skew * y) * self.scale, y * self.scale]
    }
}

impl OutlinePen for EmPen {
    fn move_to(&mut self, x: f32, y: f32) {
        let point = self.map(x, y);
        self.commands.push(PathCommand::Move(point));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let point = self.map(x, y);
        self.commands.push(PathCommand::Line(point));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (control, point) = (self.map(cx0, cy0), self.map(x, y));
        self.commands.push(PathCommand::Quad(control, point));
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0, c1, point) = (self.map(cx0, cy0), self.map(cx1, cy1), self.map(x, y));
        self.commands.push(PathCommand::Cubic(c0, c1, point));
    }

    fn close(&mut self) {
        self.commands.push(PathCommand::Close);
    }
}

#[derive(Clone, Debug)]
pub enum SlugLookup {
    Outline(Arc<SlugGlyph>),
    Blank,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlugKey {
    pub font_id: FontId,
    pub weight: u16,
    pub stretch: FontStretch,
    pub synthetic_italic: bool,
    pub glyph_id: u16,
}

impl SlugKey {
    pub fn of(key: &RasterKey) -> Self {
        Self {
            font_id: key.font_id,
            weight: key.weight,
            stretch: key.stretch,
            synthetic_italic: key.synthetic_italic,
            glyph_id: key.glyph_id,
        }
    }

    fn face(self) -> Self {
        Self {
            glyph_id: 0,
            ..self
        }
    }
}

pub const SLUG_CACHE_BUDGET_BYTES: usize = 4 * 1024 * 1024;

fn lookup_bytes(lookup: &SlugLookup) -> usize {
    match lookup {
        SlugLookup::Outline(glyph) => glyph.heap_bytes() + std::mem::size_of::<SlugGlyph>(),
        _ => 0,
    }
}

pub struct SlugCache {
    glyphs: FastMap<SlugKey, SlugLookup>,
    revision: u64,
    bytes: usize,
    budget: usize,
}

impl Default for SlugCache {
    fn default() -> Self {
        Self::with_budget(SLUG_CACHE_BUDGET_BYTES)
    }
}

impl SlugCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_budget(budget: usize) -> Self {
        Self {
            glyphs: FastMap::default(),
            revision: 0,
            bytes: 0,
            budget,
        }
    }

    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    pub fn heap_bytes(&self) -> usize {
        self.bytes
    }

    pub fn clear(&mut self) {
        self.glyphs = FastMap::default();
        self.bytes = 0;
    }

    pub fn lookup(&mut self, fonts: &mut FontManager, key: &RasterKey) -> SlugLookup {
        if self.revision != fonts.font_revision() {
            self.clear();
            self.revision = fonts.font_revision();
        }
        let slug_key = SlugKey::of(key);
        if let Some(found) = self.glyphs.get(&slug_key) {
            return found.clone();
        }
        let built = build(fonts, key);
        let added = lookup_bytes(&built);
        if self.bytes + added > self.budget {
            self.clear();
        }
        self.bytes += added;
        self.glyphs.insert(slug_key, built.clone());
        built
    }
}

pub fn em_outline(fonts: &mut FontManager, key: &RasterKey) -> Option<Vec<PathCommand>> {
    let entry = fonts.entry_with(key.font_id, key.weight, key.stretch, key.synthetic_italic)?;
    let scale = 1.0 / entry.units_per_em();
    entry
        .with_font(|font| {
            let outline = font
                .outline_glyphs()
                .get(GlyphId::new(u32::from(key.glyph_id)))?;
            let mut pen = EmPen {
                scale,
                skew: if key.synthetic_italic {
                    ITALIC_SKEW
                } else {
                    0.0
                },
                commands: Vec::new(),
            };
            outline
                .draw(
                    DrawSettings::unhinted(Size::unscaled(), LocationRef::from(entry.location())),
                    &mut pen,
                )
                .ok()?;
            Some(pen.commands)
        })
        .flatten()
}

fn build(fonts: &mut FontManager, key: &RasterKey) -> SlugLookup {
    let Some(entry) = fonts.entry_with(key.font_id, key.weight, key.stretch, key.synthetic_italic)
    else {
        return SlugLookup::Unsupported;
    };
    if entry.synthetic_bold() || entry.has_color() {
        return SlugLookup::Unsupported;
    }
    let drawn = em_outline(fonts, key);
    match drawn {
        None => SlugLookup::Unsupported,
        Some(commands) => match SlugGlyph::from_path(&commands) {
            Some(glyph) => SlugLookup::Outline(Arc::new(glyph)),
            None => SlugLookup::Blank,
        },
    }
}

pub const STORE_HANDLE: TextureHandle = TextureHandle(u32::MAX - 1);

const MIN_STORE_ROWS: u32 = 8;
const ROW_TEXELS: usize = STORE_WIDTH as usize;
const GROWTH_STEP_TEXELS: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StoredGlyph {
    pub index: u32,
    pub em_bounds: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StoreLookup {
    Stored(StoredGlyph),
    Blank,
    Unsupported,
}

pub const STORE_BUDGET_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
struct StoreEntry {
    lookup: StoreLookup,
    last_used: u64,
    block: Option<(u32, u32)>,
}

pub struct SlugStore<K = SlugKey> {
    entries: FastMap<K, StoreEntry>,
    warmed: Vec<SlugKey>,
    texels: Vec<[f32; 4]>,
    free: Vec<(u32, u32)>,
    rows: u32,
    dirty: Option<(usize, usize)>,
    created: bool,
    revision: u64,
    epoch: u64,
    budget_texels: usize,
    evictions: u64,
    overruns: u64,
}

impl<K: Copy + Eq + std::hash::Hash> Default for SlugStore<K> {
    fn default() -> Self {
        Self::with_budget(STORE_BUDGET_BYTES)
    }
}

impl<K: Copy + Eq + std::hash::Hash> SlugStore<K> {
    pub fn with_budget(bytes: u64) -> Self {
        Self {
            entries: FastMap::default(),
            warmed: Vec::new(),
            texels: Vec::new(),
            free: Vec::new(),
            rows: 0,
            dirty: None,
            created: false,
            revision: 0,
            epoch: 0,
            budget_texels: (bytes / 16) as usize,
            evictions: 0,
            overruns: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn texel_count(&self) -> usize {
        self.texels.len()
    }

    pub fn live_texels(&self) -> usize {
        let freed: usize = self.free.iter().map(|(_, len)| *len as usize).sum();
        self.texels.len() - freed
    }

    pub fn resident_bytes(&self) -> u64 {
        self.rows as u64 * ROW_TEXELS as u64 * 16
    }

    pub fn evictions(&self) -> u64 {
        self.evictions
    }

    pub fn overruns(&self) -> u64 {
        self.overruns
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.warmed.clear();
        self.texels = Vec::new();
        self.free = Vec::new();
        self.dirty = None;
        self.revision = 0;
    }

    pub fn reset(&mut self) {
        self.entries.clear();
        self.warmed.clear();
        self.texels.clear();
        self.free.clear();
        self.dirty = None;
        self.created = false;
        self.revision = 0;
    }

    fn release(&mut self, start: u32, len: u32) {
        let at = self.free.partition_point(|(from, _)| *from < start);
        self.free.insert(at, (start, len));
        let merged = self.free.iter().fold(
            Vec::<(u32, u32)>::with_capacity(self.free.len()),
            |mut acc, &(from, size)| {
                match acc.last_mut() {
                    Some(last) if last.0 + last.1 == from => last.1 += size,
                    _ => acc.push((from, size)),
                }
                acc
            },
        );
        self.free = merged;
        if let Some(&(from, size)) = self.free.last() {
            if (from + size) as usize == self.texels.len() {
                self.free.pop();
                self.texels.truncate(from as usize);
                self.dirty = self
                    .dirty
                    .map(|(low, high)| (low.min(from as usize), high.min(from as usize)))
                    .filter(|(low, high)| low < high);
            }
        }
    }

    fn take_free(&mut self, len: u32) -> Option<u32> {
        let at = self.free.iter().position(|(_, size)| *size >= len)?;
        let (start, size) = self.free[at];
        match size == len {
            true => {
                self.free.remove(at);
            }
            false => self.free[at] = (start + len, size - len),
        }
        Some(start)
    }

    fn allocate(&mut self, len: u32) -> u32 {
        if let Some(start) = self.take_free(len) {
            return start;
        }
        if self.texels.len() + len as usize <= self.budget_texels {
            return self.texels.len() as u32;
        }
        let mut candidates: Vec<(u64, u32, K)> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.block.is_some() && entry.last_used < self.epoch)
            .map(|(key, entry)| (entry.last_used, entry.block.map_or(0, |b| b.0), *key))
            .collect();
        candidates.sort_unstable_by_key(|(used, start, _)| (*used, *start));
        for (_, _, key) in candidates {
            if let Some(entry) = self.entries.remove(&key) {
                if let Some((start, size)) = entry.block {
                    self.evictions += 1;
                    self.release(start, size);
                }
            }
            if let Some(start) = self.take_free(len) {
                return start;
            }
            if self.texels.len() + len as usize <= self.budget_texels {
                return self.texels.len() as u32;
            }
        }
        self.overruns += 1;
        self.texels.len() as u32
    }

    fn mark_dirty(&mut self, from: usize, to: usize) {
        self.dirty = Some(match self.dirty {
            Some((low, high)) => (low.min(from), high.max(to)),
            None => (from, to),
        });
    }

    pub fn insert(&mut self, key: K, glyph: &SlugGlyph) -> StoreLookup {
        let len = pack_glyph(glyph, 0).len() as u32;
        let base = self.allocate(len);
        let packed = pack_glyph(glyph, base);
        let end = base as usize + packed.len();
        if self.texels.len() < end {
            if self.texels.capacity() < end {
                let step = end.next_multiple_of(GROWTH_STEP_TEXELS) - self.texels.len();
                self.texels.reserve_exact(step);
            }
            self.texels.resize(end, [0.0; 4]);
        }
        self.texels[base as usize..end].copy_from_slice(&packed);
        self.mark_dirty(base as usize, end);
        let entry = StoreEntry {
            lookup: StoreLookup::Stored(StoredGlyph {
                index: base,
                em_bounds: glyph.bounds,
            }),
            last_used: self.epoch,
            block: Some((base, len)),
        };
        self.entries.insert(key, entry);
        entry.lookup
    }

    pub fn insert_empty(&mut self, key: K, lookup: StoreLookup) -> StoreLookup {
        self.entries.insert(
            key,
            StoreEntry {
                lookup,
                last_used: self.epoch,
                block: None,
            },
        );
        lookup
    }

    pub fn touch(&mut self, key: &K) -> Option<StoreLookup> {
        let epoch = self.epoch;
        self.entries.get_mut(key).map(|entry| {
            entry.last_used = epoch;
            entry.lookup
        })
    }

    pub fn uploads(&mut self) -> UploadBatch {
        self.epoch += 1;
        if self.texels.is_empty() && !self.created {
            return UploadBatch::default();
        }
        let needed_rows = self.texels.len().div_ceil(ROW_TEXELS) as u32;
        let wanted_rows = needed_rows.max(MIN_STORE_ROWS).next_power_of_two();
        let shrink = self.created && !self.texels.is_empty() && wanted_rows * 4 <= self.rows;
        let recreate = !self.created || needed_rows > self.rows || shrink;
        let mut textures = Vec::new();
        let rows_range = match recreate {
            true => {
                if self.created {
                    textures.push(TextureUpload {
                        handle: STORE_HANDLE,
                        op: TextureOp::Release,
                    });
                }
                self.rows = wanted_rows;
                self.created = true;
                textures.push(TextureUpload {
                    handle: STORE_HANDLE,
                    op: TextureOp::Create {
                        extent: Extent::new(STORE_WIDTH, self.rows),
                        format: TextureFormat::Rgba32Float,
                    },
                });
                (needed_rows > 0).then_some((0, needed_rows))
            }
            false => self
                .dirty
                .map(|(low, high)| ((low / ROW_TEXELS) as u32, high.div_ceil(ROW_TEXELS) as u32)),
        };
        if let Some((first_row, last_row)) = rows_range {
            let rows = last_row - first_row;
            let from = first_row as usize * ROW_TEXELS;
            let to = (last_row as usize * ROW_TEXELS).min(self.texels.len());
            let mut data = Vec::with_capacity(rows as usize * ROW_TEXELS * 16);
            for texel in &self.texels[from..to] {
                texel
                    .iter()
                    .for_each(|value| data.extend_from_slice(&value.to_le_bytes()));
            }
            data.resize(rows as usize * ROW_TEXELS * 16, 0);
            textures.push(TextureUpload {
                handle: STORE_HANDLE,
                op: TextureOp::Write {
                    region: TextureRegion {
                        x: 0,
                        y: first_row,
                        extent: Extent::new(STORE_WIDTH, rows),
                    },
                    data,
                },
            });
        }
        self.dirty = None;
        UploadBatch { textures }
    }
}

const WARM_FIRST: char = ' ';
const WARM_LAST: char = '~';

impl SlugStore<SlugKey> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resident(&mut self, key: &RasterKey, font_revision: u64) -> Option<StoreLookup> {
        match self.revision == font_revision {
            true => self.touch(&SlugKey::of(key)),
            false => None,
        }
    }

    pub fn lookup(
        &mut self,
        cache: &mut SlugCache,
        fonts: &mut FontManager,
        key: &RasterKey,
    ) -> StoreLookup {
        if self.revision != fonts.font_revision() {
            self.clear();
            self.revision = fonts.font_revision();
        }
        let slug_key = SlugKey::of(key);
        if let Some(found) = self.touch(&slug_key) {
            return found;
        }
        self.warm_face(cache, fonts, key);
        match self.touch(&slug_key) {
            Some(found) => found,
            None => self.fetch(cache, fonts, key, slug_key),
        }
    }

    fn fetch(
        &mut self,
        cache: &mut SlugCache,
        fonts: &mut FontManager,
        key: &RasterKey,
        slug_key: SlugKey,
    ) -> StoreLookup {
        match cache.lookup(fonts, key) {
            SlugLookup::Outline(glyph) => self.insert(slug_key, &glyph),
            SlugLookup::Blank => self.insert_empty(slug_key, StoreLookup::Blank),
            SlugLookup::Unsupported => self.insert_empty(slug_key, StoreLookup::Unsupported),
        }
    }

    pub fn warm_face(&mut self, cache: &mut SlugCache, fonts: &mut FontManager, key: &RasterKey) {
        let face = SlugKey::of(key).face();
        if self.warmed.contains(&face) {
            return;
        }
        self.warmed.push(face);
        let Some(entry) =
            fonts.entry_with(key.font_id, key.weight, key.stretch, key.synthetic_italic)
        else {
            return;
        };
        for character in WARM_FIRST..=WARM_LAST {
            let glyph_id = entry.glyph_for(character);
            let slug_key = SlugKey { glyph_id, ..face };
            if glyph_id == 0 || self.entries.contains_key(&slug_key) {
                continue;
            }
            let warm = RasterKey { glyph_id, ..*key };
            self.fetch(cache, fonts, &warm, slug_key);
        }
    }
}
