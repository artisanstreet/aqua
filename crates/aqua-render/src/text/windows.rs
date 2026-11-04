use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::sync::Arc;

use aqua_text::raster::RasterKey;
use aqua_text::{FontEntry, FontManager};

use super::super::image::{Content, Image, Placement};
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{IUnknown, Interface};

struct NativeFace {
    face: IDWriteFontFace,
    _font: Arc<FontEntry>,
}

pub(super) struct NativeRasterizer {
    faces: HashMap<(aqua_text::FontId, u16, bool), NativeFace>,
    bytes: usize,
    loader: IDWriteInMemoryFontFileLoader,
    factory: IDWriteFactory5,
}

impl NativeRasterizer {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let factory: IDWriteFactory5 =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(error)?;
            let loader = factory.CreateInMemoryFontFileLoader().map_err(error)?;
            factory.RegisterFontFileLoader(&loader).map_err(error)?;
            Ok(Self {
                faces: HashMap::new(),
                bytes: 0,
                loader,
                factory,
            })
        }
    }

    pub fn rasterize(
        &mut self,
        fonts: &mut FontManager,
        key: &RasterKey,
    ) -> Result<Option<Image>, String> {
        let italic = key.synthetic_italic;
        let cache_key = (key.font_id, key.weight, italic);
        if !self.faces.contains_key(&cache_key) {
            if self.faces.len() >= 128 {
                self.faces.clear();
                self.bytes = 0;
            }
            let font = fonts
                .entry_with(key.font_id, key.weight, key.stretch, key.synthetic_italic)
                .ok_or("native font data unavailable")?;
            let index = font.face_index();
            if font.data().len() > 64 * 1024 * 1024 {
                return Err("native font exceeds byte budget".into());
            }
            if self.bytes.saturating_add(font.data().len()) > 64 * 1024 * 1024 {
                self.faces.clear();
                self.bytes = 0;
            }

            unsafe {
                let file = self
                    .loader
                    .CreateInMemoryFontFileReference(
                        &self.factory,
                        font.data().as_ptr().cast(),
                        u32::try_from(font.data().len())
                            .map_err(|_| "native font data too large")?,
                        None::<&IUnknown>,
                    )
                    .map_err(error)?;
                let mut supported = Default::default();
                let mut file_type = Default::default();
                let mut face_type = Default::default();
                let mut count = 0;
                file.Analyze(
                    &mut supported,
                    &mut file_type,
                    Some(&mut face_type),
                    &mut count,
                )
                .map_err(error)?;
                if !supported.as_bool() || index >= count {
                    return Err("native font face unsupported".into());
                }
                let simulations = if italic {
                    DWRITE_FONT_SIMULATIONS_OBLIQUE
                } else {
                    DWRITE_FONT_SIMULATIONS_NONE
                };
                let mut face = self
                    .factory
                    .CreateFontFace(face_type, &[Some(file)], index, simulations)
                    .map_err(error)?;
                if let Ok(variable) = face.cast::<IDWriteFontFace5>() {
                    if variable.HasVariations().as_bool() {
                        let resource = variable.GetFontResource().map_err(error)?;
                        let axis = DWRITE_FONT_AXIS_VALUE {
                            axisTag: DWRITE_FONT_AXIS_TAG_WEIGHT,
                            value: f32::from(key.weight),
                        };
                        face = resource
                            .CreateFontFace(simulations, &[axis])
                            .map_err(error)?
                            .cast()
                            .map_err(error)?;
                    }
                }
                self.bytes += font.data().len();
                self.faces
                    .insert(cache_key, NativeFace { face, _font: font });
            }
        }
        let face = &self.faces.get(&cache_key).unwrap().face;
        unsafe {
            let glyph_run = DWRITE_GLYPH_RUN {
                fontFace: ManuallyDrop::new(Some(face.clone())),
                fontEmSize: key.size_px(),
                glyphCount: 1,
                glyphIndices: &key.glyph_id,
                ..Default::default()
            };
            let analysis = self.factory.CreateGlyphRunAnalysis(
                &glyph_run,
                None,
                DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC,
                DWRITE_MEASURING_MODE_NATURAL,
                DWRITE_GRID_FIT_MODE_DEFAULT,
                DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                f32::from(key.subpixel) / 4.0,
                0.0,
            );
            let mut glyph_run = glyph_run;
            ManuallyDrop::drop(&mut glyph_run.fontFace);
            let analysis = analysis.map_err(error)?;
            let bounds = analysis
                .GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)
                .map_err(error)?;
            let width = u32::try_from(bounds.right - bounds.left)
                .map_err(|_| "invalid native glyph width")?;
            let height = u32::try_from(bounds.bottom - bounds.top)
                .map_err(|_| "invalid native glyph height")?;
            if width == 0 || height == 0 {
                return Ok(None);
            }
            let bytes = u64::from(width) * u64::from(height);
            if width > 4096 || height > 4096 || bytes > 16 * 1024 * 1024 {
                return Err("native glyph exceeds raster budget".into());
            }
            let mut data = vec![0; bytes as usize];
            analysis
                .CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &bounds, &mut data)
                .map_err(error)?;
            Ok(Some(Image {
                content: Content::Mask,
                placement: Placement {
                    left: bounds.left,
                    top: -bounds.top,
                    width,
                    height,
                },
                data,
            }))
        }
    }
}

impl Drop for NativeRasterizer {
    fn drop(&mut self) {
        self.faces.clear();
        unsafe {
            let _ = self.factory.UnregisterFontFileLoader(&self.loader);
        }
    }
}

fn error(error: windows::core::Error) -> String {
    format!("DirectWrite glyph rasterization: {error}")
}
