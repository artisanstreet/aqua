use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::ManuallyDrop;

use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::{
    DIBSECTION, GetCurrentObject, GetObjectW, HGDIOBJ, OBJ_BITMAP,
};
use windows::core::{IUnknown, Interface};

#[derive(Debug)]
pub enum OracleError {
    DirectWrite(String),
    UnsupportedFont,
    Bitmap(&'static str),
}

impl std::fmt::Display for OracleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OracleError::DirectWrite(message) => write!(formatter, "DirectWrite: {message}"),
            OracleError::UnsupportedFont => write!(formatter, "DirectWrite rejected the font data"),
            OracleError::Bitmap(message) => write!(formatter, "bitmap: {message}"),
        }
    }
}

impl std::error::Error for OracleError {}

fn failure(error: windows::core::Error) -> OracleError {
    OracleError::DirectWrite(error.to_string())
}

#[derive(Clone, Copy)]
pub struct FontSource<'a> {
    pub data: &'a [u8],
    pub face_index: u32,
    pub weight: u16,
}

#[derive(Clone, Copy)]
pub struct GlyphRunSpec<'a> {
    pub font: FontSource<'a>,
    pub size_px: f32,
    pub glyphs: &'a [u16],
    pub advances: &'a [f32],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShapedGlyph {
    pub glyph_id: u16,
    pub advance_px: f32,
}

#[derive(Clone, Debug)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct BlendParams {
    pub gamma: f32,
    pub enhanced_contrast: f32,
    pub foreground: [u8; 3],
    pub background: [u8; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    Unhinted,
    GridFitted,
}

struct Face {
    face: IDWriteFontFace,
    _file: IDWriteFontFile,
}

pub struct Oracle {
    factory: IDWriteFactory5,
    loader: IDWriteInMemoryFontFileLoader,
    faces: HashMap<(usize, u32, u16), Face>,
}

impl Oracle {
    pub fn new() -> Result<Self, OracleError> {
        unsafe {
            let factory: IDWriteFactory5 =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(failure)?;
            let loader = factory.CreateInMemoryFontFileLoader().map_err(failure)?;
            factory.RegisterFontFileLoader(&loader).map_err(failure)?;
            Ok(Self {
                factory,
                loader,
                faces: HashMap::new(),
            })
        }
    }

    fn face(&mut self, font: FontSource<'_>) -> Result<IDWriteFontFace, OracleError> {
        let key = (font.data.as_ptr() as usize, font.face_index, font.weight);
        if let Some(found) = self.faces.get(&key) {
            return Ok(found.face.clone());
        }
        unsafe {
            let file = self
                .loader
                .CreateInMemoryFontFileReference(
                    &self.factory,
                    font.data.as_ptr().cast(),
                    u32::try_from(font.data.len()).map_err(|_| OracleError::UnsupportedFont)?,
                    None::<&IUnknown>,
                )
                .map_err(failure)?;
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
            .map_err(failure)?;
            if !supported.as_bool() || font.face_index >= count {
                return Err(OracleError::UnsupportedFont);
            }
            let mut face = self
                .factory
                .CreateFontFace(
                    face_type,
                    &[Some(file.clone())],
                    font.face_index,
                    DWRITE_FONT_SIMULATIONS_NONE,
                )
                .map_err(failure)?;
            if let Ok(variable) = face.cast::<IDWriteFontFace5>() {
                if variable.HasVariations().as_bool() {
                    let resource = variable.GetFontResource().map_err(failure)?;
                    let axis = DWRITE_FONT_AXIS_VALUE {
                        axisTag: DWRITE_FONT_AXIS_TAG_WEIGHT,
                        value: f32::from(font.weight),
                    };
                    face = resource
                        .CreateFontFace(DWRITE_FONT_SIMULATIONS_NONE, &[axis])
                        .map_err(failure)?
                        .cast()
                        .map_err(failure)?;
                }
            }
            self.faces.insert(
                key,
                Face {
                    face: face.clone(),
                    _file: file,
                },
            );
            Ok(face)
        }
    }

