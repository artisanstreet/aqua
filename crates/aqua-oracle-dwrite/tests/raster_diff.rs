#![cfg(windows)]

use aqua_oracle_dwrite::{FontSource, GlyphRunSpec, Oracle, RenderMode};
use aqua_text::raster::{GlyphImage, Rasterizer, place_glyph};
use aqua_text::{FontFamily, FontManager, TextLayout, TextStyle};

const STRINGS: [&str; 4] = [
    "Hamburgefonstiv",
    "The quick brown fox jumps over the lazy dog",
    "Aqua 1234567890 {}[]() @#&",
    "ffi fl AVAT To Wa",
];

struct Report {
    mean_abs: f64,
    ink_ratio: f64,
}

fn style(size: f32) -> TextStyle {
    TextStyle {
        families: vec![FontFamily::Named("Segoe UI".to_owned())],
        size_px: size,
        ..TextStyle::default()
    }
}

fn compare(text: &str, size: f32, mode: RenderMode) -> Report {
    let mut fonts = FontManager::new();
    let style = style(size);
    let entry = fonts.primary(&style).expect("Segoe UI available");
    assert_eq!(entry.postscript_name(), "SegoeUI");
    let mut layout = TextLayout::new();
    layout.set_text(text, &style);
    let laid = layout.layout(&mut fonts);
    let mut oracle = Oracle::new().unwrap();
    let source = FontSource {
        data: entry.data(),
        face_index: entry.face_index(),
        weight: entry.weight(),
    };
    let mut rasterizer = Rasterizer::new();
    let mut absolute = 0.0f64;
    let mut pixels = 0.0f64;
    let mut ink_aqua = 0.0f64;
    let mut ink_native = 0.0f64;
    for glyph in laid.lines[0].glyphs.iter().filter(|g| g.w > 0.0) {
        let placement = place_glyph(glyph, 0.0, 0.0, true);
        let Some(GlyphImage::Mask(bitmap)) = rasterizer.rasterize(&mut fonts, &placement.key)
        else {
            continue;
        };
        let size_px = (size.ceil() as u32) * 3;
        let width = size_px * 2;
        let height = size_px * 2;
        let origin = (
            (size_px / 2) as f32 + f32::from(placement.key.subpixel) / 4.0,
            (size_px as f32 * 1.4).round(),
        );
        let native = oracle
            .coverage(
                GlyphRunSpec {
                    font: source,
                    size_px: size,
                    glyphs: &[glyph.glyph_id],
                    advances: &[glyph.w],
                },
                origin,
                (width, height),
                mode,
            )
            .unwrap();
        let mut ours = vec![0u8; (width * height) as usize];
        let base_x = origin.0.floor() as i32;
        let base_y = origin.1.round() as i32;
        for row in 0..bitmap.height as i32 {
            for column in 0..bitmap.width as i32 {
                let x = base_x + bitmap.left + column;
                let y = base_y - bitmap.top + row;
                if x >= 0 && y >= 0 && (x as u32) < width && (y as u32) < height {
                    ours[(y as u32 * width + x as u32) as usize] =
                        bitmap.data[(row as u32 * bitmap.width + column as u32) as usize];
                }
            }
        }
        for (a, b) in ours.iter().zip(&native.pixels) {
            absolute += (f64::from(*a) - f64::from(*b)).abs();
            ink_aqua += f64::from(*a);
            ink_native += f64::from(*b);
        }
        pixels += (bitmap.width * bitmap.height) as f64;
    }
    Report {
        mean_abs: absolute / pixels.max(1.0),
        ink_ratio: ink_aqua / ink_native.max(1.0),
    }
}

#[test]
fn unhinted_glyph_coverage_matches_directwrite() {
    for size in [13.0f32, 16.0, 24.0, 26.0, 32.0, 48.0] {
        for text in STRINGS {
            let report = compare(text, size, RenderMode::Unhinted);
            assert!(
                report.mean_abs < 9.0,
                "mean abs {} for {text:?} at {size}",
                report.mean_abs
            );
            assert!(
                (0.97..=1.02).contains(&report.ink_ratio),
                "ink ratio {} for {text:?} at {size}",
                report.ink_ratio
            );
        }
    }
}
