use std::collections::HashMap;
use std::sync::Arc;

use aqua_core::geometry::Rect;
use aqua_core::scene::{GpuiTextColorMap, GpuiTextOpacityRange, GpuiTextShimmer, TextColorMap};
use aqua_core::{GpuiHsla, GpuiTextColor, Oklch};
use aqua_motion::{Cycle, CycleSet};
use aqua_text::layout::LaidGlyph;

use super::color::{self, Srgba};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextBase {
    Normalized(Srgba),
    Source(Srgba),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextScope {
    Color,
    SourceColor,
    Opacity,
    Map,
    LegacyMap,
}

#[derive(Clone, Debug, Default)]
pub struct TextStack {
    bases: Vec<TextBase>,
    opacities: Vec<Arc<[GpuiTextOpacityRange]>>,
    maps: Vec<(GpuiTextColorMap, Option<GpuiTextShimmer>)>,
    legacy: Vec<TextColorMap>,
}

impl TextStack {
    pub fn push_color(&mut self, color: GpuiHsla) {
        self.bases.push(TextBase::Normalized(color::hsla(color)));
    }

    pub fn push_source(&mut self, color: GpuiTextColor) {
        self.bases.push(TextBase::Source(color::text_color(color)));
    }

    pub fn push_opacity(&mut self, ranges: Arc<[GpuiTextOpacityRange]>) {
        self.opacities.push(ranges);
    }

    pub fn push_map(&mut self, map: GpuiTextColorMap, shimmer: Option<GpuiTextShimmer>) {
        self.maps.push((map, shimmer));
    }

    pub fn push_legacy(&mut self, map: TextColorMap) {
        self.legacy.push(map);
    }

    pub fn pop(&mut self, scope: TextScope) -> bool {
        match scope {
            TextScope::Color => matches!(self.bases.last(), Some(TextBase::Normalized(_)))
                .then(|| self.bases.pop())
                .is_some(),
            TextScope::SourceColor => matches!(self.bases.last(), Some(TextBase::Source(_)))
                .then(|| self.bases.pop())
                .is_some(),
            TextScope::Opacity => self.opacities.pop().is_some(),
            TextScope::Map => self.maps.pop().is_some(),
            TextScope::LegacyMap => self.legacy.pop().is_some(),
        }
    }

    #[must_use]
    pub fn is_balanced(&self) -> bool {
        self.bases.is_empty()
            && self.opacities.is_empty()
            && self.maps.is_empty()
            && self.legacy.is_empty()
    }

    pub fn clear(&mut self) {
        self.bases.clear();
        self.opacities.clear();
        self.maps.clear();
        self.legacy.clear();
    }

    #[must_use]
    pub fn shimmer(&self) -> Option<GpuiTextShimmer> {
        self.maps.last().and_then(|(_, shimmer)| *shimmer)
    }

    #[must_use]
    pub fn base(&self, glyph: &LaidGlyph, scene: Oklch) -> Srgba {
        glyph
            .source_color_hsla
            .map(color::source_array)
            .or(glyph.color_hsla.map(color::hsla_array))
            .or(glyph.color_srgb.map(|[r, g, b, a]| {
                [
                    f32::from(r) / 255.0,
                    f32::from(g) / 255.0,
                    f32::from(b) / 255.0,
                    f32::from(a) / 255.0,
                ]
            }))
            .or(self.bases.last().map(|base| match base {
                TextBase::Normalized(color) | TextBase::Source(color) => *color,
            }))
            .unwrap_or_else(|| color::srgb(scene))
    }

    #[must_use]
    pub fn range_opacity(&self, glyph: &LaidGlyph) -> f32 {
        self.opacities
            .last()
            .and_then(|ranges| {
                ranges
                    .iter()
                    .find(|range| range.bytes.contains(&glyph.src_byte_start))
            })
            .map_or(1.0, |range| range.opacity)
    }

    #[must_use]
    pub fn mapped(
        &self,
        color: Srgba,
        center_x: f32,
        phase: f32,
        map_bounds: Rect,
        legacy_scale: f32,
    ) -> Srgba {
        let shimmered = match self.maps.last() {
            Some((map, _)) => {
                let position = match map_bounds.w <= 0.0 {
                    true => f32::NAN,
                    false => (center_x - map_bounds.x) / map_bounds.w,
                };
                color::shimmer(
                    color,
                    color::text_color(map.target),
                    position,
                    phase,
                    map.spread,
                )
            }
            None => color,
        };
        self.legacy.iter().fold(shimmered, |current, map| {
            let start = map.start * legacy_scale;
            let end = map.end * legacy_scale;
            match end > start {
                true => color::legacy_map(
                    current,
                    color::srgb(map.target),
                    (center_x - start) / (end - start),
                    map.strength,
                ),
                false => current,
            }
        })
    }

    #[must_use]
    pub fn cache_signature(&self) -> Option<[u32; 5]> {
        match self.has_map() || !self.opacities.is_empty() {
            true => None,
            false => Some(match self.bases.last() {
                Some(TextBase::Normalized(color) | TextBase::Source(color)) => [
                    1,
                    color[0].to_bits(),
                    color[1].to_bits(),
                    color[2].to_bits(),
                    color[3].to_bits(),
                ],
                None => [0; 5],
            }),
        }
    }

    #[must_use]
    pub fn has_base(&self) -> bool {
        !self.bases.is_empty()
    }

    #[must_use]
    pub fn top_map(&self) -> Option<&GpuiTextColorMap> {
        self.maps.last().map(|(map, _)| map)
    }

    #[must_use]
    pub fn has_map(&self) -> bool {
        !self.maps.is_empty() || !self.legacy.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct ShimmerClocks {
    cycles: CycleSet<u64>,
    phases: HashMap<u64, f32>,
}

impl ShimmerClocks {
    pub fn begin_frame(&mut self) {
        self.phases.clear();
        self.cycles.begin_frame();
    }

    pub fn end_frame(&mut self) {
        self.cycles.end_frame();
    }

    #[must_use]
    pub fn active(&self) -> bool {
        self.cycles.active()
    }

    pub fn phase(&mut self, shimmer: &GpuiTextShimmer, now: f64) -> f32 {
        if let Some(phase) = self.phases.get(&shimmer.key) {
            return *phase;
        }
        let cycle = Cycle::Sweep {
            delay: shimmer.delay.as_secs_f64(),
            duration: shimmer.duration.as_secs_f64(),
        };
        let phase = self.cycles.sample(shimmer.key, cycle, now);
        self.phases.insert(shimmer.key, phase);
        phase
    }
}
