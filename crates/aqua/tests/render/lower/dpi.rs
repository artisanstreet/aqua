use aqua::render::lower::Lowering;
use aqua::{UiApp, ui::Text};
use aqua_core::scene::{DisplayItem, TextRasterMode, TextRun};
use aqua_core::{ColorProfile, Oklch, Point, Rect, Size, Transform2D};
use aqua_graph::{PaintCommand, SlugInstance};
use aqua_protocol::Instant;
use aqua_runtime::Runtime;
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns, TextLayout, TextStyle};

const VIEWPORT: Size = Size {
    w: 1000.0,
    h: 700.0,
};

fn run(fonts: &SharedFonts, runs: &SharedRuns, scale: f32, text: &str) -> TextRun {
    let mut layout = TextLayout::new();
    layout.set_text(
        text,
        &TextStyle {
            size_px: 16.0,
            line_height_px: 24.0,
            ..TextStyle::default()
        },
    );
    layout.set_scale(scale);
    layout.layout(&mut fonts.lock());
    let (id, revision) = runs.lock().insert(layout);
    TextRun { id: id.0, revision }
}

fn scene(run: &TextRun, origin: [f32; 2], transform: Transform2D) -> Scene {
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushTransform(transform));
    scene.insert(DisplayItem::Text {
        at: origin,
        run: run.clone(),
        color: Oklch::from_srgb8(255, 255, 255, 1.0),
        opacity: 1.0,
    });
    scene.insert(DisplayItem::PopTransform);
    scene
}

fn curves(commands: &[PaintCommand]) -> Vec<SlugInstance> {
    commands
        .iter()
        .flat_map(|command| match command {
            PaintCommand::Slug(run) => run.glyphs.to_vec(),
            _ => Vec::new(),
        })
        .collect()
}

fn raster_bounds(commands: &[PaintCommand]) -> Vec<Rect> {
    commands
        .iter()
        .flat_map(|command| match command {
            PaintCommand::Glyphs(run) => run.glyphs.iter().map(|glyph| glyph.bounds).collect(),
            _ => Vec::new(),
        })
        .collect()
}

fn near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 0.002,
        "actual {actual}, expected {expected}"
    );
}

#[test]
fn painted_text_fits_gpui_measured_bounds_at_each_device_scale() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    for scale in [1.0, 1.25, 2.0] {
        let mut app = UiApp::from_runtime(
            Runtime::new(fonts.clone(), runs.clone())
                .with_color_profile(ColorProfile::GpuiWindowsEncoded),
        );
        app.tree_mut().set_gpui_paint(true);
        app.tree_mut().set_gpui_layout_scale(Some(scale));
        app.set_viewport(VIEWPORT.w, VIEWPORT.h, scale);
        app.mount(|_| {
            aqua_ui::element::Box::new()
                .layout("row")
                .child(Text::new("MMMMMMMM").font_size(16.0).build())
                .build()
        })
        .unwrap();
        app.build_frame().unwrap();
        let (node, origin, text_run) = app
            .tree()
            .traverse()
            .into_iter()
            .find_map(|node| {
                let aqua_ui::tree::NodeKind::Text(_) = &app.tree().get(node)?.kind else {
                    return None;
                };
                app.scene().ordered().find_map(|(_, item)| match item {
                    DisplayItem::Text { at, run, .. } => Some((node, *at, run.clone())),
                    _ => None,
                })
            })
            .expect("the measured text must emit a paint run");
        let measured = app.tree().visual_rect(node).unwrap();
        let mut lowering = Lowering::new(fonts.clone(), runs.clone());
        lowering.set_text_raster(TextRasterMode::Outline);
        let (commands, _, report) = lowering.commands(app.scene(), VIEWPORT, scale, Instant::ZERO);
        let painted = curves(&commands);
        assert_eq!(painted.len(), 8, "all measured glyphs must be painted");
        assert_eq!(
            report.curves, 8,
            "outline mode must exercise curve geometry"
        );
        let guard = runs.lock();
        let layout = guard
            .resolve(aqua_text::runs::RunId(text_run.id))
            .unwrap()
            .layout;
        let laid = layout.laid_out().unwrap();
        assert!(
            (laid.size.0 / scale - measured.w).abs() <= 1.0,
            "shaped logical width must agree with GPUI measurement"
        );
        for (painted, glyph) in painted
            .iter()
            .zip(laid.lines.iter().flat_map(|line| &line.glyphs))
        {
            near(painted.pixels_per_em(), 16.0 * scale);
            near(
                painted.origin[0],
                origin[0] * scale + glyph.x + glyph.x_offset,
            );
            let right = painted.em_to_screen([painted.em_bounds[2], painted.em_bounds[1]])[0];
            assert!(
                right <= (measured.right() + 1.0) * scale,
                "painted ink must fit the measured device width"
            );
        }
    }
}

