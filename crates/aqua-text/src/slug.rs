use std::collections::HashMap;
use std::sync::Arc;

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
struct SlugKey {
    font_id: FontId,
    weight: u16,
    stretch: FontStretch,
    synthetic_italic: bool,
    glyph_id: u16,
}

#[derive(Default)]
pub struct SlugCache {
    glyphs: HashMap<SlugKey, SlugLookup>,
    revision: u64,
}

impl SlugCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    pub fn heap_bytes(&self) -> usize {
        self.glyphs
            .values()
            .map(|lookup| match lookup {
                SlugLookup::Outline(glyph) => glyph.heap_bytes(),
                _ => 0,
            })
            .sum()
    }

    pub fn clear(&mut self) {
        self.glyphs.clear();
    }

    pub fn lookup(&mut self, fonts: &mut FontManager, key: &RasterKey) -> SlugLookup {
        if self.revision != fonts.font_revision() {
            self.glyphs.clear();
            self.revision = fonts.font_revision();
        }
        let slug_key = SlugKey {
            font_id: key.font_id,
            weight: key.weight,
            stretch: key.stretch,
            synthetic_italic: key.synthetic_italic,
            glyph_id: key.glyph_id,
        };
        if let Some(found) = self.glyphs.get(&slug_key) {
            return found.clone();
        }
        let built = build(fonts, key);
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

#[derive(Default)]
pub struct SlugStore {
    entries: HashMap<SlugKey, StoreLookup>,
    texels: Vec<[f32; 4]>,
    rows: u32,
    synced: usize,
    created: bool,
    revision: u64,
}

impl SlugStore {
    pub fn new() -> Self {
        Self::default()
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

    pub fn resident_bytes(&self) -> u64 {
        self.rows as u64 * ROW_TEXELS as u64 * 16
    }

    pub fn reset(&mut self) {
        self.entries.clear();
        self.texels.clear();
        self.synced = 0;
        self.created = false;
        self.revision = 0;
    }

    pub fn lookup(
        &mut self,
        cache: &mut SlugCache,
        fonts: &mut FontManager,
        key: &RasterKey,
    ) -> StoreLookup {
        if self.revision != fonts.font_revision() {
            self.reset();
            self.revision = fonts.font_revision();
        }
        let slug_key = SlugKey {
            font_id: key.font_id,
            weight: key.weight,
            stretch: key.stretch,
            synthetic_italic: key.synthetic_italic,
            glyph_id: key.glyph_id,
        };
        if let Some(found) = self.entries.get(&slug_key) {
            return *found;
        }
        let stored = match cache.lookup(fonts, key) {
            SlugLookup::Outline(glyph) => {
                let base = self.texels.len() as u32;
                self.texels.extend(pack_glyph(&glyph, base));
                StoreLookup::Stored(StoredGlyph {
                    index: base,
                    em_bounds: glyph.bounds,
                })
            }
            SlugLookup::Blank => StoreLookup::Blank,
            SlugLookup::Unsupported => StoreLookup::Unsupported,
        };
        self.entries.insert(slug_key, stored);
        stored
    }

    pub fn uploads(&mut self) -> UploadBatch {
        if self.texels.is_empty() && !self.created {
            return UploadBatch::default();
        }
        let needed_rows = self.texels.len().div_ceil(ROW_TEXELS) as u32;
        let recreate = !self.created || needed_rows > self.rows || self.synced > self.texels.len();
        let mut textures = Vec::new();
        let first_row = match recreate {
            true => {
                if self.created {
                    textures.push(TextureUpload {
                        handle: STORE_HANDLE,
                        op: TextureOp::Release,
                    });
                }
                self.rows = needed_rows.max(MIN_STORE_ROWS).next_power_of_two();
                self.created = true;
                textures.push(TextureUpload {
                    handle: STORE_HANDLE,
                    op: TextureOp::Create {
                        extent: Extent::new(STORE_WIDTH, self.rows),
                        format: TextureFormat::Rgba32Float,
                    },
                });
                0
            }
            false => (self.synced / ROW_TEXELS) as u32,
        };
        let from = first_row as usize * ROW_TEXELS;
        if self.texels.len() > from && (recreate || self.texels.len() > self.synced) {
            let rows = needed_rows - first_row;
            let mut data = Vec::with_capacity(rows as usize * ROW_TEXELS * 16);
            for texel in &self.texels[from..] {
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
        self.synced = self.texels.len();
        UploadBatch { textures }
    }
}
