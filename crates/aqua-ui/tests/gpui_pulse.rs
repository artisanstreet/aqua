use aqua_core::scene::{DisplayItem, GpuiQuadPulse};
use aqua_ui::{
    layout::NaiveMeasurer,
    style::{LayoutStyle, Length, Style},
    tree::{BoxContent, NodeKind, NodeSpec, UiTree},
};

#[test]
fn pulse_scope_surrounds_descendant_quad_paint() {
    let pulse = GpuiQuadPulse {
        start_ms: 17,
        period_ms: 1600,
        min_opacity: 0.4,
    };
    let mut tree = UiTree::new();
    let layout = LayoutStyle {
        width: Length::Px(20.0),
        height: Length::Px(20.0),
        ..Default::default()
    };
    let mut parent = NodeSpec::new(
        NodeKind::Box(BoxContent::new(
            Style::new().gpui_quad_pulse(pulse).resolve_paint(),
        )),
        layout.clone(),
    );
    parent.children.push(NodeSpec::new(
        NodeKind::Box(BoxContent::new(
            Style::new()
                .background(aqua_core::Oklch::WHITE)
                .resolve_paint(),
        )),
        layout,
    ));
    tree.mount_spec(parent);
    tree.compute_layout(aqua_core::Size::new(100.0, 100.0), &NaiveMeasurer)
        .unwrap();
    let items = tree.emit_scene();
    let start = items
        .iter()
        .position(|(_, item)| *item == DisplayItem::PushGpuiQuadPulse(pulse))
        .unwrap();
    let end = items
        .iter()
        .position(|(_, item)| *item == DisplayItem::PopGpuiQuadPulse)
        .unwrap();
    assert!(items[start + 1..end]
        .iter()
        .any(|(_, item)| matches!(item, DisplayItem::Rect { .. } | DisplayItem::Quad { .. })));
}
