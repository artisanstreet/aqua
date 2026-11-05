use aqua_core::Size;
use aqua_text::Selection;
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::style::LayoutStyle;
use aqua_ui::tree::{NodeKind, NodeSpec, TextBacking, TextContent, UiTree};

#[test]
fn selecting_static_text_clamps_to_graphemes_without_invalidating_content() {
    let mut tree = UiTree::new();
    let node = tree.mount_spec(NodeSpec::new(
        NodeKind::Text(TextContent::label("a👩‍💻b")),
        LayoutStyle::default(),
    ));
    tree.compute_layout(Size::new(300.0, 100.0), &NaiveMeasurer)
        .unwrap();
    let version = tree.content_version();
    assert!(tree.set_text_selection(node, Some(Selection::new(3, usize::MAX))));
    let NodeKind::Text(text) = &tree.get(node).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.selection, Some(Selection::new(1, "a👩‍💻b".len())));
    assert_eq!(tree.content_version(), version);
    tree.compute_layout(Size::new(300.0, 100.0), &NaiveMeasurer)
        .unwrap();
    assert!(tree.last_stats().layout_skipped);
}

#[test]
fn selecting_during_composition_restores_displaced_text_and_invalidates_layout() {
    let mut tree = UiTree::new();
    let node = tree.mount_spec(NodeSpec::new(
        NodeKind::Text(TextContent::editable("draft", false)),
        LayoutStyle::default(),
    ));
    tree.edit_text(node, |text| {
        let TextBacking::Editable(editor) = &mut text.backing else {
            panic!()
        };
        editor.set_selection(Selection::new(0, 3));
        editor.set_composition("候補", 0..3);
    });
    tree.compute_layout(Size::new(300.0, 100.0), &NaiveMeasurer)
        .unwrap();
    let version = tree.content_version();
    assert!(tree.set_text_selection(node, Some(Selection::caret_at(4))));
    let NodeKind::Text(text) = &tree.get(node).unwrap().kind else {
        panic!()
    };
    let TextBacking::Editable(editor) = &text.backing else {
        panic!()
    };
    assert_eq!(editor.text(), "draft");
    assert!(!editor.is_composing());
    assert_eq!(editor.selection(), Selection::caret_at(4));
    assert!(tree.content_version() > version);
    tree.compute_layout(Size::new(300.0, 100.0), &NaiveMeasurer)
        .unwrap();
    assert!(!tree.last_stats().layout_skipped);
}

#[test]
fn empty_leaf_layout_applies_explicit_dimensions_constraints_and_padding() {
    use aqua_core::Insets;
    use aqua_ui::style::Length;
    use aqua_ui::tree::BoxContent;
    let mut tree = UiTree::new();
    let style = LayoutStyle {
        width: Length::Px(200.0),
        height: Length::Px(100.0),
        max_width: Length::Px(150.0),
        padding: Insets::uniform(10.0),
        ..LayoutStyle::default()
    };
    let node = tree.mount_spec(NodeSpec::new(NodeKind::Box(BoxContent::default()), style));
    tree.compute_layout(Size::new(500.0, 300.0), &NaiveMeasurer)
        .unwrap();
    assert_eq!(tree.rect(node).unwrap().size(), Size::new(150.0, 100.0));
    let style = LayoutStyle {
        padding: Insets::uniform(10.0),
        ..LayoutStyle::default()
    };
    tree.set_style(node, style);
    tree.compute_layout(Size::new(500.0, 300.0), &NaiveMeasurer)
        .unwrap();
    assert_eq!(tree.rect(node).unwrap().size(), Size::new(20.0, 20.0));
}

