use aqua_core::{Insets, Point, Rect, Size};

use aqua_style::{Absolute, LayoutStyle, Length};

pub fn round_half_toward_zero(value: f32) -> f32 {
    (value.abs() - 0.5).ceil().copysign(value)
}

pub fn round_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    round_half_toward_zero(logical * scale_factor)
}

pub fn round_stroke_to_device_pixel(logical: f32, scale_factor: f32) -> f32 {
    if logical == 0.0 {
        0.0
    } else {
        round_to_device_pixel(logical.max(0.0), scale_factor).max(1.0)
    }
}

pub fn border_to_taffy(
    widths: Insets,
    scale_factor: f32,
) -> taffy::geometry::Rect<taffy::style::LengthPercentage> {
    let edge = |width| {
        taffy::style::LengthPercentage::length(round_stroke_to_device_pixel(width, scale_factor))
    };
    taffy::geometry::Rect {
        left: edge(widths.left),
        right: edge(widths.right),
        top: edge(widths.top),
        bottom: edge(widths.bottom),
    }
}

pub fn snap_measured_size_to_device_pixels(size: Size, scale_factor: f32) -> Size {
    Size::new(
        (size.w.max(0.0) * scale_factor).ceil(),
        (size.h.max(0.0) * scale_factor).ceil(),
    )
}

pub fn snap_absolute_pair(first: f32, second: f32, scale_factor: f32) -> (f32, f32) {
    let total = first + second;
    if first < 0.0 || second < 0.0 || total == 0.0 {
        return (
            round_to_device_pixel(first, scale_factor),
            round_to_device_pixel(second, scale_factor),
        );
    }
    let total_snapped = round_to_device_pixel(total, scale_factor);
    let first_snapped = total_snapped * (first / total);
    (first_snapped, total_snapped - first_snapped)
}

pub fn snap_absolute_bounds(device_origin: Point, device_size: Size, scale_factor: f32) -> Rect {
    let left = round_half_toward_zero(device_origin.x);
    let top = round_half_toward_zero(device_origin.y);
    let right = round_half_toward_zero(device_origin.x + device_size.w);
    let bottom = round_half_toward_zero(device_origin.y + device_size.h);
    Rect::from_origin_size(
        Point::new(left / scale_factor, top / scale_factor),
        Size::new((right - left) / scale_factor, (bottom - top) / scale_factor),
    )
}

fn length(value: Length, scale_factor: f32) -> Length {
    match value {
        Length::Px(value) => Length::Px(round_to_device_pixel(value, scale_factor)),
        value => value,
    }
}

fn insets(value: Insets, scale_factor: f32) -> Insets {
    Insets {
        left: round_to_device_pixel(value.left, scale_factor),
        right: round_to_device_pixel(value.right, scale_factor),
        top: round_to_device_pixel(value.top, scale_factor),
        bottom: round_to_device_pixel(value.bottom, scale_factor),
    }
}

pub fn to_taffy(style: &LayoutStyle, scale_factor: f32) -> taffy::Style {
    let mut scaled = style.clone();
    scaled.width = length(style.width, scale_factor);
    scaled.height = length(style.height, scale_factor);
    scaled.min_width = length(style.min_width, scale_factor);
    scaled.min_height = length(style.min_height, scale_factor);
    scaled.max_width = length(style.max_width, scale_factor);
    scaled.max_height = length(style.max_height, scale_factor);
    scaled.gap = round_to_device_pixel(style.gap, scale_factor);
    scaled.column_gap = style
        .column_gap
        .map(|v| round_to_device_pixel(v, scale_factor));
    scaled.row_gap = style
        .row_gap
        .map(|v| round_to_device_pixel(v, scale_factor));
    scaled.margin = insets(style.margin, scale_factor);
    scaled.padding = insets(style.padding, scale_factor);
    if style.width == Length::Auto {
        (scaled.padding.left, scaled.padding.right) =
            snap_absolute_pair(style.padding.left, style.padding.right, scale_factor);
    }
    if style.height == Length::Auto {
        (scaled.padding.top, scaled.padding.bottom) =
            snap_absolute_pair(style.padding.top, style.padding.bottom, scale_factor);
    }
    scaled.grid_columns = style.grid_columns.as_ref().map(|tracks| {
        tracks
            .iter()
            .map(|track| length(*track, scale_factor))
            .collect()
    });
    scaled.grid_rows = style.grid_rows.as_ref().map(|tracks| {
        tracks
            .iter()
            .map(|track| length(*track, scale_factor))
            .collect()
    });
    scaled.absolute = style.absolute.map(|absolute| Absolute {
        left: absolute
            .left
            .map(|v| round_to_device_pixel(v, scale_factor)),
        right: absolute
            .right
            .map(|v| round_to_device_pixel(v, scale_factor)),
        top: absolute.top.map(|v| round_to_device_pixel(v, scale_factor)),
        bottom: absolute
            .bottom
            .map(|v| round_to_device_pixel(v, scale_factor)),
    });
    crate::mapping::to_taffy(&scaled)
}
