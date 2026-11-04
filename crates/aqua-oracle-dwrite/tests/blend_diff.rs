#![cfg(windows)]

use aqua_oracle_dwrite::{BlendParams, FontSource, GlyphRunSpec, Oracle};
use aqua_text::raster::{GlyphImage, Rasterizer, place_glyph};
use aqua_text::tone::{ToneCurve, luminance};
use aqua_text::{FontFamily, FontManager, TextLayout, TextStyle};

const STRINGS: [&str; 3] = [
    "The quick brown fox jumps over the lazy dog",
    "Aqua 1234567890 {}[]() @#&",
    "Hamburgefonstiv ffi fl AVAT To Wa",
];

struct Canvas {
    width: u32,
    height: u32,
}

fn coverage_canvas(
    fonts: &mut FontManager,
    text: &str,
    size: f32,
    canvas: &Canvas,
    origin: (i32, i32),
) -> (Vec<u8>, Vec<u32>, Vec<u16>, Vec<f32>) {
    let style = TextStyle {
        families: vec![FontFamily::Named("Segoe UI".to_owned())],
        size_px: size,
        ..TextStyle::default()
    };
    let mut layout = TextLayout::new();
    layout.set_text(text, &style);
    let laid = layout.layout(fonts);
    let mut rasterizer = Rasterizer::new();
    let mut coverage = vec![0u8; (canvas.width * canvas.height) as usize];
    let mut ids = Vec::new();
    let mut advances = Vec::new();
    for glyph in &laid.lines[0].glyphs {
        ids.push(glyph.glyph_id);
        advances.push(glyph.w);
        let placement = place_glyph(glyph, origin.0 as f32, origin.1 as f32, true);
        let Some(GlyphImage::Mask(bitmap)) = rasterizer.rasterize(fonts, &placement.key) else {
            continue;
        };
        for row in 0..bitmap.height as i32 {
            for column in 0..bitmap.width as i32 {
                let x = placement.x + bitmap.left + column;
                let y = placement.y - bitmap.top + row;
                if x >= 0 && y >= 0 && (x as u32) < canvas.width && (y as u32) < canvas.height {
                    let slot = &mut coverage[(y as u32 * canvas.width + x as u32) as usize];
                    *slot = slot.saturating_add(
                        bitmap.data[(row as u32 * bitmap.width + column as u32) as usize],
                    );
                }
            }
        }
    }
    (coverage, Vec::new(), ids, advances)
}

fn blend(coverage: &[u8], curve: &ToneCurve, foreground: [u8; 3], background: [u8; 3]) -> Vec<u8> {
    let lum = luminance(foreground.map(|c| f32::from(c) / 255.0));
    let mut out = Vec::with_capacity(coverage.len() * 3);
    for value in coverage {
        let alpha = curve.adjust(f32::from(*value) / 255.0, lum);
        for channel in 0..3 {
            let f = f32::from(foreground[channel]);
            let b = f32::from(background[channel]);
            out.push((b + (f - b) * alpha).round().clamp(0.0, 255.0) as u8);
        }
    }
    out
}

fn ink(pixels: &[u8], background: [u8; 3]) -> f64 {
    pixels
        .chunks_exact(3)
        .map(|pixel| {
            (0..3)
                .map(|c| (f64::from(pixel[c]) - f64::from(background[c])).abs())
                .sum::<f64>()
        })
        .sum()
}

fn mean_abs(a: &[u8], b: &[u8], background: [u8; 3]) -> f64 {
    let mut total = 0.0f64;
    let mut count = 0.0f64;
    for (pa, pb) in a.chunks_exact(3).zip(b.chunks_exact(3)) {
        if pa != background || pb != background {
            for channel in 0..3 {
                total += (f64::from(pa[channel]) - f64::from(pb[channel])).abs();
                count += 1.0;
            }
        }
    }
    total / count.max(1.0)
}

