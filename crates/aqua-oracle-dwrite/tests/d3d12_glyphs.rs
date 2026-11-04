#![cfg(windows)]

use aqua_core::Oklch;
use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend};
use aqua_gpu_ref::{conformance, RefBackend};
use aqua_graph::tone::linear_to_srgb;
use aqua_graph::{Extent, FramePlan, GlyphInstance, PlanLimits, RenderGraph, TextureRef};
use aqua_media::atlas::PageId;
use aqua_media::residency::{Residency, ResidencyConfig};
use aqua_media::upload::{glyph_passes, upload_batch};
use aqua_oracle_dwrite::{BlendParams, FontSource, GlyphRunSpec, Oracle};
use aqua_protocol::{GpuBackend, SurfacePixels, UploadBatch};
use aqua_text::residency::{GlyphResolver, RunStyle};
use aqua_text::{FontFamily, FontManager, TextLayout, TextStyle};

const STRINGS: [&str; 3] = [
    "The quick brown fox jumps over the lazy dog",
    "Aqua 1234567890 {}[]() @#&",
    "Hamburgefonstiv ffi fl AVAT To Wa",
];

struct Scene {
    instances: Vec<(PageId, GlyphInstance)>,
    ids: Vec<u16>,
    advances: Vec<f32>,
}

fn hex(rgb: [u8; 3]) -> Oklch {
    Oklch::from_hex(&format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])).unwrap()
}

fn scene(
    fonts: &mut FontManager,
    resolver: &mut GlyphResolver,
    residency: &mut Residency,
    text: &str,
    size: f32,
    origin: (i32, i32),
    foreground: Oklch,
) -> Scene {
    let style = TextStyle {
        families: vec![FontFamily::Named("Segoe UI".to_owned())],
        size_px: size,
        ..TextStyle::default()
    };
    let mut layout = TextLayout::new();
    layout.set_text(text, &style);
    let laid = layout.layout(fonts);
    let draw = resolver
        .lower(
            residency,
            fonts,
            &laid,
            [origin.0 as f32, origin.1 as f32 - laid.lines[0].baseline_y],
            RunStyle {
                color: foreground,
                opacity: 1.0,
                clip: None,
            },
        )
        .expect("glyphs resident");
    let glyphs = &laid.lines[0].glyphs;
    Scene {
        instances: draw.masks,
        ids: glyphs.iter().map(|glyph| glyph.glyph_id).collect(),
        advances: glyphs.iter().map(|glyph| glyph.w).collect(),
    }
}

fn plan(instances: Vec<(PageId, GlyphInstance)>, size: (u32, u32), background: Oklch) -> FramePlan {
    let mut graph = RenderGraph::new(
        Extent {
            width: size.0,
            height: size.1,
        },
        background,
    );
    glyph_passes(instances, TextureRef::Surface)
        .into_iter()
        .for_each(|pass| graph.push(pass));
    graph.plan(PlanLimits::default()).unwrap()
}

fn encoded_rgb(pixels: &SurfacePixels) -> Vec<u8> {
    pixels
        .rgba8
        .chunks_exact(4)
        .flat_map(|texel| {
            [0, 1, 2].map(|channel| {
                (linear_to_srgb(f32::from(texel[channel]) / 255.0) * 255.0).round() as u8
            })
        })
        .collect()
}

fn ink(pixels: &[u8], background: [u8; 3]) -> f64 {
    pixels
        .chunks_exact(3)
        .map(|pixel| {
            (0..3)
                .map(|channel| (f64::from(pixel[channel]) - f64::from(background[channel])).abs())
                .sum::<f64>()
        })
        .sum()
}

fn mean_abs(a: &[u8], b: &[u8], background: [u8; 3]) -> f64 {
    let (mut total, mut count) = (0.0f64, 0.0f64);
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

fn max_difference(a: &SurfacePixels, b: &SurfacePixels) -> u8 {
    a.rgba8
        .iter()
        .zip(&b.rgba8)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

#[test]
fn glyph_pixels_through_d3d12_track_directwrite_and_the_reference() {
    let mut oracle = Oracle::new().unwrap();
    let system = oracle.system_rendering().unwrap();
    let mut fonts = FontManager::new();
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
    let mut ran = 0;
    for preference in [AdapterPreference::Warp, AdapterPreference::HighPerformance] {
        let Ok(mut gpu) = OffscreenBackend::new(preference) else {
            eprintln!("SKIP {preference:?}");
            continue;
        };
        let mut reference = RefBackend::new();
        for (name, fg, bg) in [
            ("black_on_white", [0u8; 3], [255u8; 3]),
            ("white_on_black", [255u8; 3], [0u8; 3]),
            ("gray_on_dark", [200, 200, 200], [30, 30, 36]),
            ("navy_on_cream", [20, 30, 80], [250, 245, 235]),
        ] {
            for size in [13.0f32, 16.0, 24.0, 32.0] {
                let (mut ratio, mut error, mut backend_gap) = (0.0, 0.0, 0u8);
                for text in STRINGS {
                    let canvas = (1100u32, (size * 2.4) as u32);
                    let origin = (8, (size * 1.3).round() as i32);
                    let mut resolver = GlyphResolver::new();
                    let mut residency = Residency::new(ResidencyConfig::default());
                    residency.begin_frame();
                    let scene = scene(
                        &mut fonts,
                        &mut resolver,
                        &mut residency,
                        text,
                        size,
                        origin,
                        hex(fg),
                    );
                    let uploads: UploadBatch = upload_batch(residency.finish_frame());
                    reference.upload(uploads.clone()).unwrap();
                    gpu.upload(uploads).unwrap();
                    let frame = plan(scene.instances, canvas, hex(bg));
                    let expected = conformance::render(&mut reference, &frame).unwrap();
                    let actual = conformance::render(&mut gpu, &frame).unwrap();
                    backend_gap = backend_gap.max(max_difference(&expected, &actual));
                    let native = oracle
                        .blended(
                            GlyphRunSpec {
                                font: source,
                                size_px: size,
                                glyphs: &scene.ids,
                                advances: &scene.advances,
                            },
                            (origin.0 as f32, origin.1 as f32),
                            canvas,
                            BlendParams {
                                gamma: system.gamma,
                                enhanced_contrast: system.grayscale_enhanced_contrast,
                                foreground: fg,
                                background: bg,
                            },
                        )
                        .unwrap();
                    let encoded = encoded_rgb(&actual);
                    let rendered_bg = [encoded[0], encoded[1], encoded[2]];
                    ratio += ink(&encoded, rendered_bg) / ink(&native.pixels, bg);
                    error += mean_abs(&encoded, &native.pixels, rendered_bg);
                }
                let n = STRINGS.len() as f64;
                let (ratio, error) = (ratio / n, error / n);
                println!(
                    "d3d12 glyphs {preference:?} {name} size={size} ink_ratio={ratio:.4} error={error:.3} backend_gap={backend_gap}"
                );
                assert!(
                    backend_gap <= 3,
                    "{name} {size}: D3D12 differs from the reference by {backend_gap}"
                );
                assert!(
                    (0.85..=1.08).contains(&ratio),
                    "{name} at {size}: ink ratio {ratio}"
                );
                assert!(error < 20.0, "{name} at {size}: error {error}");
            }
        }
        ran += 1;
    }
    assert!(ran > 0, "no D3D12 adapter available");
}
