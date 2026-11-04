use std::time::Duration;

use aqua::render::transition::{Settings, Sweep};
use aqua_core::{DisplayItem, Oklch, Rect, Size};
use aqua_protocol::Instant;
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};

mod support;

const SIZE: Size = Size { w: 128.0, h: 128.0 };

fn solid(hex: &str) -> Scene {
    let fill = Oklch::from_hex(hex).unwrap();
    let mut scene = Scene::new();
    scene.insert(DisplayItem::Rect {
        bounds: Rect {
            x: 0.0,
            y: 0.0,
            w: SIZE.w,
            h: SIZE.h,
        },
        radius: 0.0,
        fill,
        border_width: 0.0,
        border_color: fill,
        shadow: None,
    });
    scene
}

fn settings(sweep: Sweep, tint_alpha: f32) -> Settings {
    Settings {
        duration: Duration::from_secs(1),
        band: 0.4,
        tint: [1.0, 1.0, 1.0, tint_alpha],
        sweep,
    }
}

fn is(pixel: [u8; 4], expected: [u8; 3]) -> bool {
    pixel[3] == 255
        && pixel[..3]
            .iter()
            .zip(expected)
            .all(|(got, want)| got.abs_diff(want) <= 3)
}

fn painters() -> Vec<support::Painter> {
    let painters = support::painters(&SharedFonts::new(), &SharedRuns::new());
    #[cfg(windows)]
    assert!(painters.len() > 1, "no D3D12 adapter available");
    painters
}

fn at(seconds: f64) -> Instant {
    Instant::from_secs_f64(seconds)
}

#[test]
fn a_snapshot_transition_starts_on_the_old_frame_sweeps_diagonally_and_ends_on_the_new_one() {
    let red = solid("#ff0000");
    let blue = solid("#0000ff");
    let mut mid_frames = Vec::new();
    for mut painter in painters() {
        let label = painter.label.clone();
        painter.paint_at(&red, SIZE, 1.0, at(9.0));
        painter.begin_transition(settings(Sweep::Soft, 0.0));

        let first = painter.paint_at(&blue, SIZE, 1.0, at(10.0));
        assert!(painter.report.shimmering, "{label}");
        assert!(is(first.pixel(4, 4), [255, 0, 0]), "{label}: {:?}", first.pixel(4, 4));
        assert!(is(first.pixel(120, 120), [255, 0, 0]), "{label}");

        let mid = painter.paint_at(&blue, SIZE, 1.0, at(10.5));
        assert!(is(mid.pixel(4, 4), [0, 0, 255]), "{label}: {:?}", mid.pixel(4, 4));
        assert!(is(mid.pixel(124, 124), [255, 0, 0]), "{label}: {:?}", mid.pixel(124, 124));
        let seam = mid.pixel(64, 64);
        assert!(seam[0] > 20 && seam[2] > 20, "{label}: blended seam {seam:?}");
        mid_frames.push((label.clone(), mid));

        let done = painter.paint_at(&blue, SIZE, 1.0, at(11.0));
        assert!(!painter.report.shimmering, "{label}");
        assert!(is(done.pixel(4, 4), [0, 0, 255]), "{label}");
        assert!(is(done.pixel(124, 124), [0, 0, 255]), "{label}");
    }
    let (first_label, first) = &mid_frames[0];
    for (label, frame) in &mid_frames[1..] {
        let worst = first
            .rgba8
            .iter()
            .zip(&frame.rgba8)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= 6, "{label} differs from {first_label} by {worst}");
    }
}

#[test]
fn the_shimmer_tint_peaks_at_the_sweep_edge() {
    let red = solid("#ff0000");
    let blue = solid("#0000ff");
    for mut painter in painters() {
        let label = painter.label.clone();
        painter.paint_at(&red, SIZE, 1.0, at(0.0));
        painter.begin_transition(settings(Sweep::Soft, 1.0));
        painter.paint_at(&blue, SIZE, 1.0, at(1.0));
        let mid = painter.paint_at(&blue, SIZE, 1.0, at(1.5));
        let seam = mid.pixel(64, 64);
        assert!(seam[0] > 235 && seam[1] > 235 && seam[2] > 235, "{label}: {seam:?}");
        assert!(is(mid.pixel(4, 4), [0, 0, 255]), "{label}");
    }
}

#[test]
fn the_eased_sweep_is_slower_than_linear_at_the_start_and_lands_exactly() {
    let eased = settings(Sweep::Eased, 0.0);
    let soft = settings(Sweep::Soft, 0.0);
    let early = Duration::from_millis(200);
    assert!(aqua::render::transition::progress(&eased, early) < 0.2);
    assert!(
        (aqua::render::transition::progress(&soft, early) - 0.2).abs() < 1.0e-6
    );
    assert_eq!(aqua::render::transition::progress(&eased, Duration::from_secs(2)), 1.0);
    let steps: Vec<f32> = (0..=20)
        .map(|i| aqua::render::transition::progress(&eased, Duration::from_millis(i * 50)))
        .collect();
    assert!(steps.windows(2).all(|pair| pair[1] >= pair[0]));
}

#[test]
fn a_transition_with_a_bad_band_or_duration_is_rejected() {
    use aqua::render::transition::Invalid;
    use aqua_core::scene::SnapshotTransition;
    let base = SnapshotTransition {
        duration: Duration::from_millis(300),
        band: 0.3,
        shimmer: Oklch::from_hex("#ffffff").unwrap(),
    };
    assert!(Settings::plain(base).is_ok());
    assert_eq!(
        Settings::plain(SnapshotTransition {
            duration: Duration::ZERO,
            ..base
        })
        .unwrap_err(),
        Invalid::Duration
    );
    assert_eq!(
        Settings::plain(SnapshotTransition { band: 0.0, ..base }).unwrap_err(),
        Invalid::Band
    );
    assert_eq!(
        Settings::plain(SnapshotTransition { band: 1.5, ..base }).unwrap_err(),
        Invalid::Band
    );
}

#[cfg(windows)]
#[test]
fn outline_text_mode_draws_every_glyph_from_curves_and_platform_default_uses_the_atlas() {
    use aqua::{ui::Text, UiApp};
    use aqua_core::scene::TextRasterMode;
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut app = UiApp::new(fonts.clone(), runs.clone());
    app.mount(|_| {
        Text::new("Outline Hamburgefonstiv")
            .font_size(16.0)
            .color(Oklch::from_hex("#ffffff").unwrap())
            .build()
    })
    .unwrap();
    app.set_viewport(SIZE.w, SIZE.h, 1.0);
    app.build_frame().unwrap();
    for mut painter in support::painters(&fonts, &runs) {
        let label = painter.label.clone();
        painter.paint(app.scene(), SIZE, 1.0);
        assert_eq!(painter.report.curves, 0, "{label}");
        let glyphs = painter.report.glyphs;
        assert!(glyphs > 0, "{label}");

        painter.set_text_raster(TextRasterMode::Outline);
        let outlined = painter.paint(app.scene(), SIZE, 1.0);
        assert_eq!(painter.report.curves, glyphs, "{label}");
        let ink: f32 = outlined.pixels().map(|p| f32::from(p[3]) / 255.0).sum();
        assert!(ink > 30.0, "{label}: {ink}");

        painter.set_text_raster(TextRasterMode::PlatformDefault);
        painter.paint(app.scene(), SIZE, 1.0);
        assert_eq!(painter.report.curves, 0, "{label}");
    }
}
