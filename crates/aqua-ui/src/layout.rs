//! Text measurement hook for Taffy integration.
//!
//! Layout must measure text under the same constraints it lays out with, but
//! shaping belongs to `aqua-text`. This module defines the narrow provider
//! trait plus a naive bootstrap implementation for tests. The cosmic-text
//! provider (text owner) implements [`TextMeasurer`] with real shaping;
//! swapping providers requires no UI change because [`UiTree::compute_layout`]
//! takes `&dyn TextMeasurer`.
//!
//! [`UiTree::compute_layout`]: crate::tree::UiTree::compute_layout

use aqua_core::Size;
use aqua_text::{measure_text, SharedFonts, TextStyle};

/// Text metrics provider for layout. Implementations must return the same
/// advance/line metrics the renderer will draw with; otherwise committed
/// layout and final pixels disagree (a layout/render contract violation the
/// conformance suite checks once the real provider lands).
pub trait TextMeasurer {
    /// Measure `text` at `font_size` logical px.
    ///
    /// - `max_width`: wrapping width when `multiline` is set (`None` =
    ///   unbounded single line).
    /// - `multiline`: wrap at `max_width`; embedded newlines always break.
    fn measure_text(
        &self,
        text: &str,
        font_size: f32,
        max_width: Option<f32>,
        multiline: bool,
    ) -> Size;
}

/// Naive bootstrap measurer: average advance 0.6em, line height 1.2em,
/// character-count wrapping. For tests and pre-text-provider bring-up ONLY;
/// it knows nothing about fonts, shaping, or bidirectional text.
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
            // Caret still needs a line.
            return Size::new(0.0, font_size * LINE);
        }
        if !multiline {
            // Longest physical line wins (embedded newlines break).
            let longest = text
                .split('\n')
                .map(|l| l.chars().count() as f32)
                .fold(0.0f32, f32::max);
            let lines = text.split('\n').count() as f32;
            return Size::new(longest * ADVANCE * font_size, lines * LINE * font_size);
        }
        match max_width {
            Some(w) if w > 0.0 => {
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

/// Production measurer backed by cosmic-text shaping through the shared
/// font manager. The same [`SharedFonts`] must be handed to the renderer so
/// measurement and pixels share one font system (see text contract).
#[derive(Clone)]
pub struct CosmicMeasurer {
    /// Shared font manager (also used by the renderer).
    pub fonts: SharedFonts,
    /// Device/pinch scale applied inside shaping; outputs stay logical px.
    pub scale: f32,
}

impl TextMeasurer for CosmicMeasurer {
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
        // 4 emoji, 16 bytes: width must reflect 4 advances, not 16.
        let s = m.measure_text("😀😀😀😀", 10.0, None, false);
        assert!((s.w - 4.0 * 0.6 * 10.0).abs() < 1e-4, "{s:?}");
    }
}
