use aqua_core::{Insets, Point, Rect, Size};
use aqua_layout::editor::{
    round_half_toward_zero, round_stroke_to_device_pixel, snap_absolute_bounds, snap_absolute_pair,
    snap_measured_size_to_device_pixels, to_taffy,
};
use aqua_style::{LayoutStyle, Length};
use taffy::style::LengthPercentage;

#[test]
fn editor_style_scales_absolute_lengths_and_preserves_percentages() {
    let style = LayoutStyle {
        width: Length::Pct(50.0),
        height: Length::Px(1.0),
        gap: 3.0,
        ..LayoutStyle::default()
    };
    let converted = to_taffy(&style, 1.5);
    assert_eq!(converted.size.width, taffy::style::Dimension::percent(0.5));
    assert_eq!(converted.size.height, taffy::style::Dimension::length(1.0));
    assert_eq!(converted.gap.width, LengthPercentage::length(4.0));
}

#[test]
fn gpui_rounds_half_pixels_toward_zero_and_keeps_nonzero_strokes() {
    for (input, expected) in [
        (0.5, 0.0),
        (1.5, 1.0),
        (2.5, 2.0),
        (-0.5, 0.0),
        (-1.5, -1.0),
        (1.5001, 2.0),
        (-1.5001, -2.0),
    ] {
        assert_eq!(round_half_toward_zero(input), expected);
    }
    assert_eq!(round_stroke_to_device_pixel(0.0, 1.0), 0.0);
    assert_eq!(round_stroke_to_device_pixel(0.4, 1.0), 1.0);
    assert_eq!(round_stroke_to_device_pixel(1.0, 1.5), 1.0);
}

#[test]
fn gpui_measured_sizes_ceil_nonnegative_dimensions_in_device_space() {
    assert_eq!(
        snap_measured_size_to_device_pixels(Size::new(10.1, -2.0), 1.5),
        Size::new(16.0, 0.0)
    );
    assert_eq!(
        snap_measured_size_to_device_pixels(Size::new(4.0, 1.0), 2.0),
        Size::new(8.0, 2.0)
    );
}

#[test]
fn gpui_proportionally_snaps_padding_on_auto_axes() {
    let mut style = LayoutStyle {
        padding: Insets {
            top: 6.5,
            right: 5.0,
            bottom: 6.5,
            left: 2.5,
        },
        ..LayoutStyle::default()
    };
    let padding = to_taffy(&style, 1.0).padding;
    let left = 7.0 * (2.5 / 7.5);
    assert_eq!(padding.left, LengthPercentage::length(left));
    assert_eq!(padding.right, LengthPercentage::length(7.0 - left));
    assert_eq!(padding.top, LengthPercentage::length(6.5));
    assert_eq!(padding.bottom, LengthPercentage::length(6.5));
    style.width = Length::Px(100.0);
    let padding = to_taffy(&style, 1.0).padding;
    assert_eq!(padding.left, LengthPercentage::length(2.0));
    assert_eq!(padding.right, LengthPercentage::length(5.0));
    style.width = Length::Auto;
    style.padding.top = 6.75;
    style.padding.bottom = 6.75;
    let padding = to_taffy(&style, 2.0).padding;
    assert_eq!(padding.top, LengthPercentage::length(13.5));
    assert_eq!(padding.bottom, LengthPercentage::length(13.5));
    assert_eq!(snap_absolute_pair(-0.5, 2.5, 1.0), (-0.0, 2.0));
}

#[test]
fn shared_edges_snap_in_absolute_device_coordinates() {
    let left = snap_absolute_bounds(Point::new(0.25, 0.0), Size::new(5.25, 3.0), 2.0);
    let right = snap_absolute_bounds(Point::new(5.5, 0.0), Size::new(4.0, 3.0), 2.0);
    assert_eq!(
        left,
        Rect::from_origin_size(Point::new(0.0, 0.0), Size::new(2.5, 1.5))
    );
    assert_eq!(
        right,
        Rect::from_origin_size(Point::new(2.5, 0.0), Size::new(2.0, 1.5))
    );
    assert_eq!(left.x + left.w, right.x);
    let nested_raw_origin = Point::new(0.4 + 0.4, 0.0);
    let child = snap_absolute_bounds(nested_raw_origin, Size::new(1.0, 1.0), 1.0);
    assert_eq!(child.x, 1.0);
}
