use std::time::Instant;

use aqua_gpu_ref::conformance::assets::standard_uploads;
use aqua_gpu_ref::conformance::cases::hex;
use aqua_gpu_ref::conformance::{Case, Session};
use aqua_gpu_ref::RefBackend;
use aqua_graph::{plan_paint, Extent, PaintCommand};
use aqua_kernels::{diff, over};
use aqua_text::raster::{place_glyph, Rasterizer};
use aqua_text::{FontManager, LineMode, TextLayout, TextStyle, WrapMode};
use aqua_verify::stats::Distribution;
use aqua_verify::workload::editor_workload;

const SURFACE: Extent = Extent {
    width: 2560,
    height: 1440,
};

fn millis(work: impl FnOnce()) -> f64 {
    let started = Instant::now();
    work();
    started.elapsed().as_secs_f64() * 1000.0
}

fn best_of(repeats: usize, mut work: impl FnMut()) -> f64 {
    (0..repeats)
        .map(|_| millis(&mut work))
        .fold(f64::INFINITY, f64::min)
}

#[test]
#[ignore]
fn hot_path_profile() {
    let commands = editor_workload(SURFACE);
    type Category = (&'static str, fn(&PaintCommand) -> bool);
    let categories: [Category; 4] = [
        ("quads", |c| matches!(c, PaintCommand::Quad(_))),
        ("shadows", |c| matches!(c, PaintCommand::Shadow(_))),
        ("glyphs", |c| matches!(c, PaintCommand::Glyphs(_))),
        ("backdrop_blur", |c| {
            matches!(
                c,
                PaintCommand::BackdropBlur(_)
                    | PaintCommand::PushOpacity(_)
                    | PaintCommand::PopOpacity
            )
        }),
    ];
    for (name, keep) in categories {
        let subset: Vec<PaintCommand> = commands
            .iter()
            .enumerate()
            .filter(|(index, command)| *index == 0 || keep(command))
            .map(|(_, command)| command.clone())
            .collect();
        let plan = plan_paint(SURFACE, hex("#000000", 1.0), &subset).unwrap();
        let case = Case {
            name: name.into(),
            plan,
            uploads: standard_uploads(),
        };
        let mut reference = RefBackend::new();
        let mut session = Session::new(&mut reference);
        let ms = millis(|| {
            session.render_case(&case).unwrap();
        });
        println!(
            "profile oracle_category name={name} instances={} ms={ms:.0}",
            case.plan.instance_count()
        );
    }
    let pixels = (SURFACE.width * SURFACE.height) as usize;
    let source: Vec<[f32; 4]> = (0..pixels)
        .map(|i| {
            [
                (i % 7) as f32 / 14.0,
                (i % 5) as f32 / 10.0,
                (i % 3) as f32 / 6.0,
                0.5,
            ]
        })
        .collect();
    let mut destination = vec![[0.1f32, 0.2, 0.3, 1.0]; pixels];
    let over_reference = best_of(5, || over::reference(&mut destination, &source, 0.8));
    let over_simd = best_of(5, || over::apply(&mut destination, &source, 0.8));
    println!(
        "profile layer_over_1440p_rgba32f reference_ms={over_reference:.2} simd_ms={over_simd:.2}"
    );

    let a: Vec<u8> = (0..pixels * 4).map(|i| (i % 251) as u8).collect();
    let b: Vec<u8> = (0..pixels * 4).map(|i| (i % 241) as u8).collect();
    let diff_reference = best_of(5, || {
        diff::reference(&a, &b);
    });
    let diff_simd = best_of(5, || {
        diff::max_abs_diff(&a, &b);
    });
    println!(
        "profile frame_diff_1440p_rgba8 reference_ms={diff_reference:.2} simd_ms={diff_simd:.2}"
    );

    let mut fonts = FontManager::new();
    let text: String = (32u8..127).map(char::from).collect();
    let mut rasters = 0usize;
    let mut raster_ms = Vec::new();
    for size in [12.0f32, 13.0, 14.0, 16.0, 20.0] {
        let mut layout = TextLayout::new();
        layout.set_text(
            &text,
            &TextStyle {
                size_px: size,
                ..TextStyle::default()
            },
        );
        let laid = layout.layout(&mut fonts);
        let mut rasterizer = Rasterizer::new();
        for glyph in &laid.lines[0].glyphs {
            for subpixel in 0..4u8 {
                let mut placement = place_glyph(glyph, 0.0, laid.lines[0].baseline_y, true);
                placement.key.subpixel = subpixel;
                raster_ms.push(millis(|| {
                    rasterizer.rasterize(&mut fonts, &placement.key);
                }));
                rasters += 1;
            }
        }
    }
    println!(
        "profile glyph_raster count={rasters} per_glyph_ms {}",
        Distribution::from_samples(&raster_ms).unwrap()
    );
    println!(
        "profile glyph_raster total_ms={:.1}",
        raster_ms.iter().sum::<f64>()
    );

    let line = "fn layout(&mut self, fonts: &mut FontManager) -> Arc<LaidOut> { let value = 42; // shaped text }";
    let document = (0..2000)
        .map(|i| format!("{i}: {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut layout = TextLayout::new();
    layout.set_line_mode(LineMode::Multi);
    layout.set_wrap(WrapMode::WordOrGlyph);
    layout.set_text(&document, &TextStyle::default());
    layout.set_max_width(Some(600.0));
    let cold = millis(|| {
        layout.layout(&mut fonts);
    });
    layout.set_max_width(Some(500.0));
    let rewrap = millis(|| {
        layout.layout(&mut fonts);
    });
    println!("profile shaping_2000_paragraphs cold_ms={cold:.1} rewrap_ms={rewrap:.1}");
}
