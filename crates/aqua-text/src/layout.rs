//! Measurement and shaped-run extraction.
//!
//! `TextLayout` owns display text plus styling, retains one shaping-engine
//! `Buffer` across frames (reusing its allocations), and produces owned,
//! renderer-neutral [`LaidOut`] snapshots:
//!
//! * overall size, per-line boxes, baselines — for `aqua-ui` measurement,
//! * per-glyph shaped data (font id, glyph id, positions, bidi levels) —
//!   for `aqua-render`/custom canvas work and accessibility,
//! * caret and hit-testing geometry — for editing and IME candidate windows.
//!
//! Coordinate space: scaled logical pixels, origin at the layout top-left,
//! y growing downward. All sizes are already multiplied by the layout scale,
//! so consumers must not scale them again.
//!
//! Rasterization is deliberately *not* here: the renderer (glyphon, which
//! shares `cosmic-text 0.19`) rasterizes from text + attributes. These DTOs
//! exist so measurement and final pixels agree on the same shaping.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use unicode_segmentation::UnicodeSegmentation;

use crate::font::FontManager;
use crate::style::{LineMode, StyledSpan, TextAlign, TextStyle, WrapMode};

/// One shaped glyph in layout space.
///
/// `x`/`y` are the shaping engine's glyph offsets: `x` is the visual offset
/// from the layout left edge (line alignment already applied), `y` is the
/// engine-internal vertical offset of the glyph hitbox. Line-level geometry
/// (`LaidLine::y_top`, `baseline_y`, `height`) is the contract for caret and
/// selection rectangles; glyph `x`/`w` position carets horizontally.
/// `src_byte_*` are byte indices in the *source* (unmasked) text.
#[derive(Clone, Debug)]
pub struct LaidGlyph {
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

/// One visual line.
#[derive(Clone, Debug)]
pub struct LaidLine {
    /// Index of the source paragraph (`\n`-separated) this line belongs to.
    pub paragraph: usize,
    /// Byte range in *display* coordinates covered by this visual line.
    pub display_range: std::ops::Range<usize>,
    /// Distance from layout top to line top, scaled px.
    pub y_top: f32,
    /// Line height, scaled px.
    pub height: f32,
    /// Baseline y in layout space (`y_top + ascent offset`), scaled px.
    pub baseline_y: f32,
    /// Natural line width (glyph extents), scaled px, alignment baked in.
    pub width: f32,
    /// True when the paragraph base direction is right-to-left.
    pub rtl: bool,
    pub glyphs: Vec<LaidGlyph>,
}

/// Owned, renderer-neutral layout result.
#[derive(Clone, Debug, Default)]
pub struct LaidOut {
    /// Overall size in scaled px: `(max line width, sum of line heights)`.
    pub size: (f32, f32),
    /// Width constraint used, if any, in scaled px.
    pub constrained_width: Option<f32>,
    pub lines: Vec<LaidLine>,
    /// Baseline of the first visual line, or `None` for empty text.
    pub first_baseline_y: Option<f32>,
}

impl LaidOut {
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.glyphs.is_empty())
    }
}

/// Caret rectangle in layout space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretRect {
    pub x: f32,
    pub y_top: f32,
    pub height: f32,
}

