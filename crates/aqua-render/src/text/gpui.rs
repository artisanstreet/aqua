// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

#[allow(dead_code)]
pub(crate) mod atlas;
pub mod map;
pub mod pairing;
#[allow(dead_code)]
pub(crate) mod pipeline;
mod provider;
pub(super) use provider::Provider;
#[allow(dead_code)]
mod raster;

pub const MASK_SHADER: &str = include_str!("gpui-mask.wgsl");

pub fn gamma_ratios(gamma: f32) -> [f32; 4] {
    const RATIOS: [[f32; 4]; 13] = [
        [0.0000 / 4.0, 0.0000 / 4.0, 0.0000 / 4.0, 0.0000 / 4.0],
        [0.0166 / 4.0, -0.0807 / 4.0, 0.2227 / 4.0, -0.0751 / 4.0],
        [0.0350 / 4.0, -0.1760 / 4.0, 0.4325 / 4.0, -0.1370 / 4.0],
        [0.0543 / 4.0, -0.2821 / 4.0, 0.6302 / 4.0, -0.1876 / 4.0],
        [0.0739 / 4.0, -0.3963 / 4.0, 0.8167 / 4.0, -0.2287 / 4.0],
        [0.0933 / 4.0, -0.5161 / 4.0, 0.9926 / 4.0, -0.2616 / 4.0],
        [0.1121 / 4.0, -0.6395 / 4.0, 1.1588 / 4.0, -0.2877 / 4.0],
        [0.1300 / 4.0, -0.7649 / 4.0, 1.3159 / 4.0, -0.3080 / 4.0],
        [0.1469 / 4.0, -0.8911 / 4.0, 1.4644 / 4.0, -0.3234 / 4.0],
        [0.1627 / 4.0, -1.0170 / 4.0, 1.6051 / 4.0, -0.3347 / 4.0],
        [0.1773 / 4.0, -1.1420 / 4.0, 1.7385 / 4.0, -0.3426 / 4.0],
        [0.1908 / 4.0, -1.2652 / 4.0, 1.8650 / 4.0, -0.3476 / 4.0],
        [0.2031 / 4.0, -1.3864 / 4.0, 1.9851 / 4.0, -0.3501 / 4.0],
    ];
    const NORM13: f32 = ((0x10000 as f64) / (255.0 * 255.0) * 4.0) as f32;
    const NORM24: f32 = ((0x100 as f64) / 255.0 * 4.0) as f32;
    let index = ((gamma * 10.0).round() as usize).clamp(10, 22) - 10;
    let ratio = RATIOS[index];
    [
        ratio[0] * NORM13,
        ratio[1] * NORM24,
        ratio[2] * NORM13,
        ratio[3] * NORM24,
    ]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderingParameters {
    pub gamma_ratios: [f32; 4],
    pub grayscale_enhanced_contrast: f32,
    pub subpixel_enhanced_contrast: f32,
    pub is_bgr: bool,
}

impl RenderingParameters {
    pub fn cosmic_from_env() -> Self {
        let value = |name: &str, fallback: f32| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(fallback)
        };
        static PARAMETERS: std::sync::OnceLock<RenderingParameters> = std::sync::OnceLock::new();
        *PARAMETERS.get_or_init(|| Self {
            gamma_ratios: gamma_ratios(value("ZED_FONTS_GAMMA", 1.8).clamp(1.0, 2.2)),
            grayscale_enhanced_contrast: value("ZED_FONTS_GRAYSCALE_ENHANCED_CONTRAST", 1.0)
                .max(0.0),
            subpixel_enhanced_contrast: value("ZED_FONTS_SUBPIXEL_ENHANCED_CONTRAST", 0.5).max(0.0),
            is_bgr: false,
        })
    }

    pub fn exact_coverage(self) -> Self {
        Self {
            gamma_ratios: [0.0; 4],
            grayscale_enhanced_contrast: 0.0,
            subpixel_enhanced_contrast: 0.0,
            ..self
        }
    }

    pub fn validate(self) -> Result<Self, &'static str> {
        if !self.gamma_ratios.into_iter().all(f32::is_finite)
            || !self.grayscale_enhanced_contrast.is_finite()
            || !self.subpixel_enhanced_contrast.is_finite()
            || self.grayscale_enhanced_contrast < 0.0
            || self.subpixel_enhanced_contrast < 0.0
        {
            return Err("invalid text rendering parameters");
        }
        Ok(self)
    }

    pub fn grayscale_coverage(self, sample: f32, color: [f32; 3]) -> f32 {
        let brightness = color[0] * 0.30 + color[1] * 0.59 + color[2] * 0.11;
        let k = self.grayscale_enhanced_contrast * (4.0 * (0.75 - brightness)).clamp(0.0, 1.0);
        let alpha = sample * (k + 1.0) / (sample * k + 1.0);
        correct_alpha(alpha, brightness, self.gamma_ratios)
    }

    pub fn subpixel_coverage(self, mut sample: [f32; 3], color: [f32; 3]) -> [f32; 3] {
        if self.is_bgr {
            sample.swap(0, 2);
        }
        let brightness = color[0] * 0.30 + color[1] * 0.59 + color[2] * 0.11;
        let k = self.subpixel_enhanced_contrast * (4.0 * (0.75 - brightness)).clamp(0.0, 1.0);
        std::array::from_fn(|index| {
            let alpha = sample[index] * (k + 1.0) / (sample[index] * k + 1.0);
            correct_alpha(alpha, color[index], self.gamma_ratios)
        })
    }
}

