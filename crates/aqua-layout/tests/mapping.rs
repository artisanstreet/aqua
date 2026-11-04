use aqua_core::Insets;
use aqua_layout::mapping::{dimension, to_taffy};
use aqua_style::{px, Axis, LayoutStyle, Length, Style};
use taffy::prelude::TaffyAuto as _;
use taffy::style::{Dimension, Display, FlexDirection};

#[test]
fn flex_row_maps_to_taffy() {
    let style = LayoutStyle {
        axis: Axis::Row,
        gap: 8.0,
        width: Length::Px(100.0),
        ..Default::default()
    };
    let mapped = to_taffy(&style);
    assert_eq!(mapped.display, Display::Flex);
    assert_eq!(mapped.flex_direction, FlexDirection::Row);
    assert_eq!(mapped.size.width, Dimension::length(100.0));
}

#[test]
fn grid_tracks_map() {
    let style = LayoutStyle {
        grid_columns: Some(vec![Length::Px(40.0), Length::Auto]),
        ..Default::default()
    };
    let mapped = to_taffy(&style);
    assert_eq!(mapped.display, Display::Grid);
    assert_eq!(mapped.grid_template_columns.len(), 2);
}

#[test]
fn length_conversions() {
    assert_eq!(dimension(Length::Auto), Dimension::AUTO);
    assert_eq!(dimension(Length::Px(4.0)), Dimension::length(4.0));
    assert_eq!(dimension(Length::Pct(50.0)), Dimension::percent(0.5));
}

#[test]
fn resolved_style_maps_through_taffy() {
    let style = Style::new()
        .width(px(100.0))
        .height(px(50.0))
        .column()
        .gap(8.0)
        .padding_insets(Insets::hv(12.0, 4.0));
    let mapped = to_taffy(&style.resolve_layout());
    assert_eq!(mapped.flex_direction, FlexDirection::Column);
    assert_eq!(mapped.size.width, Dimension::length(100.0));
    assert_eq!(mapped.size.height, Dimension::length(50.0));
}
