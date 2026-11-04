use std::collections::HashMap;

use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_graph::{GlyphInstance, ImageInstance};
use aqua_media::atlas::{ColorGlyphBitmap, GlyphBitmap, GlyphKey, GlyphRaster, PageId, Slot};
use aqua_media::residency::{GlyphSlot, Residency, ResidencyError};
use aqua_media::upload::{glyph_instance, image_instance};

use crate::font::{FontId, FontManager};
use crate::layout::LaidOut;
use crate::raster::{place_glyph, quantize_size, GlyphImage, RasterKey, Rasterizer, SIZE_STEPS};
use crate::style::FontStretch;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FaceKey {
    font_id: FontId,
    weight: u16,
    stretch: FontStretch,
    synthetic_italic: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunStyle {
    pub color: Oklch,
    pub opacity: f32,
    pub clip: Option<Rect>,
}

#[derive(Clone, Debug, Default)]
pub struct TextDraw {
    pub masks: Vec<(PageId, GlyphInstance)>,
    pub colors: Vec<(PageId, ImageInstance)>,
    pub blank: usize,
}

#[derive(Default)]
pub struct GlyphResolver {
    interned: HashMap<FaceKey, u64>,
    faces: Vec<FaceKey>,
    rasterizer: Rasterizer,
}

fn raster_of(faces: &[FaceKey], key: &GlyphKey) -> Option<RasterKey> {
    let face = faces.get(usize::try_from(key.font).ok()?)?;
    Some(RasterKey {
        font_id: face.font_id,
        weight: face.weight,
        stretch: face.stretch,
        synthetic_italic: face.synthetic_italic,
        glyph_id: u16::try_from(key.glyph).ok()?,
        size_bits: quantize_size(key.size_sixty_fourths as f32 / SIZE_STEPS).to_bits(),
        subpixel: key.subpixel,
    })
}

fn into_raster(image: GlyphImage) -> GlyphRaster {
    match image {
        GlyphImage::Mask(bitmap) => GlyphRaster::Mask(GlyphBitmap {
            width: bitmap.width,
            height: bitmap.height,
            left: bitmap.left,
            top: bitmap.top,
            coverage: bitmap.data,
        }),
        GlyphImage::Color(bitmap) => GlyphRaster::Color(ColorGlyphBitmap {
            width: bitmap.width,
            height: bitmap.height,
            left: bitmap.left,
            top: bitmap.top,
            premultiplied_rgba: bitmap.data,
        }),
    }
}

impl GlyphResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn key(&mut self, key: &RasterKey) -> GlyphKey {
        let face = FaceKey {
            font_id: key.font_id,
            weight: key.weight,
            stretch: key.stretch,
            synthetic_italic: key.synthetic_italic,
        };
        let faces = &mut self.faces;
        let font = *self.interned.entry(face).or_insert_with(|| {
            faces.push(face);
            faces.len() as u64 - 1
        });
        GlyphKey {
            font,
            glyph: u32::from(key.glyph_id),
            size_sixty_fourths: (key.size_px() * SIZE_STEPS).round() as u32,
            subpixel: key.subpixel,
            variant: 0,
        }
    }

    pub fn resolve(
        &mut self,
        residency: &mut Residency,
        fonts: &mut FontManager,
        key: &RasterKey,
    ) -> Result<GlyphSlot, ResidencyError> {
        let glyph = self.key(key);
        let faces = &self.faces;
        let rasterizer = &mut self.rasterizer;
        let mut source = |requested: &GlyphKey| {
            raster_of(faces, requested)
                .and_then(|raster| rasterizer.rasterize(fonts, &raster))
                .filter(|image| !image.bitmap().is_empty())
                .map(into_raster)
        };
        residency.glyph_any(glyph, &mut source)
    }

    pub fn lower(
        &mut self,
        residency: &mut Residency,
        fonts: &mut FontManager,
        laid: &LaidOut,
        origin: [f32; 2],
        style: RunStyle,
    ) -> Result<TextDraw, ResidencyError> {
        let mut draw = TextDraw::default();
        for line in &laid.lines {
            for glyph in &line.glyphs {
                let placement = place_glyph(glyph, origin[0], origin[1] + line.baseline_y, true);
                let pen = [placement.x, placement.y];
                match self.resolve(residency, fonts, &placement.key)? {
                    GlyphSlot::Mask(Slot::Resident(resident)) => draw.masks.push((
                        resident.page,
                        glyph_instance(&resident, pen, style.color, style.clip, style.opacity),
                    )),
                    GlyphSlot::Color(Slot::Resident(resident)) => {
                        let bounds = Rect {
                            x: (pen[0] + resident.bearing[0]) as f32,
                            y: (pen[1] - resident.bearing[1]) as f32,
                            w: resident.width as f32,
                            h: resident.height as f32,
                        };
                        draw.colors.push((
                            resident.page,
                            image_instance(&resident, bounds, 0.0, style.opacity, style.clip),
                        ));
                    }
                    GlyphSlot::Mask(Slot::Blank { .. }) | GlyphSlot::Color(Slot::Blank { .. }) => {
                        draw.blank += 1;
                    }
                }
            }
        }
        Ok(draw)
    }
}
