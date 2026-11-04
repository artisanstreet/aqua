use aqua_core::Size;
use aqua_ui::NaiveMeasurer;
use aqua_ui::style::{LayoutStyle, Length};
use aqua_ui::tree::{BoxContent, NodeKind, NodeSpec, TextContent, UiTree};

#[test]
fn wrapping_text_in_a_row_shrinks_to_the_container_instead_of_overflowing() {
    let mut text = TextContent::label("alpha beta gamma delta epsilon zeta eta theta");
    text.wrap = true;
    let mut tree = UiTree::new();
    let container = NodeSpec::new(
        NodeKind::Box(BoxContent::default()),
        LayoutStyle {
            width: Length::Px(120.0),
            ..Default::default()
        },
    )
    .child(NodeSpec::new(NodeKind::Text(text), LayoutStyle::default()));
    let root = tree.mount_spec(container);
    tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
        .unwrap();
    let child = tree.get(root).unwrap().children[0];
    let rect = tree.rect(child).unwrap();
    assert!(rect.w <= 120.0, "{rect:?}");
    assert!(rect.h > 14.0 * 1.2, "{rect:?}");
}