#[test]
fn transformed_subtree_culling_uses_visual_geometry_and_portals_paint_above_later_siblings() {
    use aqua_core::{DisplayItem, Oklch, Transform2D};
    use aqua_ui::element::{lower_specs, ui};
    use aqua_ui::style::{Axis, Style, px};
    let element = ui::Box::new()
        .style(
            Style::new()
                .axis(Axis::Row)
                .width(px(100.0))
                .height(px(80.0))
                .clip(true),
        )
        .child(
            ui::Box::new()
                .style(
                    Style::new()
                        .width(px(110.0))
                        .height(px(20.0))
                        .flex_shrink(0.0),
                )
                .build(),
        )
        .child(
            ui::Box::new()
                .style(
                    Style::new()
                        .width(px(20.0))
                        .height(px(20.0))
                        .flex_shrink(0.0)
                        .background(Oklch::WHITE)
                        .transform(Transform2D::translate(-80.0, 0.0)),
                )
                .build(),
        )
        .build();
    let mut tree = UiTree::new();
    let root = tree.mount_spec(lower_specs(&element).unwrap().remove(0));
    let target = tree.get(root).unwrap().children[1];
    tree.compute_layout(Size::new(400.0, 300.0), &NaiveMeasurer)
        .unwrap();
    assert_eq!(tree.visual_rect(target).unwrap().x, 30.0);
    assert!(
        tree.emit_scene()
            .iter()
            .any(|(node, item)| *node == target && matches!(item, DisplayItem::Rect { .. }))
    );
    let mut paint = tree.get(target).unwrap().paint().clone();
    paint.transform = Some(Transform2D::translate(150.0, 0.0));
    tree.set_paint(target, paint);
    assert!(!tree.emit_scene().iter().any(|(node, _)| *node == target));
    let portal = tree
        .insert_spec(
            tree.get(root).unwrap().children[0],
            0,
            lower_specs(
                &ui::Box::new()
                    .style(
                        Style::new()
                            .width(px(30.0))
                            .height(px(30.0))
                            .background(Oklch::WHITE),
                    )
                    .build(),
            )
            .unwrap()
            .remove(0),
        )
        .unwrap();
    tree.mark_portal(portal);
    tree.compute_layout(Size::new(400.0, 300.0), &NaiveMeasurer)
        .unwrap();
    let scene = tree.emit_scene();
    let end_clip = scene
        .iter()
        .position(|(node, item)| *node == root && matches!(item, DisplayItem::PopClip))
        .unwrap();
    let portal_paint = scene
        .iter()
        .position(|(node, item)| *node == portal && matches!(item, DisplayItem::Rect { .. }))
        .unwrap();
    assert!(portal_paint > end_clip);
}

#[test]
fn media_preserves_asset_lease_and_fit_source_then_releases_on_unmount() {
    use aqua_core::image::ImageFit;
    use aqua_core::{DisplayItem, ImageId, ImageRef, Point, RawId, Rect};
    use aqua_ui::element::{lower_specs, ui};
    use aqua_ui::style::{Style, px};
    use std::sync::Arc;
    let lease = Arc::new(());
    let id = ImageId::from_raw(RawId {
        index: 7,
        generation: 2,
    });
    let image = ImageRef::with_lease(id, lease.clone());
    let element = ui::Media::new(image)
        .fit(ImageFit::Cover)
        .position(Point::new(1.0, 0.0))
        .source_rect(Rect::from_edges(0.25, 0.0, 0.75, 1.0))
        .style(Style::new().width(px(100.0)).height(px(50.0)))
        .build();
    let mut tree = UiTree::new();
    let root = tree.mount_spec(lower_specs(&element).unwrap().remove(0));
    drop(element);
    assert_eq!(Arc::strong_count(&lease), 2);
    tree.compute_layout(Size::new(300.0, 200.0), &NaiveMeasurer)
        .unwrap();
    assert!(tree.emit_scene().iter().any(|(node, item)| *node == root && matches!(item, DisplayItem::Image { fit: ImageFit::Cover, position, source, .. } if *position == Point::new(1.0, 0.0) && *source == Rect::from_edges(0.25, 0.0, 0.75, 1.0))));
    tree.remove(root);
    assert_eq!(Arc::strong_count(&lease), 1);
}

#[test]
fn shadow_stack_emits_outer_then_background_then_inset_and_retains_visible_transformed_shadow() {
    use aqua_core::{DisplayItem, Oklch, Transform2D};
    use aqua_ui::element::{lower_specs, ui};
    use aqua_ui::style::{Axis, ShadowLayer, Style, px};
    let outer = ShadowLayer {
        dx: -30.0,
        dy: 0.0,
        blur: 0.0,
        spread: 8.0,
        inset: false,
        color: Oklch::BLACK,
    };
    let inset = ShadowLayer {
        dx: 0.0,
        dy: 1.0,
        blur: 0.0,
        spread: 0.0,
        inset: true,
        color: Oklch::WHITE,
    };
    let element = ui::Box::new()
        .style(
            Style::new()
                .axis(Axis::Row)
                .width(px(100.0))
                .height(px(60.0))
                .clip(true),
        )
        .child(
            ui::Box::new()
                .style(
                    Style::new()
                        .width(px(110.0))
                        .height(px(20.0))
                        .flex_shrink(0.0),
                )
                .build(),
        )
        .child(
            ui::Box::new()
                .style(
                    Style::new()
                        .width(px(20.0))
                        .height(px(20.0))
                        .flex_shrink(0.0)
                        .background(Oklch::WHITE)
                        .transform(Transform2D::translate(5.0, 0.0))
                        .shadows(vec![inset, outer]),
                )
                .build(),
        )
        .build();
    let mut tree = UiTree::new();
    let root = tree.mount_spec(lower_specs(&element).unwrap().remove(0));
    let target = tree.get(root).unwrap().children[1];
    tree.compute_layout(Size::new(400.0, 300.0), &NaiveMeasurer)
        .unwrap();
    let operations: Vec<_> = tree
        .emit_scene()
        .into_iter()
        .filter(|(node, _)| *node == target)
        .map(|(_, item)| item)
        .collect();
    let outer_index = operations
        .iter()
        .position(|item| matches!(item, DisplayItem::Shadow { layer, .. } if *layer == outer))
        .unwrap();
    let fill_index = operations
        .iter()
        .position(|item| matches!(item, DisplayItem::Rect { .. }))
        .unwrap();
    let inset_index = operations
        .iter()
        .position(|item| matches!(item, DisplayItem::Shadow { layer, .. } if *layer == inset))
        .unwrap();
    assert!(outer_index < fill_index && fill_index < inset_index);
}

