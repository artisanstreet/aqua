use cosmic_text::{FontSystem, SwashContent, SwashImage};
use swash::{
    scale::{Render, ScaleContext, Source, StrikeWith},
    zeno::{Format, Vector},
};

pub(super) struct GlyphRequest {
    pub font_id: fontdb::ID,
    pub weight: cosmic_text::Weight,
    pub glyph_id: u16,
    pub font_size: f32,
    pub scale: f32,
    pub x_variant: u8,
    pub is_emoji: bool,
    pub subpixel: bool,
    pub hint: bool,
}

pub(super) struct Rasterizer {
    context: ScaleContext,
}

impl Rasterizer {
    pub fn new() -> Self {
        Self {
            context: ScaleContext::new(),
        }
    }

    pub fn is_emoji_font(fonts: &FontSystem, font_id: fontdb::ID) -> Result<bool, String> {
        let face = fonts
            .db()
            .face(font_id)
            .ok_or("glyph font face is unavailable")?;
        Ok(matches!(
            face.post_script_name.as_str(),
            "NotoColorEmoji"
                | "SegoeUIEmoji"
                | "AppleColorEmoji"
                | ".AppleColorEmojiUI"
                | "TwemojiMozilla"
        ))
    }

    pub fn is_color_glyph(
        &mut self,
        fonts: &mut FontSystem,
        font_id: fontdb::ID,
        weight: cosmic_text::Weight,
        glyph_id: u16,
    ) -> Result<bool, String> {
        if !Self::is_emoji_font(fonts, font_id)? {
            return Ok(false);
        }
        let font = fonts
            .get_font(font_id, weight)
            .ok_or("glyph font is unavailable")?;
        let mut scaler = self.context.builder(font.as_swash()).build();
        Ok(scaler.scale_color_bitmap_into(
            glyph_id,
            StrikeWith::LargestSize,
            &mut swash::scale::image::Image::new(),
        ) || scaler
            .scale_color_outline_into(glyph_id, &mut swash::scale::outline::Outline::new()))
    }

    pub fn rasterize(
        &mut self,
        fonts: &mut FontSystem,
        request: &GlyphRequest,
    ) -> Result<Option<SwashImage>, String> {
        if !request.font_size.is_finite()
            || request.font_size <= 0.0
            || !request.scale.is_finite()
            || request.scale <= 0.0
            || request.x_variant >= 4
        {
            return Err("invalid GPUI glyph request".into());
        }
        let font = fonts
            .get_font(request.font_id, request.weight)
            .ok_or("glyph font is unavailable")?;
        let mut scaler = self
            .context
            .builder(font.as_swash())
            .size(request.font_size * request.scale)
            .hint(request.hint)
            .build();
        let sources: &[Source] = if request.is_emoji {
            &[
                Source::ColorOutline(0),
                Source::ColorBitmap(StrikeWith::BestFit),
                Source::Outline,
            ]
        } else {
            &[Source::Bitmap(StrikeWith::ExactSize), Source::Outline]
        };
        let mut renderer = Render::new(sources);
        let offset = Vector::new(f32::from(request.x_variant) / 4.0 / request.scale, 0.0);
        if request.subpixel {
            renderer.format(Format::subpixel_bgra()).offset(offset);
        } else {
            renderer.format(Format::Alpha).offset(offset);
        }
        Ok(renderer.render(&mut scaler, request.glyph_id))
    }
}

pub(super) fn atlas_rgba(image: &SwashImage, subpixel: bool) -> Result<Vec<u8>, String> {
    let pixels = u64::from(image.placement.width) * u64::from(image.placement.height);
    match image.content {
        SwashContent::Mask if image.data.len() as u64 == pixels => Ok(image
            .data
            .iter()
            .flat_map(|&alpha| [alpha, alpha, alpha, alpha])
            .collect()),
        SwashContent::SubpixelMask | SwashContent::Color
            if image.data.len() as u64 == pixels * 4 =>
        {
            let mut output = image.data.clone();
            if !subpixel && image.content == SwashContent::SubpixelMask {
                return Err("subpixel mask returned for grayscale glyph".into());
            }
            for pixel in output.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            Ok(output)
        }
        _ => Err("invalid GPUI glyph bitmap".into()),
    }
}
