use std::collections::HashMap;

use cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, SwashCache, SwashContent, SwashImage};
use glyphon::{ContentType, RasterizedCustomGlyph};

use aqua_core::scene::TextRasterMode;

#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod windows;

pub(crate) struct CachedGlyph {
    pub id: u16,
    pub image: Option<SwashImage>,
}

pub(crate) struct RasterCache {
    entries: HashMap<CacheKey, CachedGlyph>,
    bytes: usize,
    ids: Vec<CacheKey>,
    pub mode: TextRasterMode,
    #[cfg(target_os = "windows")]
    native: Option<windows::NativeRasterizer>,
}

impl Default for RasterCache {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            ids: Vec::new(),
            bytes: 0,
            mode: TextRasterMode::PlatformDefault,
            #[cfg(target_os = "windows")]
            native: None,
        }
    }
}

impl RasterCache {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.ids.clear();
        self.bytes = 0;
    }

    pub fn near_capacity(&self) -> bool {
        self.entries.len() > 60_000 || self.bytes > 48 * 1024 * 1024
    }

    pub fn get(
        &mut self,
        fonts: &mut FontSystem,
        swash: &mut SwashCache,
        mut key: CacheKey,
    ) -> Result<&CachedGlyph, String> {
        if self.mode == TextRasterMode::Outline {
            key.flags.insert(CacheKeyFlags::DISABLE_HINTING);
        }
        if !self.entries.contains_key(&key) {
            if self.entries.len() >= u16::MAX as usize {
                return Err("glyph cache count budget exceeded".into());
            }
            let image = swash.get_image_uncached(fonts, key);
            #[cfg(target_os = "windows")]
            let image = match (&image, self.mode) {
                (Some(image), TextRasterMode::PlatformDefault)
                    if image.content != SwashContent::Color =>
                {
                    if self.native.is_none() {
                        self.native = Some(windows::NativeRasterizer::new()?);
                    }
                    self.native.as_mut().unwrap().rasterize(fonts, key)?
                }
                _ => image,
            };
            let bytes = image.as_ref().map_or(0, |image| image.data.len());
            if self.bytes.saturating_add(bytes) > 64 * 1024 * 1024 {
                return Err("glyph cache byte budget exceeded".into());
            }
            let id = self.entries.len() as u16;
            self.ids.push(key);
            self.entries.insert(key, CachedGlyph { id, image });
            self.bytes += bytes;
        }
        Ok(self.entries.get(&key).unwrap())
    }

    pub fn rasterized(&self, id: u16) -> Option<RasterizedCustomGlyph> {
        let image = self
            .entries
            .get(self.ids.get(id as usize)?)?
            .image
            .as_ref()?;
        Some(RasterizedCustomGlyph {
            data: image.data.clone(),
            content_type: match image.content {
                SwashContent::Color => ContentType::Color,
                _ => ContentType::Mask,
            },
        })
    }
}
