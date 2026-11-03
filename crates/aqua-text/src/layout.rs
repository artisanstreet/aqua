use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use unicode_segmentation::UnicodeSegmentation;

use crate::font::FontManager;
use crate::style::{LineMode, StyledSpan, TextAlign, TextStyle, WrapMode};

#[derive(Clone, Debug)]
pub struct LaidGlyph {
    pub display_byte_start: usize,
    pub display_byte_end: usize,
    pub src_byte_start: usize,
    pub src_byte_end: usize,
    pub font_id: fontdb::ID,
    pub glyph_id: u16,
    pub font_size_px: f32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub bidi_level: u8,
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

pub struct TextLayout {
    spans: Vec<StyledSpan>,
    wrap: WrapMode,
    align: TextAlign,
    line_mode: LineMode,
    max_width: Option<f32>,
    scale: f32,
    mask: Option<char>,
    buffer: Option<cosmic_text::Buffer>,
    last_key: Option<InputKey>,
    cached: Option<Arc<LaidOut>>,
    fonts_rev_seen: u64,
    display_text: String,
    para_starts: Vec<usize>,
    cluster_map: Vec<(usize, usize)>,
    display_len: usize,
    source_len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct InputKey {
    display_text: String,
    style_key: String,
    wrap: WrapMode,
    align: TextAlign,
    line_mode: LineMode,
    width_bits: Option<u32>,
    scale_bits: u32,
    mask: Option<char>,
    fonts_rev: u64,
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
            mask: None,
            buffer: None,
            last_key: None,
            cached: None,
            fonts_rev_seen: 0,
            display_text: String::new(),
            para_starts: vec![0],
            cluster_map: Vec::new(),
            display_len: 0,
            source_len: 0,
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

    pub fn set_mask(&mut self, mask: Option<char>) {
        if self.mask != mask {
            self.mask = mask;
            self.last_key = None;
        }
    }

    pub fn source_len(&self) -> usize {
        self.source_len
    }

    pub fn scale(&self) -> f32 {
        self.scale
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
        let mut style_key = String::new();
        for s in &self.spans {
            style_key.push_str(&format!("{}:{:?}:", s.text.len(), s.color_srgb));
            style_key.push_str(&s.style.cache_key());
            style_key.push(';');
        }
        InputKey {
            display_text: display.to_string(),
            style_key,
            wrap: self.wrap,
            align: self.align,
            line_mode: self.line_mode,
            width_bits: self.max_width.map(|w| (w * self.scale).to_bits()),
            scale_bits: self.scale.to_bits(),
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
        let buffer = self.buffer.get_or_insert_with(|| {
            let metrics = first_metrics(&self.spans, self.scale);
            cosmic_text::Buffer::new(fonts.font_system_mut(), metrics)
        });
        if self.line_mode == LineMode::Single {
            buffer.set_wrap(cosmic_text::Wrap::None);
        } else {
            buffer.set_wrap(self.wrap.to_cosmic());
        }
        let width_px = self.max_width.map(|w| (w * self.scale).max(1.0));
        buffer.set_size(width_px, None);
        let metrics = first_metrics(&self.spans, self.scale);
        buffer.set_metrics(metrics);

        {
            let default_attrs = self
                .spans
                .first()
                .map(|s| s.style.to_cosmic_in())
                .unwrap_or_else(cosmic_text::Attrs::new);
            let mut texts: Vec<String> = Vec::with_capacity(self.spans.len());
            let mut attrs_vec: Vec<cosmic_text::Attrs<'_>> = Vec::with_capacity(self.spans.len());
            for s in &self.spans {
                let mut attrs = s.style.to_cosmic_in();
                if let Some([r, g, b, a]) = s.color_srgb {
                    attrs = attrs.color(cosmic_text::Color::rgba(r, g, b, a));
                }
                attrs = attrs.metrics(cosmic_text::Metrics::new(
                    (s.style.size_px * self.scale).max(1.0),
                    (s.style.line_height_px * self.scale).max(1.0),
                ));
                texts.push(masked_span_text(&s.text, self.mask, self.line_mode));
                attrs_vec.push(attrs);
            }
            let refs: Vec<(&str, cosmic_text::Attrs<'_>)> =
                texts.iter().map(String::as_str).zip(attrs_vec).collect();
            buffer.set_rich_text(
                refs,
                &default_attrs,
                cosmic_text::Shaping::Advanced,
                self.align.to_cosmic(),
            );
        }
        buffer.shape_until_scroll(fonts.font_system_mut(), false);

        let mut para_starts = Vec::with_capacity(buffer.lines.len() + 1);
        let mut off = 0usize;
        for line in &buffer.lines {
            para_starts.push(off);
            off += line.text().len() + 1;
        }
        off = off.min(display.len());
        para_starts.push(off);

        let mut lines = Vec::new();
        let mut max_w = 0.0f32;
        let mut total_h = 0.0f32;
        let mut first_baseline_y = None;
        for run in buffer.layout_runs() {
            let para = run.line_i;
            let para_start = para_starts.get(para).copied().unwrap_or(0);
            let mut glyphs = Vec::with_capacity(run.glyphs.len());
            for g in run.glyphs {
                let d_start = para_start + g.start;
                let d_end = para_start + g.end;
                let (s_start, s_end) = display_range_to_source(&cluster_map, d_start, d_end);
                glyphs.push(LaidGlyph {
                    display_byte_start: d_start,
                    display_byte_end: d_end,
                    src_byte_start: s_start,
                    src_byte_end: s_end,
                    font_id: g.font_id,
                    glyph_id: g.glyph_id,
                    font_size_px: g.font_size,
                    x: g.x,
                    y: g.y,
                    w: g.w,
                    bidi_level: g.level.number(),
                });
            }
            if first_baseline_y.is_none() {
                first_baseline_y = Some(run.line_y);
            }
            max_w = max_w.max(run.line_w);
            total_h = run.line_top + run.line_height;
            let start = run.glyphs.iter().map(|g| g.start).min().unwrap_or(0);
            let end = run.glyphs.iter().map(|g| g.end).max().unwrap_or(0);
            let display_range = para_start + start..para_start + end;
            lines.push(LaidLine {
                paragraph: para,
                display_range,
                y_top: run.line_top,
                height: run.line_height,
                baseline_y: run.line_y,
                width: run.line_w,
                rtl: run.rtl,
                glyphs,
            });
        }

        if lines.is_empty() {
            let h = metrics.line_height.max(1.0);
            lines.push(LaidLine {
                paragraph: 0,
                display_range: 0..0,
                y_top: 0.0,
                height: h,
                baseline_y: h,
                width: 0.0,
                rtl: false,
                glyphs: Vec::new(),
            });
            total_h = h;
            first_baseline_y = Some(h);
        }

        let laid = Arc::new(LaidOut {
            size: (max_w, total_h),
            constrained_width: width_px,
            lines,
            first_baseline_y,
        });
        self.display_text = display;
        self.display_len = self.display_text.len();
        self.source_len = source_len;
        self.para_starts = para_starts;
        self.cluster_map = cluster_map;
        self.last_key = Some(key);
        self.fonts_rev_seen = fonts_rev;
        self.cached = Some(Arc::clone(&laid));
        laid
    }

    fn to_display(&self, src: usize) -> usize {
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
            (s.style.line_height_px * self.scale).max(1.0)
        } else {
            16.0 * self.scale
        }
    }

    pub fn buffer(&self) -> Option<&cosmic_text::Buffer> {
        self.buffer.as_ref()
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

fn first_metrics(spans: &[StyledSpan], scale: f32) -> cosmic_text::Metrics {
    match spans.first() {
        Some(s) => cosmic_text::Metrics::new(
            (s.style.size_px * scale).max(1.0),
            (s.style.line_height_px * scale).max(1.0),
        ),
        None => cosmic_text::Metrics::new(16.0 * scale, 22.0 * scale),
    }
}

fn build_display(
    spans: &[StyledSpan],
    mask: Option<char>,
    line_mode: LineMode,
) -> (String, Vec<(usize, usize)>, usize) {
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
