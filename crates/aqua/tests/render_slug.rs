#![cfg(windows)]

use aqua::{ui::Text, UiApp};
use aqua_core::scene::DisplayItem;
use aqua_core::{Oklch, Size, Transform2D};
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};

mod support;

const SIZE: Size = Size { w: 200.0, h: 120.0 };
const PHRASE: &str = "Slug Hamburgefonstiv";

fn white() -> Oklch {
    Oklch::from_hex("#ffffff").unwrap()
}

fn text_app(fonts: &SharedFonts, runs: &SharedRuns, size: f32) -> UiApp {
    let mut app = UiApp::new(fonts.clone(), runs.clone());
    app.mount(move |_| Text::new(PHRASE).font_size(size).color(white()).build())
        .unwrap();
    app.set_viewport(SIZE.w, SIZE.h, 1.0);
    app.build_frame().unwrap();
    app
}

fn under(app: &UiApp, transform: Transform2D) -> Scene {
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushTransform(transform));
    app.scene().ordered().for_each(|(_, item)| {
        scene.insert(item.clone());
    });
    scene.insert(DisplayItem::PopTransform);
    scene
}

fn rotation(radians: f32) -> Transform2D {
    let (sin, cos) = radians.sin_cos();
    Transform2D {
        a: cos,
        b: sin,
        c: -sin,
        d: cos,
        tx: 0.0,
        ty: 0.0,
    }
}

fn ink(frame: &support::Frame) -> f32 {
    frame.pixels().map(|p| f32::from(p[3]) / 255.0).sum()
}

#[test]
fn scaled_text_is_drawn_from_curves_and_carries_the_same_ink_as_the_atlas() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let direct = text_app(&fonts, &runs, 28.0);
    let small = text_app(&fonts, &runs, 14.0);
    for mut painter in support::painters(&fonts, &runs) {
        let baseline = painter.paint(direct.scene(), SIZE, 1.0);
        assert_eq!(painter.report.curves, 0, "{}", painter.label);
        let glyphs = painter.report.glyphs;
        assert!(glyphs > 0, "{}", painter.label);

        let scene = under(&small, Transform2D::scale(2.0, 2.0));
        let frame = painter.paint(&scene, SIZE, 1.0);
        assert_eq!(painter.report.curves, glyphs, "{}", painter.label);
        let (a, b) = (ink(&baseline), ink(&frame));
        assert!(a > 50.0, "{}: no text drawn", painter.label);
        assert!(
            (a - b).abs() / a < 0.12,
            "{}: atlas ink {a} versus curve ink {b}",
            painter.label
        );
    }
}

#[test]
fn rotated_text_keeps_its_ink_and_leaves_the_old_bounding_box_corners_empty() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let app = text_app(&fonts, &runs, 16.0);
    for mut painter in support::painters(&fonts, &runs) {
        let reference = painter.paint(app.scene(), SIZE, 1.0);
        let turn = Transform2D::translate(-70.0, -6.0)
            .then(rotation(0.6))
            .then(Transform2D::translate(90.0, 60.0));
        let frame = painter.paint(&under(&app, turn), SIZE, 1.0);
        assert!(painter.report.curves > 0, "{}", painter.label);
        assert_eq!(painter.report.approximated, 0, "{}", painter.label);
        let (a, b) = (ink(&reference), ink(&frame));
        assert!(
            (a - b).abs() / a < 0.15,
            "{}: upright ink {a} versus rotated ink {b}",
            painter.label
        );
        assert_eq!(frame.pixel(1, 1)[3], 0, "{}: top left", painter.label);
        assert_eq!(
            frame.pixel(198, 118)[3],
            0,
            "{}: bottom right",
            painter.label
        );
    }
}
