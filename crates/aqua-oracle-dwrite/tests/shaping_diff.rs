#![cfg(windows)]

use aqua_oracle_dwrite::{FontSource, Oracle};
use aqua_text::{FontFamily, FontManager, TextLayout, TextStyle};

const STRINGS: [&str; 5] = [
    "The quick brown fox jumps over the lazy dog",
    "Hamburgefonstiv 1234567890",
    "AVATAR To Wa Ty fi fl ffi",
    "if (value != null) { return value; }",
    "Aqua: text, shaped & laid out.",
];

#[test]
fn glyph_ids_and_advances_match_directwrite() {
    let mut fonts = FontManager::new();
    let mut oracle = Oracle::new().unwrap();
    for size in [13.0f32, 16.0, 26.0, 32.0, 48.0] {
        let style = TextStyle {
            families: vec![FontFamily::Named("Segoe UI".to_owned())],
            size_px: size,
            ..TextStyle::default()
        };
        let entry = fonts.primary(&style).expect("Segoe UI available");
        let source = FontSource {
            data: entry.data(),
            face_index: entry.face_index(),
            weight: entry.weight(),
        };
        for text in STRINGS {
            let mut layout = TextLayout::new();
            layout.set_text(text, &style);
            let laid = layout.layout(&mut fonts);
            let ours: Vec<_> = laid.lines[0].glyphs.iter().collect();
            let theirs = oracle.shape(source, size, text).unwrap();
            assert_eq!(
                ours.len(),
                theirs.len(),
                "glyph count for {text:?} at {size}"
            );
            let mut worst = 0.0f32;
            for (ours, theirs) in ours.iter().zip(&theirs) {
                assert_eq!(
                    ours.glyph_id, theirs.glyph_id,
                    "glyph id for {text:?} at {size}"
                );
                worst = worst.max((ours.w - theirs.advance_px).abs());
            }
            let total_ours: f32 = ours.iter().map(|glyph| glyph.w).sum();
            let total_theirs: f32 = theirs.iter().map(|glyph| glyph.advance_px).sum();
            println!(
                "shaping size={size} text={text:?} worst_advance_delta={worst:.4} total_delta={:.4}",
                total_ours - total_theirs
            );
            assert!(
                worst < 0.02 * size,
                "advance delta {worst} for {text:?} at {size}"
            );
        }
    }
}