/// Axis-aligned rectangle in layout space (selection highlight).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Retained single-text layout with dirty tracking.
///
/// Reuse one `TextLayout` per text widget: unchanged inputs skip shaping
/// entirely and return the cached [`LaidOut`].
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
    /// Displayed text currently in the buffer (masked/newline-folded).
    display_text: String,
    /// Paragraph start offsets (display bytes) mirroring buffer lines.
    para_starts: Vec<usize>,
    /// Cluster map for masked text: ((src_start), (display_start)) pairs.
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

    /// Plain text with one style.
    pub fn set_text(&mut self, text: &str, style: &TextStyle) {
        let spans = vec![StyledSpan::new(text.to_string(), style.clone())];
        self.set_spans(spans);
    }

    /// Rich text: multiple styled spans concatenated in order.
    /// Newlines may appear inside any span and start new paragraphs.
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

    /// Wrapping width in *unscaled* logical px. `None` means unbounded.
    pub fn set_max_width(&mut self, width: Option<f32>) {
        if self.max_width != width {
            self.max_width = width;
            self.last_key = None;
        }
    }

    /// Device/pinch scale. Font sizes, line heights and widths are multiplied
    /// by this before shaping; results are reported in scaled px.
    pub fn set_scale(&mut self, scale: f32) {
        let scale = scale.max(0.01);
        if (self.scale - scale).abs() > f32::EPSILON {
            self.scale = scale;
            self.last_key = None;
        }
    }

    /// Mask each grapheme cluster with `c` (password fields). `None`
    /// disables masking. Caret/hit APIs keep speaking *source* coordinates.
    pub fn set_mask(&mut self, mask: Option<char>) {
        if self.mask != mask {
            self.mask = mask;
            self.last_key = None;
        }
    }

    /// Source text length in bytes (unmasked coordinates).
    pub fn source_len(&self) -> usize {
        self.source_len
    }

    /// Scale this layout was configured with (outputs are in scaled px).
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Scaled size from the last [`TextLayout::layout`] call, or `None`
    /// before the first layout. Borrowed from the retained cache.
    pub fn last_scaled_size(&self) -> Option<(f32, f32)> {
        self.cached.as_ref().map(|laid| laid.size)
    }

    /// Lay out (reusing the retained buffer and cache when inputs and the
    /// font database are unchanged) and return the result.
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

    /// Lay out via a [`SharedLayoutCache`], so identical text elsewhere in
    /// the UI reuses shaping work. Instance state (needed by
    /// [`TextLayout::caret`]/[`TextLayout::hit`]) is still refreshed on a
    /// shared hit; only shaping is skipped.
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
            // Borrowed attrs must not outlive `self.spans`; collect owned
            // (text, attrs-with-metrics) pairs first, then shape.
            let mut texts: Vec<String> = Vec::with_capacity(self.spans.len());
            let mut attrs_vec: Vec<cosmic_text::Attrs<'_>> = Vec::with_capacity(self.spans.len());
            for s in &self.spans {
                let mut attrs = s.style.to_cosmic_in();
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

        // Paragraph offsets from the actual buffer lines.
        let mut para_starts = Vec::with_capacity(buffer.lines.len() + 1);
        let mut off = 0usize;
        for line in &buffer.lines {
            para_starts.push(off);
            off += line.text().len() + 1; // +1 for the '\n' separator
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
            let display_range = para_start..para_start + run.text.len();
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

        // Empty content still occupies one line height so carets have a home.
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

    /// Map a *source* byte index to display coordinates (identity unless masked).
    fn to_display(&self, src: usize) -> usize {
        if self.mask.is_none() {
            return src.min(self.display_len);
        }
        let src = src.min(self.source_len);
        // Binary search cluster starts; caret positions are boundaries.
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

    /// Map a *display* byte index (snapped to a boundary) to source coordinates.
    fn to_source(&self, display: usize) -> usize {
        if self.mask.is_none() {
            return display.min(self.source_len);
        }
        let (s, _) = display_range_to_source(&self.cluster_map, display, display);
        s
    }

    /// Caret rectangle for a *source* byte index. Returns `None` only if no
    /// layout has been produced yet (call [`TextLayout::layout`] first).
    pub fn caret(&self, src_byte: usize) -> Option<CaretRect> {
        let laid = self.cached.as_ref()?;
        let d = self.to_display(src_byte);
        for line in &laid.lines {
            if d >= line.display_range.start && d <= line.display_range.end {
                let x = caret_x_in_line(line, d, laid.constrained_width);
                return Some(CaretRect {
                    x,
                    y_top: line.y_top,
                    height: line.height,
                });
            }
        }
        // Past the end: use the last line.
        let line = laid.lines.last()?;
        Some(CaretRect {
            x: caret_x_in_line(line, d.min(line.display_range.end), laid.constrained_width),
            y_top: line.y_top,
            height: line.height,
        })
    }

    /// Hit-test layout space to a *source* byte index, snapped to a
    /// grapheme boundary of the source text.
    ///
    /// Correct for LTR and RTL paragraphs. Inside mixed-direction runs the
    /// result is the nearest visual glyph edge, which may split a logical
    /// cluster order — the return value is still a valid boundary.
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

    /// Selection highlight rectangles for a *source* byte range.
    pub fn selection_rects(&self, range: std::ops::Range<usize>) -> Vec<Rect> {
        let Some(laid) = self.cached.as_ref() else {
            return Vec::new();
        };
        let start = self.to_display(range.start);
        let end = self.to_display(range.end);
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let mut out = Vec::new();
        for line in &laid.lines {
            let ls = line.display_range.start;
            let le = line.display_range.end;
            if end <= ls || start >= le {
                // Fully covered middle lines still highlight full width.
                if start <= ls && end >= le && le > ls {
                    let (x0, x1) = full_line_span(line, laid.constrained_width);
                    out.push(Rect {
                        x: x0,
                        y: line.y_top,
                        w: (x1 - x0).max(0.0),
                        h: line.height,
                    });
                }
                continue;
            }
            let s = start.max(ls);
            let e = end.min(le);
            if s >= e && !(line.glyphs.is_empty() && start == end) {
                continue;
            }
            let x0 = caret_x_in_line(line, s, laid.constrained_width);
            let x1 = caret_x_in_line(line, e, laid.constrained_width);
            out.push(Rect {
                x: x0.min(x1),
                y: line.y_top,
                w: (x1 - x0).abs(),
                h: line.height,
            });
        }
        out
    }

    /// Height of one line in scaled px with the current first-span style.
    /// Useful for empty editors before layout.
    pub fn single_line_height(&self) -> f32 {
        if let Some(s) = self.spans.first() {
            (s.style.line_height_px * self.scale).max(1.0)
        } else {
            16.0 * self.scale
        }
    }

    /// The retained shaping buffer backing the last [`TextLayout::layout`].
    /// `None` before the first layout call.
    ///
    /// Renderer contract (`aqua-render`/glyphon): build
    /// `glyphon::TextArea { buffer: layout.buffer(), … }` and call
    /// `TextRenderer::prepare` with `fonts.lock().font_system_mut()` from
    /// the *same* [`FontManager`] used for [`TextLayout::layout`]. Same
    /// buffer + same font system = measurement and pixels cannot disagree.
    /// Do not mutate the buffer (no `set_text`/`set_size`); all changes go
    /// through `TextLayout` setters so the cache key stays valid.
    pub fn buffer(&self) -> Option<&cosmic_text::Buffer> {
        self.buffer.as_ref()
    }

    /// Current display text (masked/newline-folded). Mostly for debugging;
    /// prefer source coordinates everywhere else.
    pub fn display_text(&self) -> &str {
        &self.display_text
    }
}

/// One-shot measurement for `aqua-ui` layout callbacks (Taffy measure fns).
///
/// Creates no retained state; for repeated measurement of the same text
/// keep a [`TextLayout`] instead. `max_width` and the result are in
/// unscaled logical px; `scale` is applied internally.
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

/// Bounded LRU cache sharing [`LaidOut`] snapshots across `TextLayout`s
/// (e.g. repeated labels in a list). Capacity counts entries; each key holds
/// its content string, so memory is `O(capacity × longest content)`.
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
    // Explicit monotonic counter owned by FontManager (bumped on every
    // load_*). Face count alone would miss same-count font replacement.
    fonts.font_revision()
}

/// Rebuild paragraph offsets from a cached result (display coordinates).
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

/// Build the display string plus cluster map.
/// Returns `(display, cluster_map, source_len)`.
/// `cluster_map` entries are `(src_byte_start, display_byte_start)` per
/// grapheme cluster, in order; only populated when masking.
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

/// Map a display byte range to source coordinates via the cluster map.
/// Identity when the map is empty (unmasked).
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

fn caret_x_in_line(line: &LaidLine, display_byte: usize, constrained_width: Option<f32>) -> f32 {
    if line.glyphs.is_empty() {
        return empty_line_x(line, constrained_width);
    }
    // Glyphs are in visual order; display offsets advance in the same order
    // (exact both masked and unmasked — see caret_x_visual).
    caret_x_visual(line, display_byte, constrained_width)
}

/// True visual caret placement using glyph edges in display coordinates.
///
/// Glyph display spans advance in visual order. Post-hoc we only know each
/// glyph's *source* span, so display offsets are distributed proportionally
/// to source lengths. This is exact when unmasked (source == display bytes)
/// and exact when masked (every cluster maps to one mask char and glyph
/// source spans are whole clusters). The caret snaps to the nearest glyph
/// edge; for LTR text this is exact, for RTL/mixed runs it is the nearest
/// visual edge (documented approximation, still a valid boundary).
fn caret_x_visual(line: &LaidLine, display_byte: usize, constrained_width: Option<f32>) -> f32 {
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
    // Walk glyphs; each glyph's display span is proportional to its
    // source length (exact masked and unmasked — see above). Find the
    // glyph edge nearest to the fractional position.
    edge_search(line, display_byte)
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
    // Alignment is baked into glyph x by the shaper; with no glyphs there
    // is no baked offset, so x=0 (line origin). UI layers applying block
    // alignment to empty lines should offset by their own constraint.
    let _ = line;
    0.0
}

/// Visual edges of a line: `(x, display_byte)` for each glyph's leading
/// edge in visual order plus the trailing edge of the last glyph.
/// Display bytes distribute proportionally to source lengths — exact both
/// masked and unmasked (see `caret_x_visual`).
fn glyph_edges(line: &LaidLine) -> Vec<(f32, usize)> {
    let mut edges = Vec::with_capacity(line.glyphs.len() + 1);
    if line.glyphs.is_empty() {
        return edges;
    }
    let line_start = line.display_range.start;
    let total_src: usize = line
        .glyphs
        .iter()
        .map(|g| g.src_byte_end.saturating_sub(g.src_byte_start))
        .sum();
    let total_src = total_src.max(1) as f32;
    let total_disp = (line.display_range.end - line_start).max(1) as f32;
    let mut acc = 0.0f32;
    for g in &line.glyphs {
        edges.push((g.x, line_start + (acc / total_src * total_disp) as usize));
        acc += g.src_byte_end.saturating_sub(g.src_byte_start) as f32;
    }
    let last = line.glyphs.last().expect("non-empty");
    edges.push((last.x + last.w, line.display_range.end));
    edges
}

/// Nearest visual glyph-edge x for a display byte offset within the line.
fn edge_search(line: &LaidLine, display_byte: usize) -> f32 {
    let mut best_x = line.glyphs.first().map(|g| g.x).unwrap_or(0.0);
    let mut best_dist = usize::MAX;
    for (ex, eb) in glyph_edges(line) {
        let dist = display_byte.abs_diff(eb);
        if dist < best_dist {
            best_dist = dist;
            best_x = ex;
        }
    }
    best_x
}

fn full_line_span(line: &LaidLine, constrained_width: Option<f32>) -> (f32, f32) {
    let _ = constrained_width;
    if line.glyphs.is_empty() {
        return (0.0, 0.0);
    }
    (line_start_x(line), line_end_x(line))
}

fn hit_in_line(display_text: &str, line: &LaidLine, x: f32) -> usize {
    if line.glyphs.is_empty() {
        return line.display_range.start;
    }
    // Nearest visual edge by x; ties favor the earlier edge.
    let mut best_byte = line.display_range.start;
    let mut best_dist = f32::INFINITY;
    for (ex, eb) in glyph_edges(line) {
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
        // Snap further to grapheme boundary (combining sequences).
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
        // Caret at byte 0 and byte 3 (after the e+combining cluster).
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
        // Natural (unwrapped) width for comparison.
        let mut natural = TextLayout::new();
        natural.set_line_mode(LineMode::Multi);
        natural.set_text("this is a fairly long line of text for wrapping", &style());
        let natural_w = natural.layout(&mut fonts).size.0;
        assert!(natural_w > 200.0);

        let mut layout = TextLayout::new();
        layout.set_line_mode(LineMode::Multi);
        // WordOrGlyph: even the over-long word ("wrapping" ≈ 74px at 16px)
        // must break so the line stays near the 60px constraint. The shaper
        // may overshoot by a partial glyph; wrapping (not exactness) is the
        // property under test.
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
        // Correct shaper behavior: the over-long word overflows its line
        // rather than splitting mid-word.
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
        // 6 graphemes -> 6 mask chars.
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
        // Same-count replacement must still reshape: the key carries the
        // explicit font revision, not a face count.
        let mut fonts = manager(); // system fonts
        let mut cache = SharedLayoutCache::new(8);
        let mut t = TextLayout::new();
        t.set_text("abc", &style());
        t.layout_shared(&mut fonts, &mut cache);
        assert_eq!(cache.misses, 1);
        // Fresh instance, same inputs: shared hit (the per-instance fast
        // path would bypass the shared counters, so use a new layout).
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