#[test]
fn mixed_corners_and_edge_border_reach_scene_and_clip() {
    use aqua_core::{CornerRadii, DisplayItem, Insets, Oklch};
    use aqua_ui::element::{lower_specs, ui};
    use aqua_ui::style::{Style, px};
    let corners = CornerRadii {
        top_left: 0.0,
        top_right: 8.0,
        bottom_right: 3.0,
        bottom_left: 1.0,
    };
    let edges = Insets {
        top: 2.0,
        right: 3.0,
        bottom: 0.0,
        left: 0.0,
    };
    let element = ui::Box::new()
        .style(
            Style::new()
                .width(px(40.0))
                .height(px(40.0))
                .background(Oklch::WHITE)
                .corners(corners)
                .border_edges(edges, Oklch::BLACK)
                .border_dashed(true)
                .clip(true),
        )
        .build();
    let mut tree = UiTree::new();
    tree.mount_spec(lower_specs(&element).unwrap().remove(0));
    tree.compute_layout(Size::new(100.0, 100.0), &NaiveMeasurer)
        .unwrap();
    let items = tree.emit_scene();
    assert!(items.iter().any(|(_, item)| matches!(item, DisplayItem::PushRoundedClip { corners: value, .. } if *value == corners)));
    assert!(items.iter().any(|(_, item)| matches!(item, DisplayItem::Quad { corners: value, border_widths, dashed: true, .. } if *value == corners && *border_widths == edges)));
}

#[test]
fn gpui_paint_profile_emits_uniform_quads_and_invalidates_cached_content() {
    use aqua_core::{DisplayItem, Oklch};
    use aqua_ui::element::{lower_specs, ui};
    use aqua_ui::style::{Style, px};
    let element = ui::Box::new()
        .style(
            Style::new()
                .width(px(40.0))
                .height(px(40.0))
                .radius(px(4.0))
                .background(Oklch::WHITE)
                .border_of(0.4, Oklch::BLACK)
                .clip(true),
        )
        .build();
    let mut tree = UiTree::new();
    tree.mount_spec(lower_specs(&element).unwrap().remove(0));
    tree.compute_layout(Size::new(100.0, 100.0), &NaiveMeasurer)
        .unwrap();
    assert!(
        tree.emit_scene()
            .iter()
            .any(|(_, item)| matches!(item, DisplayItem::Rect { .. }))
    );
    let version = tree.content_version();
    tree.set_gpui_paint(true);
    assert!(tree.content_version() > version);
    let items = tree.emit_scene();
    assert!(
        items.iter().any(
            |(_, item)| matches!(item, DisplayItem::Quad { fill, .. } if *fill == Oklch::WHITE)
        )
    );
    assert!(items.iter().any(|(_, item)| matches!(item, DisplayItem::Quad { border_widths, .. } if border_widths.top == 0.4)));
    assert!(
        items
            .iter()
            .any(|(_, item)| matches!(item, DisplayItem::PushGpuiContentMask { .. }))
    );
    tree.set_gpui_paint(false);
    assert!(
        tree.emit_scene()
            .iter()
            .any(|(_, item)| matches!(item, DisplayItem::Rect { .. }))
    );
}

