use aqua_core::{DisplayItem, Oklch, Rect, Size};
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};

mod support;

fn rect_at(x: f32, y: f32, fill: Oklch) -> DisplayItem {
    DisplayItem::Rect {
        bounds: Rect {
            x,
            y,
            w: 14.0,
            h: 14.0,
        },
        radius: 2.0,
        fill,
        border_width: 0.0,
        border_color: fill,
        shadow: None,
    }
}

fn painters() -> Vec<support::Painter> {
    let painters = support::painters(&SharedFonts::new(), &SharedRuns::new());
    #[cfg(windows)]
    assert!(painters.len() > 1, "no D3D12 adapter available");
    painters
}

#[test]
fn offscreen_red_rect_pixel_check() {
    let red = Oklch::from_hex("#ff0000").unwrap();
    for mut painter in painters() {
        let mut scene = Scene::new();
        scene.insert(DisplayItem::Rect {
            bounds: Rect {
                x: 8.0,
                y: 8.0,
                w: 112.0,
                h: 112.0,
            },
            radius: 0.0,
            fill: red,
            border_width: 0.0,
            border_color: red,
            shadow: None,
        });
        let frame = painter.paint(&scene, Size::new(128.0, 128.0), 1.0);
        let center = frame.pixel(64, 64);
        assert!(center[0] > 200, "{}: red channel {center:?}", painter.label);
        assert!(
            center[1] < 30 && center[2] < 30,
            "{}: {center:?}",
            painter.label
        );
        assert_eq!(center[3], 255, "{}: opaque", painter.label);
        assert_eq!(
            frame.pixel(0, 0),
            [0, 0, 0, 0],
            "{}: clear outside",
            painter.label
        );
        assert_eq!(painter.report.items, 1);
        assert!(painter.report.skipped.is_empty());
    }
}

#[test]
fn offscreen_update_changes_only_the_edited_cell() {
    let red = Oklch::from_hex("#ff0000").unwrap();
    let blue = Oklch::from_hex("#0000ff").unwrap();
    for mut painter in painters() {
        let mut scene = Scene::new();
        let ids: Vec<_> = (0..64)
            .map(|i| scene.insert(rect_at((i % 8) as f32 * 16.0, (i / 8) as f32 * 16.0, red)))
            .collect();
        let before = painter.paint(&scene, Size::new(128.0, 128.0), 1.0);
        scene.update(ids[30], rect_at(6.0 * 16.0, 3.0 * 16.0, blue));
        let after = painter.paint(&scene, Size::new(128.0, 128.0), 1.0);
        let (cx, cy) = (6 * 16 + 7, 3 * 16 + 7);
        let edited = after.pixel(cx, cy);
        assert!(
            edited[2] > 200 && edited[0] < 30,
            "{}: {edited:?}",
            painter.label
        );
        let changed = before
            .pixels()
            .zip(after.pixels())
            .filter(|(old, new)| old != new)
            .count();
        assert!(
            changed > 0 && changed <= 16 * 16,
            "{}: {changed}",
            painter.label
        );
    }
}

#[test]
fn offscreen_unchanged_scene_renders_identical_pixels() {
    let red = Oklch::from_hex("#ff0000").unwrap();
    for mut painter in painters() {
        let mut scene = Scene::new();
        scene.insert(rect_at(4.0, 4.0, red));
        let first = painter.paint(&scene, Size::new(64.0, 64.0), 1.0);
        let second = painter.paint(&scene, Size::new(64.0, 64.0), 1.0);
        assert_eq!(first.rgba8, second.rgba8, "{}", painter.label);
    }
}
