#![cfg_attr(not(windows), allow(unused))]
use aqua_text::{FontManager, TextLayout, TextStyle};
use harfrust::{Direction, FontRef, ShapeOptions, ShaperData, UnicodeBuffer};

const SAMPLE: &str = "The quick brown fox jumps over the lazy dog, and then some. AVATAR To";
const SIZE: f32 = 16.0;

fn aqua_advances(fonts: &mut FontManager, letter_spacing_em: Option<f32>) -> Vec<(f32, f32)> {
    let style = TextStyle {
        size_px: SIZE,
        letter_spacing_em,
        ..TextStyle::default()
    };
    let mut layout = TextLayout::new();
    layout.set_text(SAMPLE, &style);
    layout.layout(fonts).lines[0]
        .glyphs
        .iter()
        .map(|glyph| (glyph.x, glyph.w))
        .collect()
}

fn reference_advances(fonts: &mut FontManager) -> Vec<f32> {
    let entry = fonts.primary(&TextStyle::default()).expect("a system font");
    let font = FontRef::from_index(entry.data(), entry.face_index()).unwrap();
    let data = ShaperData::new(&font);
    let shaper = data.shaper(&font).build();
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(SAMPLE);
    buffer.set_direction(Direction::LeftToRight);
    buffer.guess_segment_properties();
    let shaped = shaper.shape(buffer, ShapeOptions::new());
    let scale = SIZE / entry.units_per_em();
    shaped
        .glyph_positions()
        .iter()
        .map(|position| position.x_advance as f32 * scale)
        .collect()
}

#[cfg(windows)]
#[test]
fn glyph_advances_match_unhinted_reference_shaping() {
    let mut fonts = FontManager::new();
    let reference = reference_advances(&mut fonts);
    let laid = aqua_advances(&mut fonts, None);
    assert_eq!(laid.len(), reference.len());
    for ((_, width), expected) in laid.iter().zip(&reference) {
        assert!((width - expected).abs() < 1e-3, "{width} != {expected}");
    }
    let mut pen = 0.0;
    for ((x, width), _) in laid.iter().zip(&reference) {
        assert!((x - pen).abs() < 1e-2);
        pen += width;
    }
}

#[test]
fn tracking_shifts_every_advance_by_em_fraction_of_size() {
    let mut fonts = FontManager::new();
    let natural = aqua_advances(&mut fonts, None);
    let tracked = aqua_advances(&mut fonts, Some(-0.04));
    assert_eq!(natural.len(), tracked.len());
    for ((_, natural_width), (_, tracked_width)) in natural.iter().zip(&tracked) {
        assert!((tracked_width - (natural_width - 0.04 * SIZE)).abs() < 1e-3);
    }
    for pair in tracked.windows(2) {
        assert!((pair[0].0 + pair[0].1 - pair[1].0).abs() < 1e-3);
    }
}
