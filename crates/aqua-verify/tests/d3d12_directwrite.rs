#![cfg(windows)]

use aqua_core::Oklch;
use aqua_gpu_d3d12::hal::tier::Tier;
use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat, TierLimit};
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
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|texel| {
            [0, 1, 2].map(|channel| {
                (linear_to_srgb(f32::from(texel[channel]) / 255.0) * 255.0).round() as u8
            })
        })
        .collect()
}

fn ink(pixels: &[u8], background: [u8; 3]) -> f64 {
    pixels
        .as_chunks::<3>()
        .0
        .iter()
        .map(|pixel| {
            (0..3)
                .map(|channel| (f64::from(pixel[channel]) - f64::from(background[channel])).abs())
                .sum::<f64>()
        })
        .sum()
}

fn mean_abs(a: &[u8], b: &[u8], background: [u8; 3]) -> f64 {
    let (mut total, mut count) = (0.0f64, 0.0f64);
    for (pa, pb) in a.as_chunks::<3>().0.iter().zip(b.as_chunks::<3>().0.iter()) {
        if *pa != background || *pb != background {
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

const COMBINATIONS: [(&str, TierLimit); 4] = [
    ("baseline", TierLimit::BASELINE),
    (
        "bindless",
        TierLimit {
            binding: Tier::Bindless,
            enhanced_barriers: false,
        },
    ),
    (
        "enhanced-barriers",
        TierLimit {
            binding: Tier::Baseline,
            enhanced_barriers: true,
        },
    ),
    ("bindless+enhanced-barriers", TierLimit::MAX),
];

const CASES: [(&str, [u8; 3], [u8; 3]); 3] = [
    ("black_on_white", [0; 3], [255; 3]),
    ("white_on_black", [255; 3], [0; 3]),
    ("navy_on_cream", [20, 30, 80], [250, 245, 235]),
];

const SIZES: [f32; 3] = [13.0, 18.0, 28.0];

struct Measurement {
    ratio: f64,
    error: f64,
    reference_gap: u8,
    baseline_gap: u8,
}

fn measure_adapter(preference: AdapterPreference) -> Option<usize> {
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
    let mut devices: Vec<(&str, OffscreenBackend)> = COMBINATIONS
        .into_iter()
        .filter_map(|(name, limit)| {
            OffscreenBackend::with_tiers(preference, SurfaceFormat::Rgba16Float, limit)
                .ok()
                .map(|device| (name, device))
        })
        .collect();
    if devices.is_empty() {
        eprintln!("SKIP-NO-ADAPTER {preference:?}");
        return None;
    }
    let mut reference = RefBackend::new();
    for (name, device) in &devices {
        let tiers = device.tiers();
        eprintln!(
            "dwrite {preference:?} {name}: binding {:?} enhanced barriers {}",
            tiers.binding, tiers.enhanced_barriers
        );
    }
    let mut failures = Vec::new();
    for (case, fg, bg) in CASES {
        for size in SIZES {
            let mut results: Vec<Measurement> = Vec::new();
            for text in STRINGS {
                let canvas = (1100u32, (size * 2.4) as u32);
                let origin = (8, (size * 1.3).round() as i32);
                let mut resolver = GlyphResolver::new();
                let mut residency = Residency::new(ResidencyConfig::default());
                residency.begin_frame();
                let laid = scene(
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
                devices
                    .iter_mut()
                    .for_each(|(_, device)| device.upload(uploads.clone()).unwrap());
                let frame = plan(laid.instances, canvas, hex(bg));
                let expected = conformance::render(&mut reference, &frame).unwrap();
                let native = oracle
                    .blended(
                        GlyphRunSpec {
                            font: source,
                            size_px: size,
                            glyphs: &laid.ids,
                            advances: &laid.advances,
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
                let mut baseline: Option<SurfacePixels> = None;
                for (name, device) in &mut devices {
                    let actual = conformance::render(device, &frame).unwrap();
                    let encoded = encoded_rgb(&actual);
                    let rendered_bg = [encoded[0], encoded[1], encoded[2]];
                    let measurement = Measurement {
                        ratio: ink(&encoded, rendered_bg) / ink(&native.pixels, bg),
                        error: mean_abs(&encoded, &native.pixels, rendered_bg),
                        reference_gap: max_difference(&expected, &actual),
                        baseline_gap: baseline
                            .as_ref()
                            .map(|base| max_difference(base, &actual))
                            .unwrap_or(0),
                    };
                    if baseline.is_none() {
                        baseline = Some(actual);
                    }
                    let label = format!("{preference:?} {name} {case} {size} {text:?}");
                    if !(0.80..=1.12).contains(&measurement.ratio) {
                        failures.push(format!("{label}: ink ratio {}", measurement.ratio));
                    }
                    if measurement.error >= 24.0 {
                        failures.push(format!("{label}: error {}", measurement.error));
                    }
                    if measurement.reference_gap > 3 {
                        failures.push(format!(
                            "{label}: {} from the reference",
                            measurement.reference_gap
                        ));
                    }
                    if measurement.baseline_gap > 2 {
                        failures.push(format!(
                            "{label}: {} from the baseline tier",
                            measurement.baseline_gap
                        ));
                    }
                    results.push(measurement);
                }
            }
            let tiers = devices.len();
            for (index, (name, _)) in devices.iter().enumerate() {
                let own: Vec<&Measurement> = results.iter().skip(index).step_by(tiers).collect();
                let n = own.len() as f64;
                eprintln!(
                    "dwrite {preference:?} {name} {case} size={size} ink_ratio={:.4} error={:.3}",
                    own.iter().map(|m| m.ratio).sum::<f64>() / n,
                    own.iter().map(|m| m.error).sum::<f64>() / n
                );
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Some(devices.len())
}

#[test]
fn warp_glyphs_on_every_tier_track_directwrite() {
    assert!(measure_adapter(AdapterPreference::Warp).unwrap() > 0);
}

#[test]
fn hardware_glyphs_on_every_tier_track_directwrite() {
    measure_adapter(AdapterPreference::HighPerformance);
}

#[test]
fn integrated_glyphs_on_every_tier_track_directwrite() {
    measure_adapter(AdapterPreference::MinimumPower);
}
