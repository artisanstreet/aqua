use std::time::Duration;

use aqua::render::text::{ShimmerClocks, TextScope, TextStack};
use aqua_core::scene::{GpuiTextColorMap, GpuiTextOpacityRange, GpuiTextShimmer};
use aqua_core::{GpuiHsla, GpuiTextColor, Rect};

fn shimmer(key: u64, duration_ms: u64, delay_ms: u64) -> GpuiTextShimmer {
    GpuiTextShimmer {
        map: GpuiTextColorMap {
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 20.0,
            },
            phase: 0.0,
            spread: 30.0,
            target: GpuiTextColor {
                hue_degrees: 0.0,
                saturation: 0.0,
                lightness: 1.0,
                alpha: 1.0,
            },
        },
        key,
        duration: Duration::from_millis(duration_ms),
        delay: Duration::from_millis(delay_ms),
    }
}

fn phase_at(clocks: &mut ShimmerClocks, spec: &GpuiTextShimmer, seconds: f64) -> f32 {
    clocks.begin_frame();
    let phase = clocks.phase(spec, seconds);
    clocks.end_frame();
    phase
}

#[test]
fn shimmer_holds_at_zero_during_the_delay_then_runs_and_wraps() {
    let mut clocks = ShimmerClocks::default();
    let spec = shimmer(1, 1000, 2000);
    assert_eq!(phase_at(&mut clocks, &spec, 0.0), 0.0);
    assert_eq!(phase_at(&mut clocks, &spec, 1.5), 0.0);
    assert_eq!(phase_at(&mut clocks, &spec, 2.0), 0.0);
    let halfway = phase_at(&mut clocks, &spec, 2.5);
    assert!((halfway - 0.5).abs() < 1e-4);
    assert_eq!(phase_at(&mut clocks, &spec, 3.0), 0.0);
    assert_eq!(phase_at(&mut clocks, &spec, 4.0), 0.0);
}

#[test]
fn shimmer_clocks_drop_keys_that_were_not_drawn() {
    let mut clocks = ShimmerClocks::default();
    let spec = shimmer(7, 1000, 0);
    phase_at(&mut clocks, &spec, 0.0);
    assert!(clocks.active());
    clocks.begin_frame();
    clocks.end_frame();
    assert!(!clocks.active());
}

#[test]
fn one_phase_per_key_within_a_frame() {
    let mut clocks = ShimmerClocks::default();
    let spec = shimmer(3, 1000, 0);
    clocks.begin_frame();
    let first = clocks.phase(&spec, 0.0);
    let again = clocks.phase(&spec, 0.4);
    assert_eq!(first, again);
}

#[test]
fn text_scopes_pop_only_their_own_kind() {
    let mut stack = TextStack::default();
    stack.push_color(GpuiHsla {
        h: 0.0,
        s: 1.0,
        l: 0.5,
        a: 1.0,
    });
    assert!(!stack.pop(TextScope::SourceColor));
    assert!(stack.pop(TextScope::Color));
    assert!(!stack.pop(TextScope::Color));
    assert!(stack.is_balanced());
}

fn glyph_at(byte: usize) -> aqua_text::layout::LaidGlyph {
    aqua_text::layout::LaidGlyph {
        display_byte_start: byte,
        display_byte_end: byte + 1,
        src_byte_start: byte,
        src_byte_end: byte + 1,
        font_id: aqua_text::SharedFonts::new().lock().first_face().unwrap(),
        font_weight: 400,
        font_stretch: Default::default(),
        synthetic_italic: false,
        glyph_id: 0,
        font_size_px: 16.0,
        x: 0.0,
        y: 0.0,
        x_offset: 0.0,
        y_offset: 0.0,
        w: 8.0,
        bidi_level: 0,
        color_srgb: None,
        color_hsla: None,
        source_color_hsla: None,
        font_max_width: None,
    }
}

#[test]
fn opacity_lookup_preserves_boundaries_and_first_matching_range() {
    for ranges in [
        vec![(0..3, 0.2), (3..3, 0.0), (5..8, 0.8)],
        vec![(5..8, 0.8), (0..3, 0.2)],
        vec![(2..7, 0.4), (0..4, 0.2), (6..9, 0.8)],
        vec![(3..3, 0.0), (0..10, 1.0)],
        vec![],
    ] {
        let ranges: Vec<_> = ranges
            .into_iter()
            .map(|(bytes, opacity)| GpuiTextOpacityRange { bytes, opacity })
            .collect();
        let mut stack = TextStack::default();
        stack.push_opacity(ranges.clone().into());
        for byte in 0..12 {
            let expected = ranges
                .iter()
                .find(|range| range.bytes.contains(&byte))
                .map_or(1.0, |range| range.opacity);
            assert_eq!(stack.range_opacity(&glyph_at(byte)), expected);
        }
    }
}

#[test]
fn settled_opacity_scopes_restore_caching_and_pop_to_the_previous_scope() {
    let mut stack = TextStack::default();
    let signature = stack.cache_signature();
    stack.push_opacity(
        vec![GpuiTextOpacityRange {
            bytes: 0..10,
            opacity: 0.5,
        }]
        .into(),
    );
    assert!(stack.cache_signature().is_none());
    stack.push_opacity(
        vec![GpuiTextOpacityRange {
            bytes: 0..10,
            opacity: 1.0,
        }]
        .into(),
    );
    assert_eq!(stack.cache_signature(), signature);
    assert_eq!(stack.range_opacity(&glyph_at(5)), 1.0);
    assert!(stack.pop(TextScope::Opacity));
    assert_eq!(stack.range_opacity(&glyph_at(5)), 0.5);
    assert!(stack.cache_signature().is_none());
    assert!(stack.pop(TextScope::Opacity));
    assert_eq!(stack.cache_signature(), signature);
}
