use std::collections::HashMap;

use etagere::{AllocId, AtlasAllocator, size2};

use crate::font::FontManager;
use crate::raster::{Bitmap, GlyphImage, RasterKey, Rasterizer};

const GUTTER: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AtlasKind {
    Mask,
    Color,
}

impl AtlasKind {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            AtlasKind::Mask => 1,
            AtlasKind::Color => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasSlot {
    pub kind: AtlasKind,
    pub page: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtlasUpload {
    pub kind: AtlasKind,
    pub page: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AtlasStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub resident_glyphs: usize,
    pub pages: usize,
}

struct Page {
    kind: AtlasKind,
    allocator: AtlasAllocator,
}

enum Resident {
    Empty,
    Slot {
        slot: AtlasSlot,
        allocation: AllocId,
    },
}

struct Entry {
    resident: Resident,
    last_used: u64,
}

pub struct GlyphAtlas {
    page_size: u32,
    max_pages: usize,
    pages: Vec<Page>,
    entries: HashMap<RasterKey, Entry>,
    frame: u64,
    uploads: Vec<AtlasUpload>,
    stats: AtlasStats,
}

impl GlyphAtlas {
    pub fn new(page_size: u32, max_pages: usize) -> Self {
        Self {
            page_size: page_size.max(64),
            max_pages: max_pages.max(2),
            pages: Vec::new(),
            entries: HashMap::new(),
            frame: 0,
            uploads: Vec::new(),
            stats: AtlasStats::default(),
        }
    }

    pub fn page_size(&self) -> u32 {
        self.page_size
    }

    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    pub fn stats(&self) -> AtlasStats {
        AtlasStats {
            resident_glyphs: self.entries.len(),
            pages: self.pages.len(),
            ..self.stats
        }
    }

    pub fn take_uploads(&mut self) -> Vec<AtlasUpload> {
        std::mem::take(&mut self.uploads)
    }

    pub fn slot(
        &mut self,
        fonts: &mut FontManager,
        rasterizer: &mut Rasterizer,
        key: &RasterKey,
    ) -> Option<AtlasSlot> {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_used = self.frame;
            self.stats.hits += 1;
            return match entry.resident {
                Resident::Empty => None,
                Resident::Slot { slot, .. } => Some(slot),
            };
        }
        self.stats.misses += 1;
        let image = rasterizer
            .rasterize(fonts, key)
            .filter(|image| !image.bitmap().is_empty());
        let Some(image) = image else {
            self.entries.insert(
                *key,
                Entry {
                    resident: Resident::Empty,
                    last_used: self.frame,
                },
            );
            return None;
        };
        let kind = match image {
            GlyphImage::Mask(_) => AtlasKind::Mask,
            GlyphImage::Color(_) => AtlasKind::Color,
        };
        let bitmap = match image {
            GlyphImage::Mask(bitmap) | GlyphImage::Color(bitmap) => bitmap,
        };
        let (page, allocation, x, y) = self.allocate(kind, &bitmap)?;
        let slot = AtlasSlot {
            kind,
            page: page as u32,
            x,
            y,
            width: bitmap.width,
            height: bitmap.height,
            left: bitmap.left,
            top: bitmap.top,
        };
        self.uploads.push(AtlasUpload {
            kind,
            page: slot.page,
            x,
            y,
            width: bitmap.width,
            height: bitmap.height,
            data: bitmap.data,
        });
        self.entries.insert(
            *key,
            Entry {
                resident: Resident::Slot { slot, allocation },
                last_used: self.frame,
            },
        );
        Some(slot)
    }

    fn try_allocate(
        &mut self,
        kind: AtlasKind,
        bitmap: &Bitmap,
    ) -> Option<(usize, AllocId, u32, u32)> {
        let width = (bitmap.width + 2 * GUTTER) as i32;
        let height = (bitmap.height + 2 * GUTTER) as i32;
        for (index, page) in self.pages.iter_mut().enumerate() {
            if page.kind != kind {
                continue;
            }
            if let Some(allocation) = page.allocator.allocate(size2(width, height)) {
                return Some((
                    index,
                    allocation.id,
                    allocation.rectangle.min.x as u32 + GUTTER,
                    allocation.rectangle.min.y as u32 + GUTTER,
                ));
            }
        }
        None
    }

    fn allocate(&mut self, kind: AtlasKind, bitmap: &Bitmap) -> Option<(usize, AllocId, u32, u32)> {
        if bitmap.width + 2 * GUTTER > self.page_size || bitmap.height + 2 * GUTTER > self.page_size
        {
            return None;
        }
        if let Some(found) = self.try_allocate(kind, bitmap) {
            return Some(found);
        }
        if self.pages.len() < self.max_pages {
            self.pages.push(Page {
                kind,
                allocator: AtlasAllocator::new(size2(self.page_size as i32, self.page_size as i32)),
            });
            return self.try_allocate(kind, bitmap);
        }
        let mut stale: Vec<(RasterKey, u64)> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.last_used < self.frame)
            .filter(|(_, entry)| {
                matches!(&entry.resident, Resident::Slot { slot, .. } if slot.kind == kind)
            })
            .map(|(key, entry)| (*key, entry.last_used))
            .collect();
        stale.sort_by_key(|(_, last_used)| *last_used);
        for (key, _) in stale {
            if let Some(Entry {
                resident: Resident::Slot { slot, allocation },
                ..
            }) = self.entries.remove(&key)
            {
                self.pages[slot.page as usize]
                    .allocator
                    .deallocate(allocation);
                self.stats.evictions += 1;
            }
            if let Some(found) = self.try_allocate(kind, bitmap) {
                return Some(found);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::place_glyph;
    use crate::{FontManager, TextLayout, TextStyle};

    fn keys(text: &str, size: f32) -> (FontManager, Vec<RasterKey>) {
        let mut fonts = FontManager::new();
        let mut layout = TextLayout::new();
        layout.set_text(
            text,
            &TextStyle {
                size_px: size,
                ..TextStyle::default()
            },
        );
        let laid = layout.layout(&mut fonts);
        let keys = laid.lines[0]
            .glyphs
            .iter()
            .map(|glyph| place_glyph(glyph, 0.0, 20.0, true).key)
            .collect();
        (fonts, keys)
    }

    #[test]
    fn repeated_glyphs_hit_and_upload_once() {
        let (mut fonts, keys) = keys("lll", 18.0);
        let mut atlas = GlyphAtlas::new(256, 2);
        let mut rasterizer = Rasterizer::new();
        atlas.begin_frame();
        let slots: Vec<_> = (0..3)
            .map(|_| atlas.slot(&mut fonts, &mut rasterizer, &keys[0]).unwrap())
            .collect();
        assert_eq!(slots[0], slots[1]);
        assert_eq!(slots[1], slots[2]);
        assert_eq!(atlas.take_uploads().len(), 1);
        assert_eq!(atlas.stats().hits, 2);
        assert_eq!(atlas.stats().misses, 1);
    }

    #[test]
    fn space_is_cached_as_empty() {
        let (mut fonts, keys) = keys("a a", 18.0);
        let mut atlas = GlyphAtlas::new(256, 2);
        let mut rasterizer = Rasterizer::new();
        atlas.begin_frame();
        assert!(atlas.slot(&mut fonts, &mut rasterizer, &keys[1]).is_none());
        assert!(atlas.slot(&mut fonts, &mut rasterizer, &keys[1]).is_none());
        assert_eq!(atlas.stats().misses, 1);
    }

    #[test]
    fn full_atlas_evicts_only_glyphs_unused_this_frame() {
        let (mut fonts, keys) = keys("abcdefghijklmnopqrstuvwxyz", 40.0);
        let mut atlas = GlyphAtlas::new(64, 2);
        let mut rasterizer = Rasterizer::new();
        atlas.begin_frame();
        let first: Vec<_> = keys
            .iter()
            .map(|key| atlas.slot(&mut fonts, &mut rasterizer, key))
            .collect();
        assert!(first.iter().any(Option::is_none), "tiny atlas must run out");
        atlas.begin_frame();
        let again = atlas.slot(&mut fonts, &mut rasterizer, &keys[25]);
        assert!(again.is_some());
        assert!(atlas.stats().evictions > 0);
    }
}