#[test]
fn native_tone_curve_tracks_directwrite_blending() {
    let mut fonts = FontManager::new();
    let mut oracle = Oracle::new().unwrap();
    let system = oracle.system_rendering().unwrap();
    let curve = ToneCurve::new(system.gamma, system.grayscale_enhanced_contrast);
    assert_eq!(curve, ToneCurve::native());
    let entry = fonts
        .primary(&TextStyle {
            families: vec![FontFamily::Named("Segoe UI".to_owned())],
            ..TextStyle::default()
        })
        .unwrap();
    let source = FontSource {
        data: entry.data(),
        face_index: entry.face_index(),
        weight: 400,
    };
    for (name, fg, bg) in [
        ("black_on_white", [0u8; 3], [255u8; 3]),
        ("white_on_black", [255u8; 3], [0u8; 3]),
        ("gray_on_dark", [200, 200, 200], [30, 30, 36]),
        ("navy_on_cream", [20, 30, 80], [250, 245, 235]),
    ] {
        for size in [13.0f32, 16.0, 24.0, 32.0, 48.0] {
            let mut ratio = 0.0;
            let mut error = 0.0;
            let mut unadjusted_error = 0.0;
            for text in STRINGS {
                let canvas = Canvas {
                    width: 1400,
                    height: 96,
                };
                let origin = (8, (size * 1.3).round() as i32);
                let (coverage, _, ids, advances) =
                    coverage_canvas(&mut fonts, text, size, &canvas, origin);
                let native = oracle
                    .blended(
                        GlyphRunSpec {
                            font: source,
                            size_px: size,
                            glyphs: &ids,
                            advances: &advances,
                        },
                        (origin.0 as f32, origin.1 as f32),
                        (canvas.width, canvas.height),
                        BlendParams {
                            gamma: system.gamma,
                            enhanced_contrast: system.grayscale_enhanced_contrast,
                            foreground: fg,
                            background: bg,
                        },
                    )
                    .unwrap();
                let toned = blend(&coverage, &curve, fg, bg);
                let exact = blend(&coverage, &ToneCurve::EXACT, fg, bg);
                ratio += ink(&toned, bg) / ink(&native.pixels, bg);
                error += mean_abs(&toned, &native.pixels, bg);
                unadjusted_error += mean_abs(&exact, &native.pixels, bg);
            }
            let n = STRINGS.len() as f64;
            let (ratio, error, unadjusted_error) = (ratio / n, error / n, unadjusted_error / n);
            println!(
                "blend {name} size={size} ink_ratio={ratio:.4} error={error:.3} unadjusted_error={unadjusted_error:.3}"
            );
            assert!(
                (0.93..=1.06).contains(&ratio),
                "{name} at {size}: ink ratio {ratio}"
            );
            assert!(error < 18.0, "{name} at {size}: error {error}");
        }
    }
}

#[test]
#[ignore]
fn fit_tone_curve() {
    let mut fonts = FontManager::new();
    let mut oracle = Oracle::new().unwrap();
    let system = oracle.system_rendering().unwrap();
    let entry = fonts
        .primary(&TextStyle {
            families: vec![FontFamily::Named("Segoe UI".to_owned())],
            ..TextStyle::default()
        })
        .unwrap();
    let source = FontSource {
        data: entry.data(),
        face_index: entry.face_index(),
        weight: 400,
    };
    let mut cases = Vec::new();
    for (fg, bg) in [
        ([0u8; 3], [255u8; 3]),
        ([255u8; 3], [0u8; 3]),
        ([200, 200, 200], [30, 30, 36]),
        ([20, 30, 80], [250, 245, 235]),
        ([120, 120, 120], [255, 255, 255]),
        ([140, 140, 150], [24, 24, 28]),
    ] {
        for size in [13.0f32, 16.0, 24.0, 32.0] {
            for text in STRINGS {
                let canvas = Canvas {
                    width: 1400,
                    height: 96,
                };
                let origin = (8, (size * 1.3).round() as i32);
                let (coverage, _, ids, advances) =
                    coverage_canvas(&mut fonts, text, size, &canvas, origin);
                let native = oracle
                    .blended(
                        GlyphRunSpec {
                            font: source,
                            size_px: size,
                            glyphs: &ids,
                            advances: &advances,
                        },
                        (origin.0 as f32, origin.1 as f32),
                        (canvas.width, canvas.height),
                        BlendParams {
                            gamma: system.gamma,
                            enhanced_contrast: system.grayscale_enhanced_contrast,
                            foreground: fg,
                            background: bg,
                        },
                    )
                    .unwrap();
                cases.push((coverage, native.pixels, fg, bg, size));
            }
        }
    }
    let mut results = Vec::new();
    for gamma10 in 10..=22 {
        for contrast4 in 0..=10 {
            let gamma = gamma10 as f32 / 10.0;
            let contrast = contrast4 as f32 / 4.0;
            let curve = ToneCurve::new(gamma, contrast);
            let mut error = 0.0;
            let mut ink_bias = 0.0;
            for (coverage, native, fg, bg, _) in &cases {
                let toned = blend(coverage, &curve, *fg, *bg);
                error += mean_abs(&toned, native, *bg);
                ink_bias += (ink(&toned, *bg) / ink(native, *bg)).ln().abs();
            }
            results.push((
                error / cases.len() as f64,
                ink_bias / cases.len() as f64,
                gamma,
                contrast,
            ));
        }
    }
    results.sort_by(|a, b| a.0.total_cmp(&b.0));
    for r in results.iter().take(8) {
        println!(
            "fit error={:.3} ink_bias={:.4} gamma={} contrast={}",
            r.0, r.1, r.2, r.3
        );
    }
    let native = results.iter().find(|r| r.2 == 1.8 && r.3 == 1.0).unwrap();
    println!("fit native error={:.3} ink_bias={:.4}", native.0, native.1);
}
