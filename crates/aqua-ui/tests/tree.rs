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
    use aqua_ui::style::{px, Axis, Style};
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
    assert!(tree
        .emit_scene()
        .iter()
        .any(|(node, item)| *node == target && matches!(item, DisplayItem::Rect { .. })));
    let mut paint = tree.get(target).unwrap().paint.clone();
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
    use aqua_ui::style::{px, Style};
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
    use aqua_ui::style::{px, Axis, ShadowLayer, Style};
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