#[test]
fn source_shadow_and_border_colors_preserve_layer_order_in_scene() {
    use aqua_core::{DisplayItem, GpuiBackground, GpuiHsla, Insets};
    use aqua_ui::element::{lower_specs, ui};
    use aqua_ui::style::{GpuiShadowLayer, Style, px};
    let color = GpuiHsla {
        h: 0.19,
        s: 0.43,
        l: 0.27,
        a: 0.31,
    };
    let outer = GpuiShadowLayer {
        dx: 1.0,
        dy: 2.0,
        blur: 4.0,
        spread: -1.0,
        inset: false,
        color,
    };
    let inset = GpuiShadowLayer {
        inset: true,
        ..outer
    };
    let node = ui::Box::new()
        .style(
            Style::new()
                .width(px(20.0))
                .height(px(20.0))
                .gpui_shadows(vec![inset, outer])
                .gpui_background(GpuiBackground::Solid(color))
                .gpui_border(Insets::uniform(1.0), color, false),
        )
        .build();
    let mut tree = UiTree::new();
    tree.mount_spec(lower_specs(&node).unwrap().remove(0));
    tree.compute_layout(Size::new(80.0, 60.0), &NaiveMeasurer)
        .unwrap();
    let items: Vec<_> = tree
        .emit_scene()
        .into_iter()
        .map(|(_, item)| item)
        .collect();
    let position = |predicate: fn(&DisplayItem) -> bool| items.iter().position(predicate).unwrap();
    let outer_at =
        position(|item| matches!(item, DisplayItem::GpuiShadow { layer, .. } if !layer.inset));
    let fill_at = position(|item| matches!(item, DisplayItem::GpuiBackground { .. }));
    let inset_at =
        position(|item| matches!(item, DisplayItem::GpuiShadow { layer, .. } if layer.inset));
    let border_at = position(|item| matches!(item, DisplayItem::GpuiBorder { .. }));
    assert!(outer_at < fill_at && fill_at < inset_at && inset_at < border_at);
    assert!(
        matches!(&items[border_at], DisplayItem::GpuiBorder { color: actual, .. } if *actual == color)
    );
}

#[test]
fn gpui_overflow_mask_insets_only_visible_source_borders() {
    use aqua_core::{DisplayItem, GpuiHsla, Insets, Rect};
    use aqua_ui::element::{lower_specs, ui};
    use aqua_ui::style::{Style, px};
    let widths = Insets {
        top: 1.0,
        right: 2.0,
        bottom: 3.0,
        left: 4.0,
    };
    let color = GpuiHsla {
        h: 0.0,
        s: 0.0,
        l: 0.0,
        a: 1.0,
    };
    let element = ui::Box::new()
        .style(
            Style::new()
                .width(px(40.0))
                .height(px(40.0))
                .radius(px(9.0))
                .gpui_border(widths, color, false)
                .clip(true),
        )
        .build();
    let mut tree = UiTree::new();
    let root = tree.mount_spec(lower_specs(&element).unwrap().remove(0));
    tree.set_gpui_paint(true);
    tree.compute_layout(Size::new(100.0, 100.0), &NaiveMeasurer)
        .unwrap();
    let items = tree.emit_scene();
    assert!(items.iter().any(|(_, item)| matches!(item, DisplayItem::PushGpuiContentMask { bounds } if *bounds == Rect::from_edges(4.0, 1.0, 38.0, 37.0))));
    let mut paint = tree.paint(root).unwrap();
    paint.gpui_border.as_mut().unwrap().color.a = 0.0;
    assert!(tree.set_paint(root, paint));
    let items = tree.emit_scene();
    assert!(items.iter().any(|(_, item)| matches!(item, DisplayItem::PushGpuiContentMask { bounds } if *bounds == Rect::from_edges(0.0, 0.0, 40.0, 40.0))));
}

#[test]
fn non_box_paint_round_trips_and_returns_to_default() {
    use aqua_core::Oklch;
    use aqua_ui::style::Paint;
    let mut tree = UiTree::new();
    let node = tree.mount_spec(NodeSpec::new(
        NodeKind::Text(TextContent::label("hello")),
        LayoutStyle::default(),
    ));
    assert_eq!(tree.paint(node), Some(Paint::default()));
    assert!(!tree.set_paint(node, Paint::default()));
    let painted = Paint {
        fill: Some(Oklch::BLACK),
        opacity: Some(0.5),
        ..Paint::default()
    };
    assert!(tree.set_paint(node, painted.clone()));
    assert_eq!(tree.paint(node), Some(painted.clone()));
    assert!(!tree.set_paint(node, painted));
    assert!(tree.set_paint(node, Paint::default()));
    assert_eq!(tree.paint(node), Some(Paint::default()));
}
