use aqua_core::geometry::{snap_device_pixel, snap_device_stroke};

#[test]
fn device_snapping_matches_gpui_ties_and_strokes() {
    for (value, scale, expected) in [
        (1.0, 1.5, 1.0),
        (-1.0, 1.5, -1.0),
        (1.5001, 1.0, 2.0),
        (-1.5001, 1.0, -2.0),
        (0.3, 2.0, 1.0),
        (2.0, 2.0, 4.0),
    ] {
        assert_eq!(snap_device_pixel(value, scale), expected);
    }
    for (value, scale, expected) in [
        (0.0, 1.0, 0.0),
        (0.4, 1.0, 1.0),
        (0.5, 1.0, 1.0),
        (1.0, 1.5, 1.0),
        (1.6, 1.0, 2.0),
        (-0.4, 1.0, 1.0),
    ] {
        assert_eq!(snap_device_stroke(value, scale), expected);
    }
}
