use aqua_core::{Insets, Point, Rect, Size};
use aqua_ui::layout::editor::{
    round_half_toward_zero, round_stroke_to_device_pixel, snap_absolute_bounds, snap_absolute_pair,
    snap_measured_size_to_device_pixels, to_taffy,
};
use aqua_ui::style::{LayoutStyle, Length};
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

#[test]
fn tree_opt_in_recomputes_on_scale_change_and_restores_legacy_layout() {
    use aqua_ui::layout::NaiveMeasurer;
    use aqua_ui::tree::{BoxContent, NodeKind, NodeSpec, UiTree};
    let mut tree = UiTree::new();
    let node = tree.mount_spec(NodeSpec::new(
        NodeKind::Box(BoxContent::default()),
        LayoutStyle {
            width: Length::Px(1.0),
            height: Length::Px(1.0),
            ..LayoutStyle::default()
        },
    ));
    let viewport = Size::new(100.0, 100.0);
    tree.compute_layout(viewport, &NaiveMeasurer).unwrap();
    assert_eq!(tree.rect(node).unwrap().size(), Size::new(1.0, 1.0));
    assert!(tree.set_gpui_layout_scale(Some(1.5)));
    tree.compute_layout(viewport, &NaiveMeasurer).unwrap();
    assert_eq!(
        tree.rect(node).unwrap().size(),
        Size::new(1.0 / 1.5, 1.0 / 1.5)
    );
    tree.compute_layout(viewport, &NaiveMeasurer).unwrap();
    assert!(tree.last_stats().layout_skipped);
    assert!(tree.set_gpui_layout_scale(Some(2.0)));
    tree.compute_layout(viewport, &NaiveMeasurer).unwrap();
    assert!(!tree.last_stats().layout_skipped);
    assert_eq!(tree.rect(node).unwrap().size(), Size::new(1.0, 1.0));
    assert!(!tree.set_gpui_layout_scale(Some(0.0)));
    assert!(tree.set_gpui_layout_scale(None));
    tree.compute_layout(viewport, &NaiveMeasurer).unwrap();
    assert_eq!(tree.rect(node).unwrap().size(), Size::new(1.0, 1.0));
}

#[test]
fn tree_opt_in_preserves_shared_flex_edges() {
    use aqua_ui::layout::NaiveMeasurer;
    use aqua_ui::tree::{BoxContent, NodeKind, NodeSpec, UiTree};
    let child = || {
        NodeSpec::new(
            NodeKind::Box(BoxContent::default()),
            LayoutStyle {
                flex_grow: 1.0,
                ..LayoutStyle::default()
            },
        )
    };
    let mut tree = UiTree::new();
    let root = tree.mount_spec(
        NodeSpec::new(
            NodeKind::Box(BoxContent::default()),
            LayoutStyle {
                width: Length::Px(5.0),
                height: Length::Px(1.0),
                ..LayoutStyle::default()
            },
        )
        .child(child())
        .child(child())
        .child(child()),
    );
    let children = tree.get(root).unwrap().children.clone();
    tree.set_gpui_layout_scale(Some(1.0));
    tree.compute_layout(Size::new(20.0, 20.0), &NaiveMeasurer)
        .unwrap();
    let rects: Vec<_> = children.iter().map(|id| tree.rect(*id).unwrap()).collect();
    assert_eq!(
        rects
            .iter()
            .map(|rect| (rect.x, rect.w))
            .collect::<Vec<_>>(),
        vec![(0.0, 2.0), (2.0, 1.0), (3.0, 2.0)]
    );
}

#[test]
fn tree_opt_in_ceil_snaps_measured_text_in_device_pixels() {
    use aqua_ui::layout::TextMeasurer;
    use aqua_ui::tree::{NodeKind, NodeSpec, TextContent, UiTree};
    struct Fixed;
    impl TextMeasurer for Fixed {
        fn measure_text(&self, _: &str, _: f32, _: Option<f32>, _: bool) -> Size {
            Size::new(1.2, 2.1)
        }
    }
    let mut tree = UiTree::new();
    let node = tree.mount_spec(NodeSpec::new(
        NodeKind::Text(TextContent::label("x")),
        LayoutStyle::default(),
    ));
    tree.set_gpui_layout_scale(Some(2.0));
    tree.compute_layout(Size::new(100.0, 100.0), &Fixed)
        .unwrap();
    assert_eq!(tree.rect(node).unwrap().size(), Size::new(1.5, 2.5));
}

#[test]
fn tree_opt_in_border_changes_invalidate_layout() {
    use aqua_core::Oklch;
    use aqua_ui::layout::NaiveMeasurer;
    use aqua_ui::style::Border;
    use aqua_ui::tree::{BoxContent, NodeKind, NodeSpec, UiTree};
    let mut tree = UiTree::new();
    let node = tree.mount_spec(NodeSpec::new(
        NodeKind::Box(BoxContent::default()),
        LayoutStyle::default(),
    ));
    tree.set_gpui_layout_scale(Some(1.5));
    let viewport = Size::new(100.0, 100.0);
    tree.compute_layout(viewport, &NaiveMeasurer).unwrap();
    let mut paint = tree.paint(node).unwrap();
    paint.border = Some(Border {
        width: 1.0,
        color: Oklch::WHITE,
    });
    assert!(tree.set_paint(node, paint));
    tree.compute_layout(viewport, &NaiveMeasurer).unwrap();
    assert!(!tree.last_stats().layout_skipped);
    assert_eq!(
        tree.rect(node).unwrap().size(),
        Size::new(2.0 / 1.5, 2.0 / 1.5)
    );
}

#[test]
fn source_border_widths_place_children_and_width_changes_relayout() {
    use aqua_core::GpuiHsla;
    use aqua_ui::layout::NaiveMeasurer;
    use aqua_ui::style::Style;
    use aqua_ui::tree::{BoxContent, NodeKind, NodeSpec, UiTree};
    let color = GpuiHsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a: 1.0,
    };
    let mut tree = UiTree::new();
    let paint = Style::new()
        .gpui_border(
            Insets {
                left: 3.0,
                top: 2.0,
                right: 1.0,
                bottom: 1.0,
            },
            color,
            false,
        )
        .resolve_paint();
    let root = NodeSpec::new(
        NodeKind::Box(BoxContent { paint }),
        LayoutStyle {
            width: Length::Px(30.0),
            height: Length::Px(20.0),
            ..LayoutStyle::default()
        },
    );
    let node = tree.mount_spec(root.child(NodeSpec::new(
        NodeKind::Box(BoxContent::default()),
        LayoutStyle {
            width: Length::Px(4.0),
            height: Length::Px(4.0),
            ..LayoutStyle::default()
        },
    )));
    let child = tree.get(node).unwrap().children[0];
    tree.set_gpui_layout_scale(Some(1.0));
    let viewport = Size::new(100.0, 100.0);
    tree.compute_layout(viewport, &NaiveMeasurer).unwrap();
    assert_eq!(tree.rect(child).unwrap().origin(), Point::new(3.0, 2.0));
    let changed = Style::new()
        .gpui_border(
            Insets {
                left: 6.0,
                top: 4.0,
                right: 1.0,
                bottom: 1.0,
            },
            color,
            false,
        )
        .resolve_paint();
    assert!(tree.set_paint(node, changed));
    tree.compute_layout(viewport, &NaiveMeasurer).unwrap();
    assert!(!tree.last_stats().layout_skipped);
    assert_eq!(tree.rect(child).unwrap().origin(), Point::new(6.0, 4.0));
}