    pub fn shape(
        &mut self,
        font: FontSource<'_>,
        size_px: f32,
        text: &str,
    ) -> Result<Vec<ShapedGlyph>, OracleError> {
        let face = self.face(font)?;
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let capacity = utf16.len() * 3 / 2 + 16;
        unsafe {
            let analyzer = self.factory.CreateTextAnalyzer().map_err(failure)?;
            let analysis = DWRITE_SCRIPT_ANALYSIS {
                script: 0,
                shapes: DWRITE_SCRIPT_SHAPES_DEFAULT,
            };
            let locale = windows::core::w!("en-us");
            let mut cluster_map = vec![0u16; utf16.len()];
            let mut text_props = vec![DWRITE_SHAPING_TEXT_PROPERTIES::default(); utf16.len()];
            let mut glyphs = vec![0u16; capacity];
            let mut glyph_props = vec![DWRITE_SHAPING_GLYPH_PROPERTIES::default(); capacity];
            let mut count = 0u32;
            analyzer
                .GetGlyphs(
                    windows::core::PCWSTR(utf16.as_ptr()),
                    utf16.len() as u32,
                    &face,
                    false,
                    false,
                    &analysis,
                    locale,
                    None::<&IDWriteNumberSubstitution>,
                    None,
                    None,
                    0,
                    capacity as u32,
                    cluster_map.as_mut_ptr(),
                    text_props.as_mut_ptr(),
                    glyphs.as_mut_ptr(),
                    glyph_props.as_mut_ptr(),
                    &mut count,
                )
                .map_err(failure)?;
            let mut advances = vec![0f32; count as usize];
            let mut offsets = vec![DWRITE_GLYPH_OFFSET::default(); count as usize];
            analyzer
                .GetGlyphPlacements(
                    windows::core::PCWSTR(utf16.as_ptr()),
                    cluster_map.as_ptr(),
                    text_props.as_mut_ptr(),
                    utf16.len() as u32,
                    glyphs.as_ptr(),
                    glyph_props.as_ptr(),
                    count,
                    &face,
                    size_px,
                    false,
                    false,
                    &analysis,
                    locale,
                    None,
                    None,
                    0,
                    advances.as_mut_ptr(),
                    offsets.as_mut_ptr(),
                )
                .map_err(failure)?;
            Ok(glyphs[..count as usize]
                .iter()
                .zip(advances)
                .map(|(glyph_id, advance_px)| ShapedGlyph {
                    glyph_id: *glyph_id,
                    advance_px,
                })
                .collect())
        }
    }