fn correct_alpha(alpha: f32, brightness: f32, gamma: [f32; 4]) -> f32 {
    let brightness_adjustment = gamma[0] * brightness + gamma[1];
    let correction = brightness_adjustment * alpha + (gamma[2] * brightness + gamma[3]);
    alpha + alpha * (1.0 - alpha) * correction
}

pub struct RasterObservation {
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub bgra: Vec<u8>,
}

pub fn observe_emoji_raster(
    fonts: &mut aqua_text::FontManager,
    glyph: &cosmic_text::LayoutGlyph,
    scale: f32,
) -> Result<Option<RasterObservation>, String> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err("invalid GPUI glyph scale".into());
    }
    let mut rasterizer = raster::Rasterizer::new();
    if !rasterizer.is_color_glyph(
        fonts.font_system_mut(),
        glyph.font_id,
        glyph.font_weight,
        glyph.glyph_id,
    )? {
        return Err("glyph is not a known color emoji".into());
    }
    let request = raster::GlyphRequest {
        font_id: glyph.font_id,
        weight: glyph.font_weight,
        glyph_id: glyph.glyph_id,
        font_size: glyph.font_size / scale,
        scale,
        x_variant: 0,
        is_emoji: true,
        subpixel: false,
        hint: true,
    };
    let image = rasterizer
        .rasterize(fonts.font_system_mut(), &request)?
        .ok_or("unable to render GPUI glyph via Swash")?;
    if image.placement.width == 0 || image.placement.height == 0 {
        return Ok(None);
    }
    let bgra = raster::atlas_rgba(&image, false)?;
    Ok(Some(RasterObservation {
        width: image.placement.width,
        height: image.placement.height,
        left: image.placement.left,
        top: image.placement.top,
        bgra,
    }))
}

#[cfg(feature = "gpui-diagnostics")]
#[derive(Clone)]
pub struct GlyphDiagnostic {
    pub text: String,
    pub glyph_ids: Vec<u16>,
}

#[cfg(feature = "gpui-diagnostics")]
static GLYPH_DIAGNOSTIC_DRAWS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(feature = "gpui-diagnostics")]
static GLYPH_DIAGNOSTIC: std::sync::OnceLock<std::sync::RwLock<Option<GlyphDiagnostic>>> =
    std::sync::OnceLock::new();

#[cfg(feature = "gpui-diagnostics")]
pub fn set_glyph_diagnostic(value: Option<GlyphDiagnostic>) -> Result<(), String> {
    if let Some(value) = &value {
        let mut unique = std::collections::HashSet::new();
        if value.text.is_empty()
            || value.glyph_ids.is_empty()
            || !value.glyph_ids.iter().all(|id| unique.insert(*id))
        {
            return Err("invalid GPUI glyph diagnostic selection".into());
        }
    }
    GLYPH_DIAGNOSTIC_DRAWS.store(0, std::sync::atomic::Ordering::Relaxed);
    *GLYPH_DIAGNOSTIC
        .get_or_init(Default::default)
        .write()
        .map_err(|_| "GPUI diagnostic lock poisoned")? = value;
    Ok(())
}

#[cfg(feature = "gpui-diagnostics")]
pub(super) fn glyph_diagnostic() -> Result<Option<GlyphDiagnostic>, String> {
    Ok(GLYPH_DIAGNOSTIC
        .get_or_init(Default::default)
        .read()
        .map_err(|_| "GPUI diagnostic lock poisoned")?
        .clone())
}

#[cfg(feature = "gpui-diagnostics")]
pub fn glyph_diagnostic_draw_count() -> usize {
    GLYPH_DIAGNOSTIC_DRAWS.load(std::sync::atomic::Ordering::Relaxed)
}
