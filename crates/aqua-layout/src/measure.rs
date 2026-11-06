use aqua_core::Size;
use aqua_text::{measure_text, LineMode, SharedFonts, StyledSpan, TextLayout, TextStyle, WrapMode};

pub trait TextMeasurer {
    fn measure_text(
        &self,
        text: &str,
        font_size: f32,
        max_width: Option<f32>,
        multiline: bool,
    ) -> Size;
    fn measure_styled(
        &self,
        text: &str,
        style: &TextStyle,
        spans: &[StyledSpan],
        max_width: Option<f32>,
        multiline: bool,
    ) -> Size {
        let _ = spans;
        self.measure_text(text, style.size_px, max_width, multiline)
    }
    fn memo_epoch(&self) -> Option<u64> {
        None
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NaiveMeasurer;

impl TextMeasurer for NaiveMeasurer {
    fn measure_text(
        &self,
        text: &str,
        font_size: f32,
        max_width: Option<f32>,
        multiline: bool,
    ) -> Size {
        const ADVANCE: f32 = 0.6;
        const LINE: f32 = 1.2;
        if text.is_empty() {
            return Size::new(0.0, font_size * LINE);
        }
        if !multiline {
            let longest = text
                .split('\n')
                .map(|l| l.chars().count() as f32)
                .fold(0.0f32, f32::max);
            let lines = text.split('\n').count() as f32;
            return Size::new(longest * ADVANCE * font_size, lines * LINE * font_size);
        }
        match max_width {
            Some(w) => {
                let per_line = (w / (ADVANCE * font_size)).floor().max(1.0);
                let mut rows = 0.0f32;
                let mut width = 0.0f32;
                for line in text.split('\n') {
                    let n = line.chars().count() as f32;
                    rows += (n / per_line).ceil().max(1.0);
                    width = width.max(n.min(per_line) * ADVANCE * font_size);
                }
                Size::new(width.min(w), rows * LINE * font_size)
            }
            _ => {
                let longest = text
                    .split('\n')
                    .map(|l| l.chars().count() as f32)
                    .fold(0.0f32, f32::max);
                let lines = text.split('\n').count() as f32;
                Size::new(longest * ADVANCE * font_size, lines * LINE * font_size)
            }
        }
    }
}

#[derive(Clone)]
pub struct ShapedMeasurer {
    pub fonts: SharedFonts,
    pub scale: f32,
}

impl TextMeasurer for ShapedMeasurer {
    fn measure_text(
        &self,
        text: &str,
        font_size: f32,
        max_width: Option<f32>,
        multiline: bool,
    ) -> Size {
        let style = TextStyle {
            size_px: font_size,
            line_height_px: font_size * 1.2,
            ..TextStyle::default()
        };
        let (w, h) = measure_text(
            &mut self.fonts.lock(),
            text,
            &style,
            if multiline { max_width } else { None },
            self.scale,
        );
        Size::new(w, h)
    }
    fn measure_styled(
        &self,
        text: &str,
        style: &TextStyle,
        spans: &[StyledSpan],
        max_width: Option<f32>,
        multiline: bool,
    ) -> Size {
        let mut layout = TextLayout::new();
        match spans.is_empty() {
            true => layout.set_text(text, style),
            false => layout.set_spans(spans.to_vec()),
        }
        layout.set_line_mode(if multiline {
            LineMode::Multi
        } else {
            LineMode::Single
        });
        layout.set_wrap(if multiline {
            WrapMode::Word
        } else {
            WrapMode::None
        });
        layout.set_max_width(if multiline { max_width } else { None });
        layout.set_scale(self.scale);
        let laid = layout.layout(&mut self.fonts.lock());
        let scale = self.scale.max(0.01);
        Size::new(laid.size.0 / scale, laid.size.1 / scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naive_single_line_scales_with_len() {
        let m = NaiveMeasurer;
        let a = m.measure_text("hi", 16.0, None, false);
        let b = m.measure_text("hello world", 16.0, None, false);
        assert!(b.w > a.w);
        assert_eq!(a.h, b.h);
    }

    #[test]
    fn naive_wrap_bounds_width() {
        let m = NaiveMeasurer;
        let s = m.measure_text("a fairly long line that must wrap", 10.0, Some(30.0), true);
        assert!(s.w <= 30.0 + f32::EPSILON);
        assert!(s.h > 10.0 * 1.2);
    }

    #[test]
    fn naive_empty_has_line_height() {
        let m = NaiveMeasurer;
        let s = m.measure_text("", 16.0, None, false);
        assert_eq!(s.h, 16.0 * 1.2);
    }

    #[test]
    fn naive_counts_chars_not_bytes() {
        let m = NaiveMeasurer;
        let s = m.measure_text("😀😀😀😀", 10.0, None, false);
        assert!((s.w - 4.0 * 0.6 * 10.0).abs() < 1e-4, "{s:?}");
    }
}

#[derive(Clone)]
pub struct ProfiledMeasurer {
    pub fonts: SharedFonts,
    pub scale: f32,
    pub gpui_line_height: bool,
    pub gpui_wrap: bool,
}

const MEASURE_CACHE_LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct MeasureKey(u64, u64);

struct MeasureCache {
    limit: usize,
    current: std::collections::HashMap<MeasureKey, Size>,
    previous: std::collections::HashMap<MeasureKey, Size>,
}

impl MeasureCache {
    fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            current: std::collections::HashMap::new(),
            previous: std::collections::HashMap::new(),
        }
    }

    fn get(&mut self, key: MeasureKey) -> Option<Size> {
        match self.current.get(&key) {
            Some(size) => Some(*size),
            None => {
                let size = self.previous.remove(&key)?;
                self.insert(key, size);
                Some(size)
            }
        }
    }

    fn insert(&mut self, key: MeasureKey, size: Size) {
        if self.current.len() >= self.limit {
            self.previous = std::mem::take(&mut self.current);
            self.current.shrink_to(self.limit);
        }
        self.current.insert(key, size);
    }

    fn clear(&mut self) {
        self.current = std::collections::HashMap::new();
        self.previous = std::collections::HashMap::new();
    }
}

thread_local! {
    static MEASURE_CACHE: std::cell::RefCell<MeasureCache> =
        std::cell::RefCell::new(MeasureCache::new(MEASURE_CACHE_LIMIT));
}

pub fn trim_measure_cache() {
    MEASURE_CACHE.with(|cache| cache.borrow_mut().clear());
}

fn hash_style(hasher: &mut impl std::hash::Hasher, style: &TextStyle) {
    use std::hash::Hash;
    style.families.hash(hasher);
    hasher.write_u32(style.size_px.to_bits());
    hasher.write_u32(style.line_height_px.to_bits());
    style.weight.hash(hasher);
    style.slant.hash(hasher);
    style.stretch.hash(hasher);
    hasher.write_u32(style.letter_spacing_em.map_or(u32::MAX, f32::to_bits));
    style.font_features.hash(hasher);
}

impl ProfiledMeasurer {
    fn logical_size(&self, (width, height): (f32, f32)) -> Size {
        let scale = self.scale.max(0.01);
        let width = width / scale;
        Size::new(
            if self.gpui_wrap { width.ceil() } else { width },
            height / scale,
        )
    }

    fn key(
        &self,
        text: &str,
        style: &TextStyle,
        spans: &[StyledSpan],
        max_width: Option<f32>,
        multiline: bool,
    ) -> MeasureKey {
        use std::hash::{Hash, Hasher};
        let revision = self.fonts.lock().font_revision();
        let mut halves = [0u64; 2];
        for (seed, half) in halves.iter_mut().enumerate() {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            hasher.write_u8(seed as u8);
            hasher.write_u64(revision);
            hasher.write_u32(self.scale.to_bits());
            hasher.write_u8(u8::from(self.gpui_line_height));
            hasher.write_u8(u8::from(self.gpui_wrap));
            hasher.write_u8(u8::from(multiline));
            hasher.write_u32(max_width.map_or(u32::MAX, f32::to_bits));
            hasher.write_usize(text.len());
            hasher.write(text.as_bytes());
            hash_style(&mut hasher, style);
            hasher.write_usize(spans.len());
            for span in spans {
                span.text.hash(&mut hasher);
                hash_style(&mut hasher, &span.style);
                hasher.write_u32(span.inline_cell_width().map_or(u32::MAX, f32::to_bits));
            }
            *half = hasher.finish();
        }
        MeasureKey(halves[0], halves[1])
    }

    fn cached(&self, key: MeasureKey, measure: impl FnOnce() -> Size) -> Size {
        if let Some(size) = MEASURE_CACHE.with(|cache| cache.borrow_mut().get(key)) {
            return size;
        }
        let size = measure();
        MEASURE_CACHE.with(|cache| cache.borrow_mut().insert(key, size));
        size
    }
}

impl TextMeasurer for ProfiledMeasurer {
    fn memo_epoch(&self) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.fonts.lock().font_revision().hash(&mut hasher);
        self.scale.to_bits().hash(&mut hasher);
        self.gpui_line_height.hash(&mut hasher);
        self.gpui_wrap.hash(&mut hasher);
        Some(hasher.finish())
    }

    fn measure_text(
        &self,
        text: &str,
        font_size: f32,
        max_width: Option<f32>,
        multiline: bool,
    ) -> Size {
        let style = TextStyle {
            size_px: font_size,
            line_height_px: font_size * 1.2,
            ..TextStyle::default()
        };
        let key = self.key(
            text,
            &style,
            &[],
            if multiline { max_width } else { None },
            true,
        );
        self.cached(key, || {
            let mut layout = TextLayout::new();
            layout.set_gpui_line_height(self.gpui_line_height);
            layout.set_gpui_wrap(self.gpui_wrap);
            layout.set_text(text, &style);
            layout.set_line_mode(LineMode::Multi);
            layout.set_max_width(if multiline { max_width } else { None });
            layout.set_scale(self.scale);
            let laid = layout.layout(&mut self.fonts.lock());
            self.logical_size(laid.size)
        })
    }

    fn measure_styled(
        &self,
        text: &str,
        style: &TextStyle,
        spans: &[StyledSpan],
        max_width: Option<f32>,
        multiline: bool,
    ) -> Size {
        let key = self.key(text, style, spans, max_width, multiline);
        self.cached(key, || {
            let mut layout = TextLayout::new();
            layout.set_gpui_line_height(self.gpui_line_height);
            layout.set_gpui_wrap(self.gpui_wrap);
            match spans.is_empty() {
                true => layout.set_text(text, style),
                false => layout.set_spans(spans.to_vec()),
            }
            layout.set_line_mode(if multiline {
                LineMode::Multi
            } else {
                LineMode::Single
            });
            layout.set_wrap(if multiline {
                WrapMode::Word
            } else {
                WrapMode::None
            });
            layout.set_max_width(if multiline { max_width } else { None });
            layout.set_scale(self.scale);
            let laid = layout.layout(&mut self.fonts.lock());
            self.logical_size(laid.size)
        })
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    fn len(cache: &MeasureCache) -> usize {
        cache.current.len() + cache.previous.len()
    }

    fn key(n: u64) -> MeasureKey {
        MeasureKey(n, !n)
    }

    #[test]
    fn size_never_exceeds_twice_the_limit() {
        let mut cache = MeasureCache::new(8);
        (0..1000).for_each(|n| cache.insert(key(n), Size::new(n as f32, 1.0)));
        assert!(len(&cache) <= 16, "{}", len(&cache));
    }

    #[test]
    fn recent_entries_survive_a_rotation() {
        let mut cache = MeasureCache::new(4);
        (0..4).for_each(|n| cache.insert(key(n), Size::new(n as f32, 1.0)));
        cache.insert(key(4), Size::new(4.0, 1.0));
        assert_eq!(cache.get(key(1)), Some(Size::new(1.0, 1.0)));
        assert_eq!(cache.get(key(4)), Some(Size::new(4.0, 1.0)));
    }

    #[test]
    fn entries_untouched_for_two_generations_are_dropped() {
        let mut cache = MeasureCache::new(4);
        (0..4).for_each(|n| cache.insert(key(n), Size::new(n as f32, 1.0)));
        (4..12).for_each(|n| cache.insert(key(n), Size::new(n as f32, 1.0)));
        assert_eq!(cache.get(key(0)), None);
        assert!(cache.get(key(11)).is_some());
    }

    #[test]
    fn promoted_entries_stay_through_the_next_rotation() {
        let mut cache = MeasureCache::new(4);
        (0..4).for_each(|n| cache.insert(key(n), Size::new(n as f32, 1.0)));
        cache.insert(key(4), Size::new(4.0, 1.0));
        assert!(cache.get(key(0)).is_some());
        (5..8).for_each(|n| cache.insert(key(n), Size::new(n as f32, 1.0)));
        assert!(cache.get(key(0)).is_some());
    }

    #[test]
    fn clear_empties_both_generations() {
        let mut cache = MeasureCache::new(2);
        (0..3).for_each(|n| cache.insert(key(n), Size::new(1.0, 1.0)));
        cache.clear();
        assert_eq!(len(&cache), 0);
    }
}
