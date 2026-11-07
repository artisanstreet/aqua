use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use unicode_segmentation::UnicodeSegmentation;

use crate::font::FontManager;
use crate::lines;
use aqua_media::hash::FastHasher;

use crate::shape::{scaled_line_height, shape_paragraph, ShapeContext, ShapedParagraph, SpanText};
use crate::shape_cache::paragraph_key;
use crate::style::{LineMode, StyledSpan, TextAlign, TextStyle, WrapMode};

#[derive(Clone, Debug)]
pub struct LaidGlyph {
    pub display_byte_start: usize,
    pub display_byte_end: usize,
    pub src_byte_start: usize,
    pub src_byte_end: usize,
    pub font_id: crate::font::FontId,
    pub font_weight: u16,
    pub font_stretch: crate::style::FontStretch,
    pub synthetic_italic: bool,
    pub glyph_id: u16,
    pub font_size_px: f32,
    pub x: f32,
    pub y: f32,
    pub x_offset: f32,
    pub y_offset: f32,
    pub w: f32,
    pub bidi_level: u8,
    pub color_srgb: Option<[u8; 4]>,
    pub color_hsla: Option<[f32; 4]>,
    pub source_color_hsla: Option<[f32; 4]>,
    pub font_max_width: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct LaidLine {
    pub paragraph: usize,
    pub display_range: std::ops::Range<usize>,
    pub y_top: f32,
    pub height: f32,
    pub baseline_y: f32,
    pub width: f32,
    pub rtl: bool,
    pub glyphs: Vec<LaidGlyph>,
}

#[derive(Clone, Debug, Default)]
pub struct LaidOut {
    pub size: (f32, f32),
    pub constrained_width: Option<f32>,
    pub lines: Vec<LaidLine>,
    pub first_baseline_y: Option<f32>,
}

impl LaidOut {
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.glyphs.is_empty())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaretAffinity {
    Upstream,
    #[default]
    Downstream,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaretPosition {
    pub byte: usize,
    pub affinity: CaretAffinity,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretRect {
    pub x: f32,
    pub y_top: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextInteractionRange {
    pub id: u64,
    pub range: std::ops::Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecorationKind {
    Underline,
    Strikethrough,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextDecoration {
    pub kind: DecorationKind,
    pub bounds: Rect,
    pub color_srgb: Option<[u8; 4]>,
    pub color_hsla: Option<[f32; 4]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextBackground {
    pub bounds: Rect,
    pub color_hsla: [f32; 4],
}

pub struct TextLayout {
    spans: Vec<StyledSpan>,
    wrap: WrapMode,
    align: TextAlign,
    line_mode: LineMode,
    max_width: Option<f32>,
    scale: f32,
    gpui_line_height: bool,
    gpui_wrap: bool,
    mask: Option<char>,
    shaped: Option<ShapedCache>,
    placed: HashMap<usize, PlacedParagraph>,
    reshapes: u64,
    last_key: Option<InputKey>,
    cached: Option<Arc<LaidOut>>,
    fonts_rev_seen: u64,
    display_text: String,
    para_starts: Vec<usize>,
    cluster_map: Vec<(usize, usize)>,
    display_len: usize,
    source_len: usize,
    overflow_source: Option<(std::ops::Range<usize>, crate::overflow::EllipsisPosition)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct InputKey {
    display_text: String,
    style_key: (u64, u64),
    wrap: WrapMode,
    align: TextAlign,
    line_mode: LineMode,
    width_bits: Option<u32>,
    scale_bits: u32,
    gpui_line_height: bool,
    gpui_wrap: bool,
    mask: Option<char>,
    fonts_rev: u64,
}

struct ShapedCache {
    key: InputKey,
    paragraphs: Vec<Arc<ShapedParagraph>>,
}

#[derive(Clone)]
struct PendingLine {
    placed: lines::PlacedLine,
    height: f32,
    ascent: f32,
    descent: f32,
}

#[derive(Clone, PartialEq)]
struct FinishKey {
    container_bits: u32,
    align: TextAlign,
    justify: bool,
    background_metrics: bool,
    colors: u64,
}

struct FinishedParagraph {
    key: FinishKey,
    lines: Vec<LaidLine>,
    height: f32,
    max_width: f32,
}

struct PlacedParagraph {
    paragraph: Arc<ShapedParagraph>,
    wrap: WrapMode,
    limit_bits: Option<u32>,
    lines: Vec<PendingLine>,
    content_width: f32,
    finished: Option<FinishedParagraph>,
}

fn place_paragraph(
    paragraph: &Arc<ShapedParagraph>,
    display: &str,
    wrap: WrapMode,
    limit: Option<f32>,
) -> PlacedParagraph {
    let breaks = paragraph.breaks.get_or_init(|| {
        lines::break_opportunities(&display[paragraph.start..paragraph.end], paragraph.start)
    });
    let ranges = lines::break_lines(&paragraph.clusters, breaks, wrap, limit);
    let last = ranges.len() - 1;
    let lines: Vec<PendingLine> = ranges
        .into_iter()
        .enumerate()
        .map(|(index, range)| {
            let placed = lines::place_line(paragraph, range, index == last);
            let (height, ascent, descent) = line_metrics(paragraph, &placed.clusters);
            PendingLine {
                placed,
                height,
                ascent,
                descent,
            }
        })
        .collect();
    let content_width = lines
        .iter()
        .map(|line| line.placed.content_width)
        .fold(0.0f32, f32::max);
    PlacedParagraph {
        paragraph: Arc::clone(paragraph),
        wrap,
        limit_bits: limit.map(f32::to_bits),
        lines,
        content_width,
        finished: None,
    }
}

fn finish_paragraph(
    paragraph: &ShapedParagraph,
    pending: &[PendingLine],
    key: FinishKey,
    container: f32,
    cluster_map: &[(usize, usize)],
    colors: &SpanColors,
    needs_background_metrics: bool,
) -> FinishedParagraph {
    let mut lines_out = Vec::with_capacity(pending.len());
    let mut max_w = 0.0f32;
    let mut total_h = 0.0f32;
    for line in pending {
        let mut line = line.clone();
        if key.justify {
            lines::justify(&mut line.placed, paragraph, container);
        }
        let offset = lines::alignment_offset(&line.placed, key.align, container);
        let y_top = total_h;
        let baseline_y = y_top + (line.height - (line.ascent + line.descent)) / 2.0 + line.ascent;
        let mut glyphs = Vec::with_capacity(line.placed.glyphs.len());
        for placed in &line.placed.glyphs {
            let cluster = &paragraph.clusters[placed.cluster];
            let run = &paragraph.runs[cluster.run()];
            let Some(entry) = &run.entry else { continue };
            let glyph = &run.glyphs[placed.glyph];
            if glyph.glyph_id == 3 && entry.is_emoji() {
                continue;
            }
            let (src_start, src_end) =
                display_range_to_source(cluster_map, cluster.start(), cluster.end());
            let (color_srgb, color_hsla, source_color_hsla) = colors.at(src_start);
            glyphs.push(LaidGlyph {
                display_byte_start: cluster.start(),
                display_byte_end: cluster.end(),
                src_byte_start: src_start,
                src_byte_end: src_end,
                font_id: entry.id(),
                font_weight: entry.weight(),
                font_stretch: entry.stretch(),
                synthetic_italic: entry.fake_italic(),
                glyph_id: glyph.glyph_id,
                font_size_px: run.size_px,
                x: offset + placed.x,
                y: 0.0,
                x_offset: glyph.x_offset,
                y_offset: glyph.y_offset,
                w: placed.advance,
                bidi_level: placed.level,
                color_srgb,
                color_hsla,
                source_color_hsla,
                font_max_width: needs_background_metrics
                    .then(|| entry.max_width_em() * run.size_px)
                    .filter(|width| *width > 0.0),
            });
        }
        let display_range = match line.placed.clusters.is_empty() {
            true => paragraph.start..paragraph.start,
            false => {
                paragraph.clusters[line.placed.clusters.start].start()
                    ..paragraph.clusters[line.placed.clusters.end - 1].end()
            }
        };
        max_w = max_w.max(line.placed.content_width);
        total_h = y_top + line.height;
        lines_out.push(LaidLine {
            paragraph: 0,
            display_range,
            y_top,
            height: line.height,
            baseline_y,
            width: line.placed.content_width,
            rtl: line.placed.rtl,
            glyphs,
        });
    }
    FinishedParagraph {
        key,
        lines: lines_out,
        height: total_h,
        max_width: max_w,
    }
}

fn color_signature(colors: &SpanColors, start: usize, end: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = FastHasher::default();
    let mut from = start;
    let mut index = colors.ends.partition_point(|span_end| *span_end <= start);
    loop {
        let to = colors
            .ends
            .get(index)
            .map_or(end, |span_end| (*span_end).min(end));
        to.saturating_sub(from).hash(&mut hasher);
        let (srgb, hsla, source) = colors
            .colors
            .get(index)
            .copied()
            .unwrap_or((None, None, None));
        srgb.hash(&mut hasher);
        hsla.map(|v| v.map(f32::to_bits)).hash(&mut hasher);
        source.map(|v| v.map(f32::to_bits)).hash(&mut hasher);
        if to >= end || index >= colors.ends.len() {
            break;
        }
        from = to;
        index += 1;
    }
    hasher.finish()
}

impl InputKey {
    fn same_shaping(&self, other: &InputKey) -> bool {
        self.fonts_rev == other.fonts_rev
            && self.scale_bits == other.scale_bits
            && self.gpui_line_height == other.gpui_line_height
            && self.mask == other.mask
            && self.line_mode == other.line_mode
            && self.style_key == other.style_key
            && self.display_text == other.display_text
    }
}

type GlyphColors = (Option<[u8; 4]>, Option<[f32; 4]>, Option<[f32; 4]>);

struct SpanColors {
    ends: Vec<usize>,
    colors: Vec<GlyphColors>,
}

impl SpanColors {
    fn at(&self, source_byte: usize) -> GlyphColors {
        let index = self.ends.partition_point(|end| *end <= source_byte);
        self.colors
            .get(index)
            .copied()
            .unwrap_or((None, None, None))
    }
}

fn span_colors(spans: &[StyledSpan]) -> SpanColors {
    let mut ends = Vec::with_capacity(spans.len());
    let mut colors = Vec::with_capacity(spans.len());
    let mut end = 0;
    for span in spans {
        end += span.text.len();
        ends.push(end);
        let color = span.color_hsla.or_else(|| {
            (span.color_srgb.is_none()
                && span.source_color_hsla.is_none()
                && span.style.gpui_source_color.is_none())
            .then_some(span.style.gpui_color)
            .flatten()
        });
        let source = span.source_color_hsla.or_else(|| {
            (span.color_srgb.is_none() && span.color_hsla.is_none())
                .then_some(span.style.gpui_source_color)
                .flatten()
        });
        colors.push((span.color_srgb, color, source));
    }
    SpanColors { ends, colors }
}

fn line_metrics(paragraph: &ShapedParagraph, range: &std::ops::Range<usize>) -> (f32, f32, f32) {
    if range.is_empty() {
        return (
            paragraph.empty_line_height,
            paragraph.empty_ascent,
            paragraph.empty_descent,
        );
    }
    let mut height = 0.0f32;
    let mut ascent = 0.0f32;
    let mut descent = 0.0f32;
    for cluster in &paragraph.clusters[range.clone()] {
        let run = &paragraph.runs[cluster.run()];
        height = height.max(run.line_height);
        if let Some(entry) = &run.entry {
            ascent = ascent.max(entry.ascent_em() * run.size_px);
            descent = descent.max(entry.descent_em() * run.size_px);
        }
    }
    (height, ascent, descent)
}

fn paragraph_ranges(display: &str) -> Vec<std::ops::Range<usize>> {
    let bytes = display.as_bytes();
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                ranges.push(start..index);
                index += 1;
                start = index;
            }
            b'\r' => {
                ranges.push(start..index);
                index += if bytes.get(index + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = index;
            }
            _ => index += 1,
        }
    }
    ranges.push(start..bytes.len());
    ranges
}

impl Default for TextLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl TextLayout {
    pub fn new() -> Self {
        Self {
            spans: vec![StyledSpan::plain("")],
            wrap: WrapMode::default(),
            align: TextAlign::Start,
            line_mode: LineMode::Multi,
            max_width: None,
            scale: 1.0,
            gpui_line_height: false,
            gpui_wrap: false,
            mask: None,
            shaped: None,
            placed: HashMap::new(),
            reshapes: 0,
            last_key: None,
            cached: None,
            fonts_rev_seen: 0,
            display_text: String::new(),
            para_starts: vec![0],
            cluster_map: Vec::new(),
            display_len: 0,
            source_len: 0,
            overflow_source: None,
        }
    }

    pub fn set_text(&mut self, text: &str, style: &TextStyle) {
        let spans = vec![StyledSpan::new(text.to_string(), style.clone())];
        self.set_spans(spans);
    }

    pub fn set_spans(&mut self, spans: Vec<StyledSpan>) {
        if spans.is_empty() {
            self.spans = vec![StyledSpan::plain("")];
        } else {
            self.spans = spans;
        }
        self.last_key = None;
    }

    pub fn set_wrap(&mut self, wrap: WrapMode) {
        if self.wrap != wrap {
            self.wrap = wrap;
            self.last_key = None;
        }
    }

    pub fn set_align(&mut self, align: TextAlign) {
        if self.align != align {
            self.align = align;
            self.last_key = None;
        }
    }

    pub fn set_line_mode(&mut self, mode: LineMode) {
        if self.line_mode != mode {
            self.line_mode = mode;
            self.last_key = None;
        }
    }

    pub fn set_max_width(&mut self, width: Option<f32>) {
        if self.max_width != width {
            self.max_width = width;
            self.last_key = None;
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        let scale = scale.max(0.01);
        if (self.scale - scale).abs() > f32::EPSILON {
            self.scale = scale;
            self.last_key = None;
        }
    }

    pub fn set_gpui_line_height(&mut self, enabled: bool) {
        if self.gpui_line_height != enabled {
            self.gpui_line_height = enabled;
            self.last_key = None;
        }
    }

    pub fn set_gpui_wrap(&mut self, enabled: bool) {
        if self.gpui_wrap != enabled {
            self.gpui_wrap = enabled;
            self.last_key = None;
        }
    }

    pub fn set_mask(&mut self, mask: Option<char>) {
        if self.mask != mask {
            self.mask = mask;
            self.last_key = None;
        }
    }

    pub fn has_gpui_colors(&self) -> bool {
        self.spans.iter().any(|span| {
            span.color_hsla.is_some()
                || span.style.gpui_color.is_some()
                || span.source_color_hsla.is_some()
                || span.style.gpui_source_color.is_some()
        })
    }

    pub fn reshape_count(&self) -> u64 {
        self.reshapes
    }

    pub fn source_len(&self) -> usize {
        self.source_len
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn laid_out(&self) -> Option<&LaidOut> {
        self.cached.as_deref()
    }

    pub fn laid_out_shared(&self) -> Option<Arc<LaidOut>> {
        self.cached.clone()
    }

    pub fn last_scaled_size(&self) -> Option<(f32, f32)> {
        self.cached.as_ref().map(|laid| laid.size)
    }

    pub fn layout(&mut self, fonts: &mut FontManager) -> Arc<LaidOut> {
        let fonts_rev = fonts_revision(fonts);
        let (display, cluster_map, source_len) =
            build_display(&self.spans, self.mask, self.line_mode);
        let key = self.input_key(fonts_rev, &display);

        if self.last_key.as_ref() == Some(&key) {
            if let Some(cached) = &self.cached {
                return Arc::clone(cached);
            }
        }

        self.shape(fonts, key, display, cluster_map, source_len, fonts_rev)
    }

    pub fn layout_single_line_overflow(
        &mut self,
        fonts: &mut FontManager,
        available_width: f32,
        mode: crate::overflow::OverflowMode,
    ) -> Arc<LaidOut> {
        use crate::overflow::{
            single_line_overflow_with_spacing, EllipsisPosition, OverflowCluster,
        };
        let natural = self.layout(fonts);
        if self.line_mode != LineMode::Single
            || self.mask.is_some()
            || mode == crate::overflow::OverflowMode::Clip
        {
            return natural;
        }
        let source: String = self.spans.iter().map(|span| span.text.as_str()).collect();
        if natural.size.0 <= available_width * self.scale {
            return natural;
        }
        let Some(first) = self.spans.first() else {
            return natural;
        };
        let mut measure_style = first.style.clone();
        let letter_spacing =
            measure_style.letter_spacing_em.unwrap_or(0.0) * measure_style.size_px * self.scale;
        measure_style.letter_spacing_em = None;
        let mut character_layout = TextLayout::new();
        character_layout.set_scale(self.scale);
        let mut widths = HashMap::new();
        let mut measure = |character: char| {
            *widths.entry(character).or_insert_with(|| {
                character_layout.set_text(&character.to_string(), &measure_style);
                character_layout.layout(fonts).size.0
            })
        };
        let ellipsis_width = measure('…') + letter_spacing;
        let clusters: Vec<_> = source
            .char_indices()
            .map(|(start, character)| OverflowCluster {
                bytes: start..start + character.len_utf8(),
                advance: measure(character),
            })
            .collect();
        let result = single_line_overflow_with_spacing(
            &source,
            &clusters,
            available_width * self.scale,
            ellipsis_width,
            letter_spacing,
            mode,
        );
        let Some(position) = result.ellipsis else {
            return natural;
        };
        let original_spans = self.spans.clone();
        let mut shown = Vec::new();
        let mut offset = 0;
        let edge = match position {
            EllipsisPosition::Start => result.source.start,
            EllipsisPosition::End => result.source.end.saturating_sub(1),
        };
        let mut ellipsis = original_spans
            .iter()
            .scan(0, |start, span| {
                let end = *start + span.text.len();
                let found = *start <= edge && edge < end;
                *start = end;
                Some((found, span))
            })
            .find(|(found, _)| *found)
            .map_or_else(|| original_spans[0].clone(), |(_, span)| span.clone());
        ellipsis.text = "…".to_owned();
        if position == EllipsisPosition::Start {
            shown.push(ellipsis.clone());
        }
        for span in &original_spans {
            let end = offset + span.text.len();
            let start_in_span = result
                .source
                .start
                .saturating_sub(offset)
                .min(span.text.len());
            let end_in_span = result
                .source
                .end
                .saturating_sub(offset)
                .min(span.text.len());
            if start_in_span < end_in_span {
                let mut part = span.clone();
                part.text = span.text[start_in_span..end_in_span].to_owned();
                shown.push(part);
            }
            offset = end;
        }
        if position == EllipsisPosition::End {
            shown.push(ellipsis);
        }
        self.set_spans(shown);
        let display = self.layout(fonts);
        let mut remapped = (*display).clone();
        let ellipsis_len = "…".len();
        let visible_len = result.source.end - result.source.start;
        for glyph in remapped.lines.iter_mut().flat_map(|line| &mut line.glyphs) {
            let map = |byte: usize| match position {
                EllipsisPosition::Start => {
                    result.source.start + byte.saturating_sub(ellipsis_len).min(visible_len)
                }
                EllipsisPosition::End => result.source.start + byte.min(visible_len),
            };
            glyph.src_byte_start = map(glyph.display_byte_start);
            glyph.src_byte_end = map(glyph.display_byte_end);
        }
        self.spans = original_spans;
        self.source_len = source.len();
        self.overflow_source = Some((result.source, position));
        self.last_key = None;
        let remapped = Arc::new(remapped);
        self.cached = Some(Arc::clone(&remapped));
        remapped
    }

    pub fn layout_shared(
        &mut self,
        fonts: &mut FontManager,
        cache: &mut SharedLayoutCache,
    ) -> Arc<LaidOut> {
        let fonts_rev = fonts_revision(fonts);
        let (display, cluster_map, source_len) =
            build_display(&self.spans, self.mask, self.line_mode);
        let key = self.input_key(fonts_rev, &display);

        if self.last_key.as_ref() == Some(&key) {
            if let Some(cached) = &self.cached {
                return Arc::clone(cached);
            }
        }
        if let Some(hit) = cache.map.get(&key) {
            cache.hits += 1;
            let laid = Arc::clone(hit);
            self.display_text = display;
            self.display_len = self.display_text.len();
            self.source_len = source_len;
            self.overflow_source = None;
            self.cluster_map = cluster_map;
            self.para_starts = para_starts_for(&laid);
            self.last_key = Some(key);
            self.fonts_rev_seen = fonts_rev;
            self.cached = Some(Arc::clone(&laid));
            return laid;
        }
        cache.misses += 1;
        let laid = self.shape(
            fonts,
            key.clone(),
            display,
            cluster_map,
            source_len,
            fonts_rev,
        );
        cache.insert(key, Arc::clone(&laid));
        laid
    }

    fn input_key(&self, fonts_rev: u64, display: &str) -> InputKey {
        let style_key = {
            use std::hash::{Hash, Hasher};
            let mut first = FastHasher::default();
            let mut second = FastHasher::default();
            second.write_u64(0x9e37_79b9_7f4a_7c15);
            for hasher in [&mut first, &mut second] {
                for s in &self.spans {
                    hasher.write_usize(s.text.len());
                    s.color_srgb.hash(hasher);
                    s.color_hsla.map(|v| v.map(f32::to_bits)).hash(hasher);
                    s.source_color_hsla
                        .map(|v| v.map(f32::to_bits))
                        .hash(hasher);
                    s.background_hsla.map(|v| v.map(f32::to_bits)).hash(hasher);
                    s.inline_cell_width().map(f32::to_bits).hash(hasher);
                    s.style.identity_hash(hasher);
                }
            }
            (first.finish(), second.finish())
        };
        InputKey {
            display_text: display.to_string(),
            style_key,
            wrap: self.wrap,
            align: self.align,
            line_mode: self.line_mode,
            width_bits: self.max_width.map(|w| (w * self.scale).to_bits()),
            scale_bits: self.scale.to_bits(),
            gpui_line_height: self.gpui_line_height,
            gpui_wrap: self.gpui_wrap,
            mask: self.mask,
            fonts_rev,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn shape(
        &mut self,
        fonts: &mut FontManager,
        key: InputKey,
        display: String,
        cluster_map: Vec<(usize, usize)>,
        source_len: usize,
        fonts_rev: u64,
    ) -> Arc<LaidOut> {
        let reusable = self
            .shaped
            .as_ref()
            .is_some_and(|cache| cache.key.same_shaping(&key));
        if !reusable {
            let mut span_ranges = Vec::with_capacity(self.spans.len());
            let mut offset = 0;
            for span in &self.spans {
                let length = match self.mask {
                    None => span.text.len(),
                    Some(_) => masked_span_text(&span.text, self.mask, self.line_mode).len(),
                };
                span_ranges.push(SpanText {
                    range: offset..offset + length,
                });
                offset += length;
            }
            let context = ShapeContext {
                spans: &self.spans,
                span_ranges: &span_ranges,
                scale: self.scale,
                gpui_line_height: self.gpui_line_height,
                inline_cells: self.mask.is_none(),
            };
            let paragraphs = paragraph_ranges(&display)
                .into_iter()
                .map(|range| {
                    let key = paragraph_key(&context, &display, &range, fonts_rev);
                    match fonts.shapes_mut().get(&key) {
                        Some(hit) => hit,
                        None => {
                            let shaped =
                                Arc::new(shape_paragraph(fonts, &context, &display, range));
                            fonts.shapes_mut().insert(key, Arc::clone(&shaped));
                            shaped
                        }
                    }
                })
                .collect();
            self.reshapes += 1;
            self.shaped = Some(ShapedCache {
                key: key.clone(),
                paragraphs,
            });
        }
        let paragraphs = &self
            .shaped
            .as_ref()
            .expect("shaped cache present")
            .paragraphs;

        let wrap = if self.line_mode == LineMode::Single || self.gpui_wrap {
            WrapMode::None
        } else {
            self.wrap
        };
        let width_px = self.max_width.map(|w| (w * self.scale).max(1.0));
        let limit = width_px.filter(|_| wrap != WrapMode::None);

        let cacheable = self.mask.is_none() && cluster_map.is_empty();
        let mut previous = std::mem::take(&mut self.placed);
        let mut placed_paragraphs: Vec<PlacedParagraph> = Vec::with_capacity(paragraphs.len());
        for paragraph in paragraphs.iter() {
            let reused = cacheable
                .then(|| previous.remove(&(Arc::as_ptr(paragraph) as usize)))
                .flatten()
                .filter(|entry| entry.wrap == wrap && entry.limit_bits == limit.map(f32::to_bits));
            placed_paragraphs.push(match reused {
                Some(entry) => entry,
                None => place_paragraph(paragraph, &display, wrap, limit),
            });
        }

        let max_content = placed_paragraphs
            .iter()
            .map(|entry| entry.content_width)
            .fold(0.0f32, f32::max);
        let container = width_px.unwrap_or(max_content);
        let colors = span_colors(&self.spans);
        let needs_background_metrics = self.spans.iter().any(|span| span.background_hsla.is_some());
        let finish_key = |paragraph: &ShapedParagraph| FinishKey {
            container_bits: container.to_bits(),
            align: self.align,
            justify: self.align == TextAlign::Justify && width_px.is_some(),
            background_metrics: needs_background_metrics,
            colors: color_signature(&colors, paragraph.start, paragraph.end),
        };

        let mut lines_out = Vec::new();
        let mut max_w = 0.0f32;
        let mut total_h = 0.0f32;
        let mut first_baseline_y = None;
        let retain_placed = placed_paragraphs.len() > 1;
        for (paragraph_index, entry) in placed_paragraphs.iter_mut().enumerate() {
            let paragraph = Arc::clone(&entry.paragraph);
            let key = finish_key(&paragraph);
            let reusable = cacheable
                && entry
                    .finished
                    .as_ref()
                    .is_some_and(|finished| finished.key == key);
            if !reusable {
                let finished = finish_paragraph(
                    &paragraph,
                    &entry.lines,
                    key,
                    container,
                    &cluster_map,
                    &colors,
                    needs_background_metrics,
                );
                entry.finished = Some(finished);
            }
            let finished = entry.finished.as_mut().expect("finished paragraph present");
            match retain_placed {
                true => {
                    for line in &finished.lines {
                        let mut line = line.clone();
                        line.paragraph = paragraph_index;
                        line.y_top += total_h;
                        line.baseline_y += total_h;
                        first_baseline_y.get_or_insert(line.baseline_y);
                        lines_out.push(line);
                    }
                }
                false => {
                    first_baseline_y = finished.lines.first().map(|line| line.baseline_y);
                    lines_out = std::mem::take(&mut finished.lines);
                }
            }
            max_w = max_w.max(finished.max_width);
            total_h += finished.height;
        }
        self.placed = match retain_placed {
            true => placed_paragraphs
                .into_iter()
                .map(|entry| (Arc::as_ptr(&entry.paragraph) as usize, entry))
                .collect(),
            false => HashMap::new(),
        };

        let mut lines = lines_out;
        if self.gpui_wrap && self.line_mode == LineMode::Multi && self.wrap != WrapMode::None {
            if let Some(width) = width_px {
                lines = gpui_wrapped_lines(lines, &display, width);
                max_w = lines.iter().map(|line| line.width).fold(0.0, f32::max);
                total_h = lines.last().map_or(0.0, |line| line.y_top + line.height);
            }
        }
        let mut para_starts: Vec<usize> = paragraphs.iter().map(|p| p.start).collect();
        para_starts.push(display.len());
        let laid = Arc::new(LaidOut {
            size: (max_w, total_h),
            constrained_width: width_px,
            lines,
            first_baseline_y,
        });
        self.display_text = display;
        self.display_len = self.display_text.len();
        self.source_len = source_len;
        self.overflow_source = None;
        self.para_starts = para_starts;
        self.cluster_map = cluster_map;
        self.last_key = Some(key);
        self.fonts_rev_seen = fonts_rev;
        self.cached = Some(Arc::clone(&laid));
        laid
    }

    fn to_display(&self, src: usize) -> usize {
        if let Some((range, position)) = &self.overflow_source {
            let offset = src.clamp(range.start, range.end) - range.start;
            return match position {
                crate::overflow::EllipsisPosition::Start => "…".len() + offset,
                crate::overflow::EllipsisPosition::End => offset,
            };
        }
        if self.mask.is_none() {
            return src.min(self.display_len);
        }
        let src = src.min(self.source_len);
        match self.cluster_map.binary_search_by(|(s, _)| s.cmp(&src)) {
            Ok(i) => self.cluster_map[i].1,
            Err(i) => {
                if i == 0 {
                    0
                } else {
                    self.cluster_map[i - 1].1
                }
            }
        }
    }

    fn to_source(&self, display: usize) -> usize {
        if let Some((range, position)) = &self.overflow_source {
            let offset = match position {
                crate::overflow::EllipsisPosition::Start => display.saturating_sub("…".len()),
                crate::overflow::EllipsisPosition::End => display,
            };
            return range.start + offset.min(range.end - range.start);
        }
        if self.mask.is_none() {
            return display.min(self.source_len);
        }
        let (s, _) = display_range_to_source(&self.cluster_map, display, display);
        s
    }

    pub fn caret(&self, src_byte: usize) -> Option<CaretRect> {
        self.caret_position(CaretPosition {
            byte: src_byte,
            affinity: CaretAffinity::Downstream,
        })
    }

    pub fn caret_position(&self, position: CaretPosition) -> Option<CaretRect> {
        let laid = self.cached.as_ref()?;
        let byte = self.to_display(position.byte);
        let line =
            match position.affinity {
                CaretAffinity::Upstream => laid.lines.iter().find(|line| {
                    byte >= line.display_range.start && byte <= line.display_range.end
                }),
                CaretAffinity::Downstream => laid.lines.iter().rev().find(|line| {
                    byte >= line.display_range.start && byte <= line.display_range.end
                }),
            }
            .or_else(|| {
                laid.lines
                    .iter()
                    .rev()
                    .find(|line| byte >= line.display_range.start)
            })
            .or_else(|| laid.lines.first())?;
        let preferred = line.glyphs.iter().find(|glyph| match position.affinity {
            CaretAffinity::Upstream => glyph.display_byte_end == byte,
            CaretAffinity::Downstream => glyph.display_byte_start == byte,
        });
        let x = preferred
            .map(|glyph| {
                let rtl = glyph.bidi_level % 2 == 1;
                match (position.affinity, rtl) {
                    (CaretAffinity::Upstream, false) | (CaretAffinity::Downstream, true) => {
                        glyph.x + glyph.w
                    }
                    _ => glyph.x,
                }
            })
            .unwrap_or_else(|| {
                caret_x_in_line(line, byte, laid.constrained_width, &self.display_text)
            });
        Some(CaretRect {
            x,
            y_top: line.y_top,
            height: line.height,
        })
    }

    pub fn hit_position(&self, x: f32, y: f32) -> Option<CaretPosition> {
        let laid = self.cached.as_ref()?;
        let line = laid
            .lines
            .iter()
            .find(|line| y < line.y_top + line.height)
            .or_else(|| laid.lines.last())?;
        let positions = glyph_positions(line, &self.display_text);
        let (_, mut position) = positions
            .into_iter()
            .min_by(|a, b| (a.0 - x).abs().total_cmp(&(b.0 - x).abs()))
            .unwrap_or((
                0.0,
                CaretPosition {
                    byte: line.display_range.start,
                    affinity: CaretAffinity::Downstream,
                },
            ));
        position.byte = self.to_source(position.byte);
        Some(position)
    }

    pub fn interaction_ranges(&self) -> Vec<TextInteractionRange> {
        let mut offset = 0;
        self.spans
            .iter()
            .filter_map(|span| {
                let range = offset..offset + span.text.len();
                offset = range.end;
                span.interaction
                    .map(|id| TextInteractionRange { id, range })
            })
            .collect()
    }

    pub fn gpui_backgrounds(&self) -> Result<Vec<TextBackground>, &'static str> {
        let laid = self.cached.as_ref().ok_or("text has not been shaped")?;
        let mut output = Vec::new();
        let mut span_index = 0;
        let mut run_end = 0;
        let mut current: Option<(f32, [f32; 4])> = None;
        for (line_index, line) in laid.lines.iter().enumerate() {
            for glyph in &line.glyphs {
                if glyph.src_byte_start < run_end {
                    continue;
                }
                let mut style = None;
                while let Some(span) = self.spans.get(span_index) {
                    span_index += 1;
                    run_end += span.text.len();
                    if glyph.src_byte_start < run_end {
                        style = Some(span.background_hsla);
                        break;
                    }
                }
                let background = style.flatten();
                if current.as_ref().map(|(_, color)| *color) != background {
                    if let Some((mut x, color_hsla)) = current.take() {
                        let width = glyph.x - x;
                        if width == 0.0 {
                            x -= glyph
                                .font_max_width
                                .ok_or("GPUI background requires font metrics")?
                                / 2.0;
                        }
                        output.push(TextBackground {
                            bounds: Rect {
                                x,
                                y: line.y_top,
                                w: width,
                                h: line.height,
                            },
                            color_hsla,
                        });
                    }
                    if let Some(color) = background {
                        current = Some((glyph.x, color));
                    }
                }
            }
            if let Some((mut x, color_hsla)) = current.take() {
                let next = laid
                    .lines
                    .get(line_index + 1)
                    .filter(|next| next.paragraph == line.paragraph)
                    .and_then(|next| next.glyphs.first());
                let mut width = line.width - x;
                if width == 0.0 {
                    let boundary = next
                        .or_else(|| line.glyphs.last())
                        .ok_or("GPUI background requires a glyph")?;
                    let half = boundary
                        .font_max_width
                        .ok_or("GPUI background requires font metrics")?
                        / 2.0;
                    x -= half;
                    width = half;
                }
                output.push(TextBackground {
                    bounds: Rect {
                        x,
                        y: line.y_top,
                        w: width,
                        h: line.height,
                    },
                    color_hsla,
                });
                if next.is_some_and(|glyph| glyph.src_byte_start < run_end) {
                    current = Some((0.0, color_hsla));
                }
            }
        }
        Ok(output)
    }

    pub fn decorations(&self) -> Vec<TextDecoration> {
        let Some(laid) = self.cached.as_ref() else {
            return Vec::new();
        };
        let mut offset = 0;
        let mut output = Vec::new();
        for span in &self.spans {
            let range = offset..offset + span.text.len();
            offset = range.end;
            if !span.style.underline && !span.style.strikethrough {
                continue;
            }
            let thickness = (span.style.size_px * self.scale / 14.0).max(1.0);
            for bounds in self.selection_rects(range) {
                let Some(line) = laid.lines.iter().find(|line| line.y_top == bounds.y) else {
                    continue;
                };
                for (enabled, kind, y) in [
                    (
                        span.style.underline,
                        DecorationKind::Underline,
                        line.baseline_y + span.style.size_px * self.scale * 0.08,
                    ),
                    (
                        span.style.strikethrough,
                        DecorationKind::Strikethrough,
                        line.baseline_y - span.style.size_px * self.scale * 0.3,
                    ),
                ] {
                    if enabled {
                        output.push(TextDecoration {
                            kind,
                            bounds: Rect {
                                x: bounds.x,
                                y,
                                w: bounds.w,
                                h: thickness,
                            },
                            color_srgb: span.color_srgb,
                            color_hsla: span.color_hsla,
                        });
                    }
                }
            }
        }
        output
    }

    pub fn word_hit(&self, x: f32, y: f32) -> Option<usize> {
        let laid = self.cached.as_ref()?;
        let line = laid
            .lines
            .iter()
            .find(|line| y < line.y_top + line.height)
            .or_else(|| laid.lines.last())?;
        let glyph = line
            .glyphs
            .iter()
            .find(|glyph| glyph.w > 0.0 && x >= glyph.x && x < glyph.x + glyph.w)
            .or_else(|| {
                line.glyphs
                    .iter()
                    .filter(|glyph| glyph.w > 0.0)
                    .min_by(|a, b| {
                        let distance =
                            |glyph: &LaidGlyph| (glyph.x - x).max(0.0).max(x - glyph.x - glyph.w);
                        distance(a).total_cmp(&distance(b))
                    })
            });
        let Some(glyph) = glyph else {
            return Some(self.to_source(line.display_range.start));
        };
        let cluster = self
            .display_text
            .get(glyph.display_byte_start..glyph.display_byte_end)?;
        let fraction = ((x - glyph.x) / glyph.w).clamp(0.0, 1.0);
        let fraction = if glyph.bidi_level % 2 == 1 {
            1.0 - fraction
        } else {
            fraction
        };
        let count = cluster.graphemes(true).count();
        let index = ((fraction * count as f32).floor() as usize).min(count.saturating_sub(1));
        let offset = cluster
            .grapheme_indices(true)
            .nth(index)
            .map_or(0, |(offset, _)| offset);
        Some(self.to_source(glyph.display_byte_start + offset))
    }

    pub fn horizontal_position(
        &self,
        position: CaretPosition,
        left: bool,
    ) -> Option<CaretPosition> {
        let laid = self.cached.as_ref()?;
        let caret = self.caret_position(position)?;
        let index = laid
            .lines
            .iter()
            .position(|line| line.y_top == caret.y_top)?;
        let line = &laid.lines[index];
        let target = glyph_positions(line, &self.display_text)
            .into_iter()
            .filter(|(x, _)| {
                if left {
                    *x < caret.x - 0.01
                } else {
                    *x > caret.x + 0.01
                }
            })
            .min_by(|a, b| (a.0 - caret.x).abs().total_cmp(&(b.0 - caret.x).abs()));
        if let Some((_, mut target)) = target {
            target.byte = self.to_source(target.byte);
            return Some(target);
        }
        let previous = left != line.rtl;
        let next = if previous {
            index.checked_sub(1)
        } else {
            (index + 1 < laid.lines.len()).then_some(index + 1)
        };
        let Some(next) = next else {
            return Some(position);
        };
        let next = &laid.lines[next];
        let target = CaretPosition {
            byte: self.to_source(if previous {
                next.display_range.end
            } else {
                next.display_range.start
            }),
            affinity: if previous {
                CaretAffinity::Upstream
            } else {
                CaretAffinity::Downstream
            },
        };
        if target.byte == position.byte {
            self.horizontal_position(target, left)
        } else {
            Some(target)
        }
    }

    pub fn line_boundary_position(
        &self,
        position: CaretPosition,
        end: bool,
    ) -> Option<CaretPosition> {
        let laid = self.cached.as_ref()?;
        let caret = self.caret_position(position)?;
        let line = laid.lines.iter().find(|line| line.y_top == caret.y_top)?;
        Some(CaretPosition {
            byte: self.to_source(if end {
                line.display_range.end
            } else {
                line.display_range.start
            }),
            affinity: if end {
                CaretAffinity::Upstream
            } else {
                CaretAffinity::Downstream
            },
        })
    }

    pub fn vertical_position(
        &self,
        position: CaretPosition,
        up: bool,
        goal_x: f32,
    ) -> Option<CaretPosition> {
        self.block_position(position, up, goal_x, false)
    }

    pub fn paragraph_position(
        &self,
        position: CaretPosition,
        up: bool,
        goal_x: f32,
    ) -> Option<CaretPosition> {
        self.block_position(position, up, goal_x, true)
    }

    fn block_position(
        &self,
        position: CaretPosition,
        up: bool,
        goal_x: f32,
        paragraph: bool,
    ) -> Option<CaretPosition> {
        let laid = self.cached.as_ref()?;
        let caret = self.caret_position(position)?;
        let index = laid
            .lines
            .iter()
            .position(|line| line.y_top == caret.y_top)?;
        let current = laid.lines[index].paragraph;
        let target = if up {
            laid.lines[..index]
                .iter()
                .rev()
                .find(|line| !paragraph || line.paragraph != current)
        } else {
            laid.lines[index + 1..]
                .iter()
                .find(|line| !paragraph || line.paragraph != current)
        };
        let Some(line) = target else {
            return Some(CaretPosition {
                byte: if up { 0 } else { self.source_len },
                affinity: if up {
                    CaretAffinity::Downstream
                } else {
                    CaretAffinity::Upstream
                },
            });
        };
        self.hit_position(goal_x, line.y_top + line.height * 0.5)
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        let laid = self.cached.as_ref()?;
        if laid.lines.is_empty() {
            return Some(0);
        }
        let mut line_idx = 0;
        for (i, line) in laid.lines.iter().enumerate() {
            if y >= line.y_top + line.height {
                line_idx = i;
                continue;
            }
            if y >= line.y_top {
                line_idx = i;
                break;
            }
            line_idx = i;
            break;
        }
        let line = &laid.lines[line_idx.min(laid.lines.len() - 1)];
        let d = hit_in_line(&self.display_text, line, x);
        Some(self.to_source(d))
    }

    pub fn horizontal_target(&self, src_byte: usize, left: bool) -> Option<usize> {
        let laid = self.cached.as_ref()?;
        let caret = self.caret(src_byte)?;
        let index = laid
            .lines
            .iter()
            .position(|line| line.y_top == caret.y_top)?;
        let line = &laid.lines[index];
        let edges = glyph_edges(line, &self.display_text);
        let target = edges
            .iter()
            .filter(|(x, _)| {
                if left {
                    *x < caret.x - 0.01
                } else {
                    *x > caret.x + 0.01
                }
            })
            .min_by(|a, b| (a.0 - caret.x).abs().total_cmp(&(b.0 - caret.x).abs()));
        if let Some((_, byte)) = target {
            return Some(self.to_source(*byte));
        }
        let previous = left != line.rtl;
        let next = if previous {
            index.checked_sub(1)
        } else {
            (index + 1 < laid.lines.len()).then_some(index + 1)
        };
        let Some(next) = next else {
            return Some(src_byte.min(self.source_len));
        };
        let next = &laid.lines[next];
        Some(self.to_source(if previous {
            next.display_range.end
        } else {
            next.display_range.start
        }))
    }

    pub fn line_boundary(&self, src_byte: usize, end: bool) -> Option<usize> {
        let laid = self.cached.as_ref()?;
        let d = self.to_display(src_byte);
        let line = laid
            .lines
            .iter()
            .rev()
            .find(|line| d >= line.display_range.start)
            .or_else(|| laid.lines.first())?;
        Some(self.to_source(if end {
            line.display_range.end
        } else {
            line.display_range.start
        }))
    }

    pub fn paragraph_target(&self, src_byte: usize, up: bool, goal_x: f32) -> Option<usize> {
        let laid = self.cached.as_ref()?;
        let caret = self.caret(src_byte)?;
        let index = laid
            .lines
            .iter()
            .position(|line| line.y_top == caret.y_top)?;
        let paragraph = laid.lines[index].paragraph;
        let target = if up {
            laid.lines[..index]
                .iter()
                .rev()
                .find(|line| line.paragraph != paragraph)
        } else {
            laid.lines[index + 1..]
                .iter()
                .find(|line| line.paragraph != paragraph)
        };
        let Some(line) = target else {
            return Some(if up { 0 } else { self.source_len });
        };
        self.hit(goal_x, line.y_top + line.height * 0.5)
    }

    pub fn vertical_target(&self, src_byte: usize, up: bool, goal_x: f32) -> Option<usize> {
        let laid = self.cached.as_ref()?;
        let caret = self.caret(src_byte)?;
        let index = laid
            .lines
            .iter()
            .position(|line| line.y_top == caret.y_top)?;
        let target = if up {
            index.checked_sub(1)
        } else {
            (index + 1 < laid.lines.len()).then_some(index + 1)
        };
        let Some(target) = target else {
            return Some(if up { 0 } else { self.source_len });
        };
        let line = &laid.lines[target];
        self.hit(goal_x, line.y_top + line.height * 0.5)
    }

    pub fn selection_rects(&self, range: std::ops::Range<usize>) -> Vec<Rect> {
        let Some(laid) = self.cached.as_ref() else {
            return Vec::new();
        };
        let start = snap_to_boundary(
            &self.display_text,
            self.to_display(range.start),
            0..self.display_len,
        );
        let end = snap_to_boundary(
            &self.display_text,
            self.to_display(range.end),
            0..self.display_len,
        );
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let mut out = Vec::new();
        for line in &laid.lines {
            let mut spans: Vec<(f32, f32)> = Vec::new();
            for glyph in &line.glyphs {
                let from = start.max(glyph.display_byte_start);
                let to = end.min(glyph.display_byte_end);
                if from >= to {
                    continue;
                }
                let Some(cluster) = self
                    .display_text
                    .get(glyph.display_byte_start..glyph.display_byte_end)
                else {
                    continue;
                };
                let count = cluster.graphemes(true).count().max(1) as f32;
                let start_count = cluster[..from - glyph.display_byte_start]
                    .graphemes(true)
                    .count() as f32;
                let end_count = cluster[..to - glyph.display_byte_start]
                    .graphemes(true)
                    .count() as f32;
                let (leading, trailing) = if glyph.bidi_level % 2 == 1 {
                    (glyph.x + glyph.w, glyph.x)
                } else {
                    (glyph.x, glyph.x + glyph.w)
                };
                let left = leading + (trailing - leading) * start_count / count;
                let right = leading + (trailing - leading) * end_count / count;
                spans.push((left.min(right), left.max(right)));
            }
            spans.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut merged: Vec<(f32, f32)> = Vec::new();
            for (left, right) in spans {
                match merged.last_mut() {
                    Some(last) if left <= last.1 + 0.1 => last.1 = last.1.max(right),
                    _ => merged.push((left, right)),
                }
            }
            out.extend(merged.into_iter().map(|(left, right)| Rect {
                x: left,
                y: line.y_top,
                w: right - left,
                h: line.height,
            }));
        }
        out
    }

    pub fn single_line_height(&self) -> f32 {
        if let Some(s) = self.spans.first() {
            scaled_line_height(s.style.line_height_px, self.scale, self.gpui_line_height)
        } else {
            16.0 * self.scale
        }
    }

    pub fn display_text(&self) -> &str {
        &self.display_text
    }
}

pub fn measure_text(
    fonts: &mut FontManager,
    text: &str,
    style: &TextStyle,
    max_width: Option<f32>,
    scale: f32,
) -> (f32, f32) {
    let mut layout = TextLayout::new();
    layout.set_text(text, style);
    layout.set_line_mode(LineMode::Multi);
    layout.set_max_width(max_width);
    layout.set_scale(scale);
    let laid = layout.layout(fonts);
    let inv = 1.0 / scale.max(0.01);
    (laid.size.0 * inv, laid.size.1 * inv)
}

pub struct SharedLayoutCache {
    map: HashMap<InputKey, Arc<LaidOut>>,
    order: VecDeque<InputKey>,
    capacity: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

impl SharedLayoutCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    fn insert(&mut self, key: InputKey, laid: Arc<LaidOut>) {
        if self.map.contains_key(&key) {
            return;
        }
        while self.map.len() >= self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
                self.evictions += 1;
            } else {
                break;
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, laid);
    }
}

fn fonts_revision(fonts: &FontManager) -> u64 {
    fonts.font_revision()
}

fn para_starts_for(laid: &LaidOut) -> Vec<usize> {
    let mut starts: Vec<usize> = laid.lines.iter().map(|l| l.display_range.start).collect();
    let end = laid.lines.last().map(|l| l.display_range.end).unwrap_or(0);
    starts.push(end);
    starts.dedup();
    starts
}

fn gpui_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '\u{00c0}'..='\u{024f}' | '\u{0400}'..='\u{04ff}' | '\u{1e00}'..='\u{1eff}' | '\u{0300}'..='\u{036f}' | '\u{0980}'..='\u{09ff}')
        || matches!(
            c,
            '-' | '_'
                | '.'
                | '\''
                | '’'
                | '‘'
                | '$'
                | '%'
                | '@'
                | '#'
                | '^'
                | '~'
                | ','
                | '='
                | ':'
                | ';'
                | '!'
                | ')'
                | ']'
                | '}'
                | '"'
                | '”'
                | '»'
                | '…'
                | '⋯'
                | '\u{202f}'
                | '\u{00a0}'
                | '\u{2011}'
        )
}

fn gpui_wrapped_lines(lines: Vec<LaidLine>, text: &str, width: f32) -> Vec<LaidLine> {
    let mut result = Vec::new();
    let mut added_y = 0.0;
    for line in lines {
        let mut boundaries = Vec::new();
        let mut first_non_whitespace = false;
        let mut candidate = None;
        let mut previous = '\0';
        let mut last_boundary = 0;
        let mut last_x = 0.0;
        for (index, glyph) in line.glyphs.iter().enumerate() {
            let Some(ch) = text
                .get(glyph.display_byte_start..)
                .and_then(|s| s.chars().next())
            else {
                continue;
            };
            if ch == '\n' {
                continue;
            }
            if ((gpui_word_char(ch) && previous == ' ') || !gpui_word_char(ch))
                && ch != ' '
                && first_non_whitespace
            {
                candidate = Some(index);
            }
            if ch != ' ' {
                first_non_whitespace = true;
            }
            let next_x = line.glyphs.get(index + 1).map_or(line.width, |next| next.x);
            if next_x - last_x > width && index > last_boundary {
                last_boundary = candidate.take().unwrap_or(index);
                last_x = line.glyphs[last_boundary].x;
                boundaries.push(last_boundary);
            }
            previous = ch;
        }
        let mut start = 0;
        for end in boundaries
            .iter()
            .copied()
            .chain(std::iter::once(line.glyphs.len()))
        {
            let origin = if start == 0 {
                0.0
            } else {
                line.glyphs.get(start).map_or(0.0, |glyph| glyph.x)
            };
            let end_x = line.glyphs.get(end).map_or(line.width, |glyph| glyph.x);
            let mut glyphs = line.glyphs[start..end].to_vec();
            for glyph in &mut glyphs {
                glyph.x -= origin;
            }
            let start_byte = glyphs
                .iter()
                .map(|glyph| glyph.display_byte_start)
                .min()
                .unwrap_or(line.display_range.start);
            let end_byte = glyphs
                .iter()
                .map(|glyph| glyph.display_byte_end)
                .max()
                .unwrap_or(line.display_range.end);
            let offset = added_y;
            result.push(LaidLine {
                paragraph: line.paragraph,
                display_range: start_byte..end_byte,
                y_top: line.y_top + offset,
                height: line.height,
                baseline_y: line.baseline_y + offset,
                width: end_x - origin,
                rtl: line.rtl,
                glyphs,
            });
            start = end;
            if end < line.glyphs.len() {
                added_y += line.height;
            }
        }
    }
    result
}

fn build_display(
    spans: &[StyledSpan],
    mask: Option<char>,
    line_mode: LineMode,
) -> (String, Vec<(usize, usize)>, usize) {
    if mask.is_none() {
        return build_plain_display(spans, line_mode);
    }
    let mut display = String::new();
    let mut map = Vec::new();
    let mut src_off = 0usize;
    for span in spans {
        for g in span.text.graphemes(true) {
            let g_bytes = g.len();
            if g == "\n" {
                if line_mode == LineMode::Single {
                    display.push(' ');
                } else {
                    display.push('\n');
                }
                src_off += g_bytes;
                continue;
            }
            if let Some(m) = mask {
                map.push((src_off, display.len()));
                display.push(m);
            } else {
                display.push_str(g);
            }
            src_off += g_bytes;
        }
    }
    (display, map, src_off)
}

fn build_plain_display(
    spans: &[StyledSpan],
    line_mode: LineMode,
) -> (String, Vec<(usize, usize)>, usize) {
    let total: usize = spans.iter().map(|span| span.text.len()).sum();
    let mut display = String::with_capacity(total);
    for span in spans {
        match (line_mode, span.text.contains('\n')) {
            (LineMode::Single, true) => {
                let mut previous = '\0';
                for character in span.text.chars() {
                    display.push(match (character, previous) {
                        ('\n', last) if last != '\r' => ' ',
                        (other, _) => other,
                    });
                    previous = character;
                }
            }
            _ => display.push_str(&span.text),
        }
    }
    (display, Vec::new(), total)
}

fn masked_span_text(text: &str, mask: Option<char>, line_mode: LineMode) -> String {
    if mask.is_none() && line_mode == LineMode::Multi {
        return text.to_string();
    }
    let mut out = String::new();
    for g in text.graphemes(true) {
        if g == "\n" {
            out.push(if line_mode == LineMode::Single {
                ' '
            } else {
                '\n'
            });
            continue;
        }
        if let Some(m) = mask {
            out.push(m);
        } else {
            out.push_str(g);
        }
    }
    out
}

fn display_range_to_source(map: &[(usize, usize)], d_start: usize, d_end: usize) -> (usize, usize) {
    if map.is_empty() {
        return (d_start, d_end);
    }
    let to_src = |d: usize| -> usize {
        match map.binary_search_by(|(_, dd)| dd.cmp(&d)) {
            Ok(i) => map[i].0,
            Err(0) => 0,
            Err(i) => map[i - 1].0,
        }
    };
    (to_src(d_start), to_src(d_end))
}

fn caret_x_in_line(
    line: &LaidLine,
    display_byte: usize,
    constrained_width: Option<f32>,
    text: &str,
) -> f32 {
    if line.glyphs.is_empty() {
        return empty_line_x(line, constrained_width);
    }
    caret_x_visual(line, display_byte, constrained_width, text)
}

fn caret_x_visual(
    line: &LaidLine,
    display_byte: usize,
    constrained_width: Option<f32>,
    text: &str,
) -> f32 {
    let line_start = line.display_range.start;
    let line_end = line.display_range.end;
    if line.glyphs.is_empty() {
        return empty_line_x(line, constrained_width);
    }
    if display_byte <= line_start {
        return if line.rtl {
            line_end_x(line)
        } else {
            line_start_x(line)
        };
    }
    if display_byte >= line_end {
        return if line.rtl {
            line_start_x(line)
        } else {
            line_end_x(line)
        };
    }
    edge_search(line, display_byte, text)
}

fn line_start_x(line: &LaidLine) -> f32 {
    line.glyphs
        .iter()
        .map(|g| g.x)
        .fold(f32::INFINITY, f32::min)
}

fn line_end_x(line: &LaidLine) -> f32 {
    line.glyphs
        .iter()
        .map(|g| g.x + g.w)
        .fold(f32::NEG_INFINITY, f32::max)
}

fn empty_line_x(line: &LaidLine, constrained_width: Option<f32>) -> f32 {
    let _ = constrained_width;
    let _ = line;
    0.0
}

fn glyph_positions(line: &LaidLine, text: &str) -> Vec<(f32, CaretPosition)> {
    let mut positions = Vec::new();
    for glyph in &line.glyphs {
        let rtl = glyph.bidi_level % 2 == 1;
        let (leading, trailing) = if rtl {
            (glyph.x + glyph.w, glyph.x)
        } else {
            (glyph.x, glyph.x + glyph.w)
        };
        positions.push((
            leading,
            CaretPosition {
                byte: glyph.display_byte_start,
                affinity: CaretAffinity::Downstream,
            },
        ));
        positions.push((
            trailing,
            CaretPosition {
                byte: glyph.display_byte_end,
                affinity: CaretAffinity::Upstream,
            },
        ));
        if let Some(cluster) = text.get(glyph.display_byte_start..glyph.display_byte_end) {
            let count = cluster.graphemes(true).count();
            for (index, (offset, _)) in cluster.grapheme_indices(true).enumerate().skip(1) {
                positions.push((
                    leading + (trailing - leading) * index as f32 / count as f32,
                    CaretPosition {
                        byte: glyph.display_byte_start + offset,
                        affinity: CaretAffinity::Downstream,
                    },
                ));
            }
        }
    }
    positions
}

fn glyph_edges(line: &LaidLine, text: &str) -> Vec<(f32, usize)> {
    let mut edges = Vec::with_capacity(line.glyphs.len() * 2);
    for glyph in &line.glyphs {
        let rtl = glyph.bidi_level % 2 == 1;
        let (leading, trailing) = if rtl {
            (glyph.x + glyph.w, glyph.x)
        } else {
            (glyph.x, glyph.x + glyph.w)
        };
        edges.push((leading, glyph.display_byte_start));
        edges.push((trailing, glyph.display_byte_end));
        if let Some(cluster) = text.get(glyph.display_byte_start..glyph.display_byte_end) {
            let count = cluster.graphemes(true).count();
            for (index, (offset, _)) in cluster.grapheme_indices(true).enumerate().skip(1) {
                let fraction = index as f32 / count as f32;
                edges.push((
                    leading + (trailing - leading) * fraction,
                    glyph.display_byte_start + offset,
                ));
            }
        }
    }
    edges
}

fn edge_search(line: &LaidLine, display_byte: usize, text: &str) -> f32 {
    let mut best_x = line.glyphs.first().map(|g| g.x).unwrap_or(0.0);
    let mut best_dist = usize::MAX;
    for (ex, eb) in glyph_edges(line, text) {
        let dist = display_byte.abs_diff(eb);
        if dist < best_dist {
            best_dist = dist;
            best_x = ex;
        }
    }
    best_x
}

fn hit_in_line(display_text: &str, line: &LaidLine, x: f32) -> usize {
    if line.glyphs.is_empty() {
        return line.display_range.start;
    }
    let mut best_byte = line.display_range.start;
    let mut best_dist = f32::INFINITY;
    for (ex, eb) in glyph_edges(line, display_text) {
        let dist = (x - ex).abs();
        if dist < best_dist {
            best_dist = dist;
            best_byte = eb;
        }
    }
    snap_to_boundary(display_text, best_byte, line.display_range.clone())
}

fn snap_to_boundary(text: &str, byte: usize, clamp: std::ops::Range<usize>) -> usize {
    let byte = byte.clamp(clamp.start, clamp.end.min(text.len()));
    if text.is_char_boundary(byte) {
        let mut b = byte;
        while b > clamp.start && !is_grapheme_boundary_at(text, b) {
            b -= 1;
        }
        b
    } else {
        let mut b = byte;
        while b > clamp.start && !text.is_char_boundary(b) {
            b -= 1;
        }
        while b > clamp.start && !is_grapheme_boundary_at(text, b) {
            b -= 1;
        }
        b
    }
}

fn is_grapheme_boundary_at(text: &str, byte: usize) -> bool {
    if byte == 0 || byte == text.len() {
        return true;
    }
    text.grapheme_indices(true).any(|(i, _)| i == byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::TextStyle;

    fn manager() -> FontManager {
        FontManager::new()
    }

    fn style() -> TextStyle {
        TextStyle::default()
    }

    #[test]
    fn empty_layout_has_one_line_height() {
        let mut fonts = manager();
        let mut layout = TextLayout::new();
        layout.set_text("", &style());
        let laid = layout.layout(&mut fonts);
        assert_eq!(laid.lines.len(), 1);
        assert!(laid.size.1 > 0.0);
        assert!(layout.caret(0).is_some());
    }

    #[cfg(windows)]
    #[test]
    fn ascii_measures_positive_width() {
        let mut fonts = manager();
        let mut layout = TextLayout::new();
        layout.set_text("hello", &style());
        let laid = layout.layout(&mut fonts);
        assert!(laid.size.0 > 0.0);
        assert!(laid.size.1 > 0.0);
        assert!(!laid.lines[0].glyphs.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn combining_marks_shape() {
        let mut fonts = manager();
        let mut layout = TextLayout::new();
        layout.set_text("é cottage", &style());
        let laid = layout.layout(&mut fonts);
        assert!(!laid.lines[0].glyphs.is_empty());
        let c0 = layout.caret(0).unwrap();
        let c3 = layout.caret(3).unwrap();
        assert!(c3.x >= c0.x);
    }

    #[cfg(windows)]
    #[test]
    fn emoji_shapes() {
        let mut fonts = manager();
        let mut layout = TextLayout::new();
        layout.set_text("hi 🙂", &style());
        let laid = layout.layout(&mut fonts);
        assert!(laid.size.0 > 0.0);
    }

    #[test]
    fn multiline_height_grows() {
        let mut fonts = manager();
        let mut one = TextLayout::new();
        one.set_line_mode(LineMode::Multi);
        one.set_text("one", &style());
        let h1 = one.layout(&mut fonts).size.1;
        let mut two = TextLayout::new();
        two.set_line_mode(LineMode::Multi);
        two.set_text("one\ntwo", &style());
        let laid = two.layout(&mut fonts);
        assert!(laid.size.1 > h1);
        assert!(laid.lines.len() >= 2);
    }

    #[cfg(windows)]
    #[test]
    fn wrap_constrains_width() {
        let mut fonts = manager();
        let mut natural = TextLayout::new();
        natural.set_line_mode(LineMode::Multi);
        natural.set_text("this is a fairly long line of text for wrapping", &style());
        let natural_w = natural.layout(&mut fonts).size.0;
        assert!(natural_w > 200.0);

        let mut layout = TextLayout::new();
        layout.set_line_mode(LineMode::Multi);
        layout.set_wrap(WrapMode::WordOrGlyph);
        layout.set_max_width(Some(60.0));
        layout.set_text("this is a fairly long line of text for wrapping", &style());
        let laid = layout.layout(&mut fonts);
        assert!(laid.size.0 < natural_w / 2.0, "width was {}", laid.size.0);
        assert!(laid.lines.len() > 1);
    }

    #[cfg(windows)]
    #[test]
    fn word_wrap_keeps_long_word_intact() {
        let mut fonts = manager();
        let mut layout = TextLayout::new();
        layout.set_line_mode(LineMode::Multi);
        layout.set_wrap(WrapMode::Word);
        layout.set_max_width(Some(60.0));
        layout.set_text("this is a fairly long line of text for wrapping", &style());
        let laid = layout.layout(&mut fonts);
        assert!(laid.size.0 > 60.0);
        assert!(laid.lines.len() > 1);
    }

    #[test]
    fn rtl_paragraph_detected() {
        let mut fonts = manager();
        let mut layout = TextLayout::new();
        layout.set_text("مرحبا بالعالم", &style());
        let laid = layout.layout(&mut fonts);
        assert!(!laid.lines.is_empty());
        assert!(laid.lines.iter().any(|l| l.rtl));
    }

    #[cfg(windows)]
    #[test]
    fn scale_changes_size_and_invalidates() {
        let mut fonts = manager();
        let mut layout = TextLayout::new();
        layout.set_text("hello", &style());
        let w1 = layout.layout(&mut fonts).size.0;
        layout.set_scale(2.0);
        let w2 = layout.layout(&mut fonts).size.0;
        assert!(w2 > w1 * 1.5);
    }

    #[cfg(windows)]
    #[test]
    fn masked_layout_maps_source_coords() {
        let mut fonts = manager();
        let mut layout = TextLayout::new();
        layout.set_mask(Some('•'));
        layout.set_text("secret", &style());
        let laid = layout.layout(&mut fonts);
        assert!(!laid.lines.is_empty());
        assert_eq!(laid.lines[0].glyphs.len(), 6);
        let c = layout.caret(6).unwrap();
        assert!(c.x > 0.0);
    }

    #[test]
    fn shared_cache_evicts_oldest_and_disposes_on_clear() {
        use super::SharedLayoutCache;
        let mut fonts = manager();
        let mut cache = SharedLayoutCache::new(2);
        for (i, word) in ["alpha", "beta", "gamma"].iter().enumerate() {
            let mut t = TextLayout::new();
            t.set_text(word, &style());
            t.layout_shared(&mut fonts, &mut cache);
            assert_eq!(cache.len(), (i + 1).min(2), "capacity bound holds");
        }
        assert_eq!(cache.evictions, 1, "oldest entry evicted");
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn font_load_invalidates_shared_cache() {
        use super::SharedLayoutCache;
        let mut fonts = manager();
        let mut cache = SharedLayoutCache::new(8);
        let mut t = TextLayout::new();
        t.set_text("abc", &style());
        t.layout_shared(&mut fonts, &mut cache);
        assert_eq!(cache.misses, 1);
        let mut t2 = TextLayout::new();
        t2.set_text("abc", &style());
        t2.layout_shared(&mut fonts, &mut cache);
        assert_eq!(cache.hits, 1);
        fonts
            .load_font_file("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
            .expect("test host needs DejaVuSans");
        let mut t3 = TextLayout::new();
        t3.set_text("abc", &style());
        t3.layout_shared(&mut fonts, &mut cache);
        assert_eq!(cache.misses, 2, "font load reshapes identical text");
    }

    #[cfg(windows)]
    #[test]
    fn hit_roundtrip_ltr() {
        let mut fonts = manager();
        let mut layout = TextLayout::new();
        layout.set_text("hello", &style());
        layout.layout(&mut fonts);
        let idx = layout.hit(-100.0, 5.0).unwrap();
        assert_eq!(idx, 0);
        let c_end = layout.caret(5).unwrap();
        let idx2 = layout.hit(c_end.x + 100.0, 5.0).unwrap();
        assert_eq!(idx2, 5);
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;

    fn by_graphemes(spans: &[StyledSpan], line_mode: LineMode) -> String {
        let mut display = String::new();
        for span in spans {
            for g in span.text.graphemes(true) {
                match (g == "\n", line_mode) {
                    (true, LineMode::Single) => display.push(' '),
                    _ => display.push_str(g),
                }
            }
        }
        display
    }

    #[test]
    fn plain_display_matches_grapheme_segmentation() {
        let samples = [
            "plain",
            "line one\nline two\n",
            "windows\r\nendings\r\nand\nmixed\r",
            "e\u{301}\nclusters 👨‍👩‍👧\n",
            "",
            "\n\n",
        ];
        for mode in [LineMode::Single, LineMode::Multi] {
            let spans: Vec<StyledSpan> = samples
                .iter()
                .map(|text| StyledSpan::plain(*text))
                .collect();
            let (display, map, length) = build_display(&spans, None, mode);
            assert_eq!(display, by_graphemes(&spans, mode));
            assert!(map.is_empty());
            assert_eq!(
                length,
                spans.iter().map(|span| span.text.len()).sum::<usize>()
            );
        }
    }

    #[test]
    fn masking_still_goes_through_graphemes() {
        let spans = vec![StyledSpan::plain("ab\ncd")];
        let (display, map, _) = build_display(&spans, Some('*'), LineMode::Multi);
        assert_eq!(display, "**\n**");
        assert_eq!(map.len(), 4);
    }
}
