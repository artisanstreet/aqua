use std::collections::HashMap;

use aqua_text::raster::{RasterKey, Rasterizer};
use aqua_text::FontManager;
use glyphon::{ContentType, RasterizedCustomGlyph};

use aqua_core::scene::TextRasterMode;

use super::image::{Content, Image};

#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod windows;

pub(crate) struct CachedGlyph {
    pub id: u16,
    pub image: Option<Image>,
}

pub(crate) struct RasterCache {
    entries: HashMap<RasterKey, CachedGlyph>,
    bytes: usize,
    ids: Vec<RasterKey>,
    rasterizer: Rasterizer,
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
            rasterizer: Rasterizer::new(),
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

    pub fn get(&mut self, fonts: &mut FontManager, key: RasterKey) -> Result<&CachedGlyph, String> {
        if !self.entries.contains_key(&key) {
            if self.entries.len() >= u16::MAX as usize {
                return Err("glyph cache count budget exceeded".into());
            }
            let image = self.rasterizer.rasterize(fonts, &key).map(Image::from);
            #[cfg(target_os = "windows")]
            let image = match (&image, self.mode) {
                (Some(image), TextRasterMode::PlatformDefault)
                    if image.content != Content::Color =>
                {
                    if self.native.is_none() {
                        self.native = Some(windows::NativeRasterizer::new()?);
                    }
                    self.native.as_mut().unwrap().rasterize(fonts, &key)?
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
                Content::Color => ContentType::Color,
                Content::Mask => ContentType::Mask,
            },
        })
    }
}
