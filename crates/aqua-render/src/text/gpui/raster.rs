use aqua_text::FontManager;
use aqua_text::raster::{RasterKey, Rasterizer as GlyphRasterizer};
use aqua_text::style::FontStretch;

use crate::text::image::{Content, Image};

pub(super) struct GlyphRequest {
    pub font_id: aqua_text::FontId,
    pub weight: u16,
    pub stretch: FontStretch,
    pub synthetic_italic: bool,
    pub glyph_id: u16,
    pub font_size: f32,
    pub scale: f32,
    pub x_variant: u8,
    pub is_emoji: bool,
    pub subpixel: bool,
    pub hint: bool,
}

pub(super) struct Rasterizer {
    inner: GlyphRasterizer,
}

impl Rasterizer {
    pub fn new() -> Self {
        Self {
            inner: GlyphRasterizer::new(),
        }
    }

    pub fn is_emoji_font(
        fonts: &mut FontManager,
        font_id: aqua_text::FontId,
        weight: u16,
        stretch: FontStretch,
    ) -> Result<bool, String> {
        let entry = fonts
            .entry_with(font_id, weight, stretch, false)
            .ok_or("glyph font face is unavailable")?;
        Ok(entry.is_emoji())
    }

    pub fn is_color_glyph(
        &mut self,
        fonts: &mut FontManager,
        font_id: aqua_text::FontId,
        weight: u16,
        stretch: FontStretch,
        glyph_id: u16,
    ) -> Result<bool, String> {
        if !Self::is_emoji_font(fonts, font_id, weight, stretch)? {
            return Ok(false);
        }
        let key = RasterKey {
            font_id,
            weight,
            stretch,
            synthetic_italic: false,
            glyph_id,
            size_bits: 32.0f32.to_bits(),
            subpixel: 0,
        };
        Ok(matches!(
            self.inner.rasterize(fonts, &key),
            Some(aqua_text::raster::GlyphImage::Color(_))
        ))
    }

    pub fn rasterize(
        &mut self,
        fonts: &mut FontManager,
        request: &GlyphRequest,
    ) -> Result<Option<Image>, String> {
        if !request.font_size.is_finite()
            || request.font_size <= 0.0
            || !request.scale.is_finite()
            || request.scale <= 0.0
            || request.x_variant >= 4
        {
            return Err("invalid GPUI glyph request".into());
        }
        let key = RasterKey {
            font_id: request.font_id,
            weight: request.weight,
            stretch: request.stretch,
            synthetic_italic: request.synthetic_italic,
            glyph_id: request.glyph_id,
            size_bits: (request.font_size * request.scale).to_bits(),
            subpixel: request.x_variant,
        };
        Ok(self.inner.rasterize(fonts, &key).map(Image::from))
    }
}

pub(super) fn atlas_rgba(image: &Image, subpixel: bool) -> Result<Vec<u8>, String> {
    let pixels = u64::from(image.placement.width) * u64::from(image.placement.height);
    match image.content {
        Content::Mask if image.data.len() as u64 == pixels => Ok(image
            .data
            .iter()
            .flat_map(|&alpha| [alpha, alpha, alpha, alpha])
            .collect()),
        Content::Color if image.data.len() as u64 == pixels * 4 && !subpixel => {
            let mut output = image.data.clone();
            for pixel in output.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            Ok(output)
        }
        _ => Err("invalid GPUI glyph bitmap".into()),
    }
}
