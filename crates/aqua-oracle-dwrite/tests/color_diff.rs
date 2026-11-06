#![cfg(windows)]

use aqua_oracle_dwrite::{ColorCanvas, FontSource, GlyphRunSpec, Oracle};
use aqua_text::raster::{place_glyph, GlyphImage, Rasterizer};
use aqua_text::{FontFamily, FontManager, TextLayout, TextStyle};

const SIZES: [f32; 4] = [16.0, 24.0, 32.0, 48.0];
const PHASES: [f32; 4] = [0.0, 0.25, 0.5, 0.75];
const EMOJI: [&str; 3] = ["\u{1f600}", "\u{1f680}", "\u{2764}\u{fe0f}"];

#[derive(Clone, Copy, Debug)]
enum Policy {
    FloorBin0,
    NearestBin0,
    PhasedBins,
}

#[derive(Default)]
struct Totals {
    centroid_error: f64,
    worst_centroid_error: f64,
    ink_ratio: f64,
    samples: f64,
}

fn centroid(rgba: &[u8], width: u32) -> Option<(f64, f64)> {
    let mut weight = 0.0;
    let mut x = 0.0;
    for (index, texel) in rgba.as_chunks::<4>().0.iter().enumerate() {
        let alpha = f64::from(texel[3]);
        weight += alpha;
        x += alpha * ((index as u32 % width) as f64 + 0.5);
    }
    (weight > 0.0).then(|| (x / weight, weight))
}

fn ours(
    rasterizer: &mut Rasterizer,
    fonts: &mut FontManager,
    glyph: &aqua_text::LaidGlyph,
    pen_x: f32,
    baseline: f32,
    size: (u32, u32),
    policy: Policy,
) -> Option<Vec<u8>> {
    let placement = place_glyph(glyph, pen_x, baseline, true);
    let (key, shift) = match policy {
        Policy::FloorBin0 => (
            aqua_text::raster::RasterKey {
                subpixel: 0,
                ..placement.key
            },
            0,
        ),
        Policy::NearestBin0 => (
            aqua_text::raster::RasterKey {
                subpixel: 0,
                ..placement.key
            },
            placement.color_x() - placement.x,
        ),
        Policy::PhasedBins => (placement.key, 0),
    };
    let GlyphImage::Color(bitmap) = rasterizer.rasterize(fonts, &key)? else {
        return None;
    };
    let mut canvas = vec![0u8; (size.0 * size.1 * 4) as usize];
    for row in 0..bitmap.height as i32 {
        for column in 0..bitmap.width as i32 {
            let x = placement.x + shift + bitmap.left + column;
            let y = placement.y - bitmap.top + row;
            if x >= 0 && y >= 0 && (x as u32) < size.0 && (y as u32) < size.1 {
                let from = ((row as u32 * bitmap.width + column as u32) * 4) as usize;
                let to = ((y as u32 * size.0 + x as u32) * 4) as usize;
                canvas[to..to + 4].copy_from_slice(&bitmap.data[from..from + 4]);
            }
        }
    }
    Some(canvas)
}

fn measure(policy: Policy) -> Totals {
    let mut fonts = FontManager::new();
    let mut oracle = Oracle::new().unwrap();
    let mut rasterizer = Rasterizer::new();
    let mut totals = Totals::default();
    for text in EMOJI {
        for size in SIZES {
            let style = TextStyle {
                families: vec![FontFamily::Named("Segoe UI Emoji".to_owned())],
                size_px: size,
                ..TextStyle::default()
            };
            let mut layout = TextLayout::new();
            layout.set_text(text, &style);
            let laid = layout.layout(&mut fonts);
            let glyph = laid.lines[0].glyphs[0].clone();
            let entry = fonts
                .entry_with(glyph.font_id, glyph.font_weight, glyph.font_stretch, false)
                .expect("emoji face");
            assert!(
                entry.postscript_name().starts_with("SegoeUIEmoji"),
                "{}",
                entry.postscript_name()
            );
            let source = FontSource {
                data: entry.data(),
                face_index: entry.face_index(),
                weight: entry.weight(),
            };
            assert!(oracle.has_color_layers(source, glyph.glyph_id).unwrap());
            let extent = ((size * 3.0).ceil() as u32, (size * 2.5).ceil() as u32);
            let baseline = (size * 1.8).round();
            for phase in PHASES {
                let pen_x = size + phase;
                let native: ColorCanvas = oracle
                    .color_layers(
                        GlyphRunSpec {
                            font: source,
                            size_px: glyph.font_size_px,
                            glyphs: &[glyph.glyph_id],
                            advances: &[glyph.w],
                        },
                        (pen_x, baseline),
                        extent,
                        [0.0; 3],
                    )
                    .unwrap();
                let mine = ours(
                    &mut rasterizer,
                    &mut fonts,
                    &glyph,
                    pen_x - glyph.x,
                    baseline,
                    extent,
                    policy,
                )
                .expect("aqua color raster");
                let (Some((native_x, native_ink)), Some((our_x, our_ink))) = (
                    centroid(&native.premultiplied_rgba, extent.0),
                    centroid(&mine, extent.0),
                ) else {
                    panic!("no ink for {text:?} at {size}");
                };
                let error = (native_x - our_x).abs();
                totals.centroid_error += error;
                totals.worst_centroid_error = totals.worst_centroid_error.max(error);
                totals.ink_ratio += our_ink / native_ink;
                totals.samples += 1.0;
            }
        }
    }
    totals
}

#[test]
fn whole_pixel_color_glyph_policies_against_directwrite_layers() {
    let floor = measure(Policy::FloorBin0);
    let nearest = measure(Policy::NearestBin0);
    let phased = measure(Policy::PhasedBins);
    for (name, totals) in [
        ("floor", &floor),
        ("nearest", &nearest),
        ("phased", &phased),
    ] {
        println!(
            "{name}: mean centroid error {:.3} px, worst {:.3} px, mean ink ratio {:.3} over {} glyph placements",
            totals.centroid_error / totals.samples,
            totals.worst_centroid_error,
            totals.ink_ratio / totals.samples,
            totals.samples
        );
    }
    let mean = |totals: &Totals| totals.centroid_error / totals.samples;
    assert!(
        mean(&phased) < 0.2,
        "phased bitmaps track DirectWrite subpixel placement"
    );
    assert!(mean(&nearest) < 0.3 && nearest.worst_centroid_error < 0.8);
    assert!(mean(&nearest) < mean(&floor) && mean(&phased) * 2.0 < mean(&nearest));
    for totals in [&floor, &nearest, &phased] {
        assert!((0.85..=1.15).contains(&(totals.ink_ratio / totals.samples)));
    }
}
