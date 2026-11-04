use std::collections::HashMap;
use std::sync::Arc;

use aqua_core::vector::VectorSource;

use crate::atlas::{
    Atlas, AtlasConfig, AtlasError, AtlasStats, AtlasUpdates, GlyphBitmap, GlyphKey, GlyphRaster,
    GlyphRasterizer, GlyphSource, ImageKey, PageId, Raster, Slot, StreamBudget, Streamer,
};
use crate::vector::{rasterize, retained_source_bytes, VectorBudget, VectorError};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VectorMode {
    Color,
    Mask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VectorKey {
    pub identity: usize,
    pub kind: u8,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MaskKey {
    Glyph(GlyphKey),
    Vector(VectorKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorKey {
    Image(ImageKey),
    Glyph(GlyphKey),
    Vector(VectorKey),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphSlot {
    Mask(Slot),
    Color(Slot),
}

impl GlyphSlot {
    pub fn slot(&self) -> &Slot {
        match self {
            GlyphSlot::Mask(slot) | GlyphSlot::Color(slot) => slot,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ResidencyConfig {
    pub masks: AtlasConfig,
    pub colors: AtlasConfig,
    pub stream: StreamBudget,
    pub vector: VectorBudget,
}

impl Default for ResidencyConfig {
    fn default() -> Self {
        Self {
            masks: AtlasConfig::glyphs(),
            colors: AtlasConfig::images(),
            stream: StreamBudget::default(),
            vector: VectorBudget::default(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResidencyError {
    Atlas(AtlasError),
    Vector(VectorError),
}

impl std::fmt::Display for ResidencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResidencyError::Atlas(error) => write!(f, "{error}"),
            ResidencyError::Vector(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ResidencyError {}

impl From<AtlasError> for ResidencyError {
    fn from(error: AtlasError) -> Self {
        ResidencyError::Atlas(error)
    }
}

impl From<VectorError> for ResidencyError {
    fn from(error: VectorError) -> Self {
        ResidencyError::Vector(error)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResidencyStats {
    pub masks: AtlasStats,
    pub colors: AtlasStats,
    pub streaming_bytes: u64,
    pub rasterizations: u64,
    pub retained_source_bytes: u64,
}

pub struct Residency {
    masks: Atlas<MaskKey>,
    colors: Atlas<ColorKey>,
    streamer: Streamer,
    stream: StreamBudget,
    vector: VectorBudget,
    retained: HashMap<VectorKey, (VectorSource, u64)>,
    rasterizations: u64,
    generation: u64,
}

fn vector_identity(source: &VectorSource) -> (usize, u8) {
    match source {
        VectorSource::Drawing(value) => (Arc::as_ptr(value) as usize, 0),
        VectorSource::Svg(value) => (Arc::as_ptr(value) as usize, 1),
    }
}

impl Residency {
    pub fn new(config: ResidencyConfig) -> Self {
        Self {
            masks: Atlas::new(config.masks),
            colors: Atlas::new(config.colors),
            streamer: Streamer::new(),
            stream: config.stream,
            vector: config.vector,
            retained: HashMap::new(),
            rasterizations: 0,
            generation: 0,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn begin_frame(&mut self) {
        self.masks.begin_frame();
        self.colors.begin_frame();
    }

    pub fn glyph(
        &mut self,
        key: GlyphKey,
        rasterizer: &mut impl GlyphRasterizer,
    ) -> Result<Slot, ResidencyError> {
        let produce = |_: &MaskKey| {
            rasterizer.rasterize(&key).map(
                |GlyphBitmap {
                     width,
                     height,
                     left,
                     top,
                     coverage,
                 }| Raster {
                    width,
                    height,
                    bearing: [left, top],
                    data: coverage,
                },
            )
        };
        Ok(self.masks.resolve(MaskKey::Glyph(key), produce)?)
    }

    pub fn glyph_any(
        &mut self,
        key: GlyphKey,
        source: &mut impl GlyphSource,
    ) -> Result<GlyphSlot, ResidencyError> {
        if let Some(slot) = self.masks.get(&MaskKey::Glyph(key)) {
            return Ok(GlyphSlot::Mask(slot));
        }
        let snapped = GlyphKey { subpixel: 0, ..key };
        if let Some(slot) = self
            .colors
            .get(&ColorKey::Glyph(key))
            .or_else(|| self.colors.get(&ColorKey::Glyph(snapped)))
        {
            return Ok(GlyphSlot::Color(slot));
        }
        let raster = match source.rasterize(&key) {
            Some(GlyphRaster::Color(_)) if key.subpixel != 0 => {
                source.rasterize(&snapped).map(|raster| (snapped, raster))
            }
            other => other.map(|raster| (key, raster)),
        };
        match raster {
            None => Ok(GlyphSlot::Mask(
                self.masks.insert_blank(MaskKey::Glyph(key), [0, 0]),
            )),
            Some((_, GlyphRaster::Mask(bitmap))) => Ok(GlyphSlot::Mask(self.masks.insert(
                MaskKey::Glyph(key),
                bitmap.width,
                bitmap.height,
                [bitmap.left, bitmap.top],
                bitmap.coverage,
            )?)),
            Some((stored, GlyphRaster::Color(bitmap))) => {
                Ok(GlyphSlot::Color(self.colors.insert(
                    ColorKey::Glyph(stored),
                    bitmap.width,
                    bitmap.height,
                    [bitmap.left, bitmap.top],
                    bitmap.premultiplied_rgba,
                )?))
            }
        }
    }

    pub fn image(
        &mut self,
        key: ImageKey,
        premultiplied_rgba: &[u8],
    ) -> Result<Slot, ResidencyError> {
        let produce = |_: &ColorKey| {
            Some(Raster {
                width: key.width,
                height: key.height,
                bearing: [0, 0],
                data: premultiplied_rgba.to_vec(),
            })
        };
        Ok(self.colors.resolve(ColorKey::Image(key), produce)?)
    }

    pub fn remove_image(&mut self, key: &ImageKey) -> bool {
        self.colors.remove(&ColorKey::Image(*key))
    }

    pub fn image_resident(&mut self, key: &ImageKey) -> Option<Slot> {
        self.colors.get(&ColorKey::Image(*key))
    }

    pub fn vector(
        &mut self,
        source: &VectorSource,
        width: u32,
        height: u32,
        mode: VectorMode,
    ) -> Result<Slot, ResidencyError> {
        let (identity, kind) = vector_identity(source);
        let key = VectorKey {
            identity,
            kind,
            width,
            height,
        };
        let budget = self.vector;
        let mut failure = None;
        let mut rasterized = false;
        let mut produce = |_: &VectorKey| match rasterize(source, width, height, budget) {
            Ok(raster) => {
                rasterized = true;
                Some(raster)
            }
            Err(error) => {
                failure = Some(error);
                None
            }
        };
        let slot = match mode {
            VectorMode::Mask => self.masks.resolve(MaskKey::Vector(key), |_| {
                produce(&key).map(|raster| Raster {
                    width,
                    height,
                    bearing: [0, 0],
                    data: raster.alpha(),
                })
            })?,
            VectorMode::Color => self.colors.resolve(ColorKey::Vector(key), |_| {
                produce(&key).map(|raster| Raster {
                    width,
                    height,
                    bearing: [0, 0],
                    data: raster.pixels,
                })
            })?,
        };
        if let Some(error) = failure {
            self.masks_or_colors_forget(mode, key);
            return Err(error.into());
        }
        if rasterized {
            self.rasterizations += 1;
            self.retained
                .insert(key, (source.clone(), retained_source_bytes(source)));
        }
        Ok(slot)
    }

    pub fn is_complete(&self, slot: &Slot) -> bool {
        match slot {
            Slot::Blank { .. } => true,
            Slot::Resident(placement) => !self.streamer.is_pending(placement.page),
        }
    }

    pub fn page_pending(&self, page: PageId) -> bool {
        self.streamer.is_pending(page)
    }

    pub fn finish_frame(&mut self) -> AtlasUpdates {
        let mut merged = self.masks.take_updates();
        merged.merge(self.colors.take_updates());
        self.masks.take_evicted().into_iter().for_each(|key| {
            if let MaskKey::Vector(vector) = key {
                self.forget(vector);
            }
        });
        self.colors.take_evicted().into_iter().for_each(|key| {
            if let ColorKey::Vector(vector) = key {
                self.forget(vector);
            }
        });
        self.streamer.submit(merged, self.stream)
    }

    pub fn trim_idle(&mut self, idle_frames: u64) -> usize {
        self.masks.trim_idle(idle_frames) + self.colors.trim_idle(idle_frames)
    }

    pub fn set_budgets(&mut self, masks: u64, colors: u64) {
        self.masks.set_budget(masks);
        self.colors.set_budget(colors);
        self.masks.enforce_budget();
        self.colors.enforce_budget();
    }

    pub fn clear(&mut self) {
        self.masks.clear();
        self.colors.clear();
        self.retained.clear();
        self.streamer = Streamer::new();
        self.generation += 1;
    }

    pub fn stats(&self) -> ResidencyStats {
        ResidencyStats {
            masks: self.masks.stats(),
            colors: self.colors.stats(),
            streaming_bytes: self.streamer.queued_bytes(),
            rasterizations: self.rasterizations,
            retained_source_bytes: self.retained.values().map(|(_, bytes)| *bytes).sum(),
        }
    }

    fn forget(&mut self, key: VectorKey) {
        let still_mask = self.masks.contains(&MaskKey::Vector(key));
        let still_color = self.colors.contains(&ColorKey::Vector(key));
        if !still_mask && !still_color {
            self.retained.remove(&key);
        }
    }

    fn masks_or_colors_forget(&mut self, mode: VectorMode, key: VectorKey) {
        match mode {
            VectorMode::Mask => {
                self.masks.remove(&MaskKey::Vector(key));
            }
            VectorMode::Color => {
                self.colors.remove(&ColorKey::Vector(key));
            }
        }
    }
}