    pub fn coverage(
        &mut self,
        spec: GlyphRunSpec<'_>,
        origin: (f32, f32),
        size: (u32, u32),
        mode: RenderMode,
    ) -> Result<Canvas, OracleError> {
        let face = self.face(spec.font)?;
        let mut canvas = Canvas {
            width: size.0,
            height: size.1,
            pixels: vec![0; (size.0 * size.1) as usize],
        };
        unsafe {
            let mut run = DWRITE_GLYPH_RUN {
                fontFace: ManuallyDrop::new(Some(face)),
                fontEmSize: spec.size_px,
                glyphCount: spec.glyphs.len() as u32,
                glyphIndices: spec.glyphs.as_ptr(),
                glyphAdvances: spec.advances.as_ptr(),
                ..Default::default()
            };
            let analysis = self.factory.CreateGlyphRunAnalysis(
                &run,
                None,
                DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC,
                DWRITE_MEASURING_MODE_NATURAL,
                match mode {
                    RenderMode::Unhinted => DWRITE_GRID_FIT_MODE_DISABLED,
                    RenderMode::GridFitted => DWRITE_GRID_FIT_MODE_ENABLED,
                },
                DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                origin.0,
                origin.1,
            );
            ManuallyDrop::drop(&mut run.fontFace);
            let analysis = analysis.map_err(failure)?;
            let bounds = analysis
                .GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1)
                .map_err(failure)?;
            let width = (bounds.right - bounds.left).max(0) as usize;
            let height = (bounds.bottom - bounds.top).max(0) as usize;
            if width == 0 || height == 0 {
                return Ok(canvas);
            }
            let mut data = vec![0u8; width * height];
            analysis
                .CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &bounds, &mut data)
                .map_err(failure)?;
            for row in 0..height {
                for column in 0..width {
                    let x = bounds.left + column as i32;
                    let y = bounds.top + row as i32;
                    if x >= 0 && y >= 0 && (x as u32) < size.0 && (y as u32) < size.1 {
                        canvas.pixels[(y as u32 * size.0 + x as u32) as usize] =
                            data[row * width + column];
                    }
                }
            }
        }
        Ok(canvas)
    }

    pub fn blended(
        &mut self,
        spec: GlyphRunSpec<'_>,
        origin: (f32, f32),
        size: (u32, u32),
        params: BlendParams,
    ) -> Result<Canvas, OracleError> {
        let face = self.face(spec.font)?;
        unsafe {
            let interop = self.factory.GetGdiInterop().map_err(failure)?;
            let target = interop
                .CreateBitmapRenderTarget(None, size.0, size.1)
                .map_err(failure)?;
            target.SetPixelsPerDip(1.0).map_err(failure)?;
            let hdc = target.GetMemoryDC();
            let bitmap = GetCurrentObject(hdc, OBJ_BITMAP);
            let mut section = DIBSECTION::default();
            let written = GetObjectW(
                HGDIOBJ(bitmap.0),
                std::mem::size_of::<DIBSECTION>() as i32,
                Some((&mut section as *mut DIBSECTION).cast::<c_void>()),
            );
            if written == 0 || section.dsBm.bmBits.is_null() || section.dsBm.bmBitsPixel != 32 {
                return Err(OracleError::Bitmap("render target is not a 32-bit DIB"));
            }
            let stride = section.dsBm.bmWidthBytes as usize;
            let bits = std::slice::from_raw_parts_mut(
                section.dsBm.bmBits.cast::<u8>(),
                stride * size.1 as usize,
            );
            for pixel in bits.chunks_exact_mut(4) {
                pixel.copy_from_slice(&[
                    params.background[2],
                    params.background[1],
                    params.background[0],
                    255,
                ]);
            }
            let rendering: IDWriteRenderingParams = self
                .factory
                .CreateCustomRenderingParams(
                    params.gamma,
                    params.enhanced_contrast,
                    params.enhanced_contrast,
                    0.0,
                    DWRITE_PIXEL_GEOMETRY_FLAT,
                    DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC,
                    DWRITE_GRID_FIT_MODE_DISABLED,
                )
                .map_err(failure)?
                .cast()
                .map_err(failure)?;
            let mut run = DWRITE_GLYPH_RUN {
                fontFace: ManuallyDrop::new(Some(face)),
                fontEmSize: spec.size_px,
                glyphCount: spec.glyphs.len() as u32,
                glyphIndices: spec.glyphs.as_ptr(),
                glyphAdvances: spec.advances.as_ptr(),
                ..Default::default()
            };
            let mut black_box = RECT::default();
            let drawn = target.DrawGlyphRun(
                origin.0,
                origin.1,
                DWRITE_MEASURING_MODE_NATURAL,
                &run,
                &rendering,
                COLORREF(
                    u32::from(params.foreground[0])
                        | (u32::from(params.foreground[1]) << 8)
                        | (u32::from(params.foreground[2]) << 16),
                ),
                Some(&mut black_box),
            );
            ManuallyDrop::drop(&mut run.fontFace);
            drawn.map_err(failure)?;
            let mut pixels = vec![0u8; (size.0 * size.1 * 3) as usize];
            for y in 0..size.1 as usize {
                let source_row = y;
                for x in 0..size.0 as usize {
                    let source = source_row * stride + x * 4;
                    let target_index = (y * size.0 as usize + x) * 3;
                    pixels[target_index] = bits[source + 2];
                    pixels[target_index + 1] = bits[source + 1];
                    pixels[target_index + 2] = bits[source];
                }
            }
            Ok(Canvas {
                width: size.0,
                height: size.1,
                pixels,
            })
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SystemRendering {
    pub gamma: f32,
    pub enhanced_contrast: f32,
    pub grayscale_enhanced_contrast: f32,
}

impl Oracle {
    pub fn system_rendering(&self) -> Result<SystemRendering, OracleError> {
        unsafe {
            let params: IDWriteRenderingParams1 = self
                .factory
                .CreateRenderingParams()
                .map_err(failure)?
                .cast()
                .map_err(failure)?;
            Ok(SystemRendering {
                gamma: params.GetGamma(),
                enhanced_contrast: params.GetEnhancedContrast(),
                grayscale_enhanced_contrast: params.GetGrayscaleEnhancedContrast(),
            })
        }
    }
}

impl Drop for Oracle {
    fn drop(&mut self) {
        self.faces.clear();
        unsafe {
            let _ = self.factory.UnregisterFontFileLoader(&self.loader);
        }
    }
}
