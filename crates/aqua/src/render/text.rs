use std::collections::HashMap;
use std::sync::Arc;

use aqua_core::geometry::Rect;
use aqua_core::scene::{GpuiTextColorMap, GpuiTextOpacityRange, GpuiTextShimmer, TextColorMap};
use aqua_core::{GpuiHsla, GpuiTextColor, Oklch};
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

#[derive(Clone, Copy, Debug)]
struct Clock {
    since: f64,
    duration: f64,
    delay: f64,
    waiting: bool,
}

#[derive(Debug, Default)]
pub struct ShimmerClocks {
    clocks: HashMap<u64, Clock>,
    seen: Vec<u64>,
    phases: HashMap<u64, f32>,
}

impl ShimmerClocks {
    pub fn begin_frame(&mut self) {
        self.phases.clear();
        self.seen.clear();
    }

    pub fn end_frame(&mut self) {
        let seen = &self.seen;
        self.clocks.retain(|key, _| seen.contains(key));
    }

    #[must_use]
    pub fn active(&self) -> bool {
        !self.clocks.is_empty()
    }

    pub fn phase(&mut self, shimmer: &GpuiTextShimmer, now: f64) -> f32 {
        if let Some(phase) = self.phases.get(&shimmer.key) {
            return *phase;
        }
        self.seen.push(shimmer.key);
        let duration = shimmer.duration.as_secs_f64();
        let delay = shimmer.delay.as_secs_f64();
        let clock = self.clocks.entry(shimmer.key).or_insert(Clock {
            since: now,
            duration,
            delay,
            waiting: delay > 0.0,
        });
        clock.duration = duration;
        clock.delay = delay;
        let elapsed = (now - clock.since).max(0.0);
        match (clock.waiting, elapsed) {
            (true, elapsed) if elapsed >= clock.delay => {
                clock.waiting = false;
                clock.since = now;
            }
            (false, elapsed) if elapsed >= clock.duration => {
                clock.since = now;
                clock.waiting = clock.delay > 0.0;
            }
            _ => {}
        }
        let phase = match clock.waiting || clock.duration <= 0.0 {
            true => 0.0,
            false => ((now - clock.since).max(0.0) / clock.duration).min(1.0) as f32,
        };
        self.phases.insert(shimmer.key, phase);
        phase
    }
}