#[test]
fn curve_run_scale_is_independent_of_display_scale_and_scene_transform() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let angle = 0.2_f32;
    let transform = Transform2D {
        a: angle.cos() * 1.5,
        b: angle.sin() * 1.5,
        c: -angle.sin() * 1.5,
        d: angle.cos() * 1.5,
        tx: 7.0,
        ty: 9.0,
    };
    let origin = [17.0, 13.0];
    for run_scale in [1.0, 1.25, 2.0] {
        let text_run = run(&fonts, &runs, run_scale, "MMM");
        let scene = scene(&text_run, origin, transform);
        for display_scale in [1.0, 1.25, 2.0] {
            let mut lowering = Lowering::new(fonts.clone(), runs.clone());
            let (commands, _, report) =
                lowering.commands(&scene, VIEWPORT, display_scale, Instant::ZERO);
            let painted = curves(&commands);
            assert_eq!(
                report.curves, 3,
                "scene transforms must exercise transformed curves"
            );
            let guard = runs.lock();
            let layout = guard
                .resolve(aqua_text::runs::RunId(text_run.id))
                .unwrap()
                .layout;
            let expected = layout
                .laid_out()
                .unwrap()
                .lines
                .iter()
                .flat_map(|line| line.glyphs.iter().map(move |glyph| (line, glyph)));
            assert_eq!(painted.len(), 3, "all transformed glyphs must be painted");
            for (painted, (line, glyph)) in painted.iter().zip(expected) {
                let pen = transform.apply(Point::new(
                    origin[0] + (glyph.x + glyph.x_offset) / run_scale,
                    origin[1] + (line.baseline_y + glyph.y - glyph.y_offset) / run_scale,
                ));
                near(painted.origin[0], pen.x * display_scale);
                near(painted.origin[1], pen.y * display_scale);
                near(painted.pixels_per_em(), 16.0 * 1.5 * display_scale);
            }
        }
    }
}

#[test]
fn raster_runs_match_logical_reference_after_cached_translation_and_display_change() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let reference = run(&fonts, &runs, 1.0, "MMMM");
    for run_scale in [1.0, 1.25, 2.0] {
        let text_run = run(&fonts, &runs, run_scale, "MMMM");
        let mut warm = Lowering::new(fonts.clone(), runs.clone());
        for display_scale in [1.0, 1.25, 2.0, 1.25] {
            for offset in [0.0, 3.3, 14.4, 0.0] {
                let transform = Transform2D::translate(0.0, offset);
                let actual = scene(&text_run, [17.0, 13.0], transform);
                let reference = scene(&reference, [17.0, 13.0], transform);
                let (commands, _, report) =
                    warm.commands(&actual, VIEWPORT, display_scale, Instant::ZERO);
                assert_eq!(
                    report.curves, 0,
                    "DPI normalization must preserve the raster path"
                );
                let mut fresh = Lowering::new(fonts.clone(), runs.clone());
                let (expected, _, _) =
                    fresh.commands(&reference, VIEWPORT, display_scale, Instant::ZERO);
                let actual_bounds = raster_bounds(&commands);
                let expected_bounds = raster_bounds(&expected);
                assert_eq!(actual_bounds.len(), 4, "all raster glyphs must be painted");
                assert_eq!(expected_bounds.len(), actual_bounds.len());
                for (actual, expected) in actual_bounds.iter().zip(&expected_bounds) {
                    assert_eq!(
                        (actual.w, actual.h),
                        (expected.w, expected.h),
                        "raster size must depend on display DPI, not run DPI"
                    );
                    assert!(
                        (actual.x - expected.x).abs() <= 1.0
                            && (actual.y - expected.y).abs() <= 1.0,
                        "cached raster placement {actual:?} must match logical reference {expected:?}"
                    );
                }
                warm.retain_frame(commands);
            }
        }
    }
}

#[test]
fn text_culling_uses_logical_run_extent_at_device_scale() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let text_run = run(&fonts, &runs, 2.0, "MMMMMMMMMMMM");
    let width = runs
        .lock()
        .resolve(aqua_text::runs::RunId(text_run.id))
        .unwrap()
        .layout
        .laid_out()
        .unwrap()
        .size
        .0
        / 2.0;
    let scene = scene(&text_run, [0.0, 0.0], Transform2D::IDENTITY);
    for display_scale in [1.0, 1.25, 2.0] {
        let mut lowering = Lowering::new(fonts.clone(), runs.clone());
        lowering.set_cull(Some(Rect {
            x: width * 1.6 * display_scale,
            y: 0.0,
            w: 10.0,
            h: 100.0,
        }));
        let (_, _, report) = lowering.commands(&scene, VIEWPORT, display_scale, Instant::ZERO);
        assert_eq!(
            report.glyphs, 0,
            "text beyond its normalized extent must be culled before glyph work"
        );
        lowering.set_cull(Some(Rect {
            x: (width - 20.0) * display_scale,
            y: 0.0,
            w: 20.0 * display_scale,
            h: 100.0,
        }));
        let (_, _, report) = lowering.commands(&scene, VIEWPORT, display_scale, Instant::ZERO);
        assert!(
            report.glyphs > 0,
            "normalized culling must retain the visible tail of the text"
        );
    }
}
