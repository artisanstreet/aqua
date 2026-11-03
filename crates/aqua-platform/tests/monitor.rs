use aqua_core::Rect;
use aqua_platform::geometry::LogicalSize;
use aqua_platform::monitor::DisplayInfo;

#[test]
fn window_geometry_fits_work_area_at_monitor_scale_and_preserves_negative_origins() {
    let display = DisplayInfo {
        name: None,
        physical_bounds: Rect {
            x: -1920.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        },
        physical_work_area: Rect {
            x: -1920.0,
            y: 0.0,
            w: 1920.0,
            h: 1040.0,
        },
        scale_factor: 2.0,
    };
    let fitted = display
        .fit_window(
            LogicalSize::new(1200.0, 900.0),
            LogicalSize::new(480.0, 420.0),
        )
        .unwrap();
    assert_eq!(fitted, LogicalSize::new(960.0, 520.0));
    let centered = display.center_physical(1280, 800);
    assert_eq!(centered.x, -1600);
    assert_eq!(centered.y, 120);
    assert!(display
        .fit_window(
            LogicalSize::new(f64::NAN, 900.0),
            LogicalSize::new(480.0, 420.0)
        )
        .is_none());
}
