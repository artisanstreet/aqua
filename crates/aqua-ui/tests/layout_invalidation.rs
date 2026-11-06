use aqua_core::NodeId;
use aqua_core::Size;
use aqua_style::{LayoutStyle, Length};
use aqua_ui::layout::{NaiveMeasurer, TextMeasurer};
use aqua_ui::tree::{BoxContent, NodeKind, NodeSpec, TextContent, UiTree};
use std::cell::Cell;

const VIEWPORT: Size = Size { w: 200.0, h: 200.0 };

fn sized(width: f32) -> LayoutStyle {
    LayoutStyle {
        width: Length::Px(width),
        height: Length::Px(10.0),
        ..LayoutStyle::default()
    }
}

fn box_spec(style: LayoutStyle) -> NodeSpec {
    NodeSpec::new(NodeKind::Box(BoxContent::default()), style)
}

struct CountingMeasurer {
    calls: Cell<usize>,
    epoch: Cell<Option<u64>>,
}

impl CountingMeasurer {
    fn new(epoch: Option<u64>) -> Self {
        Self {
            calls: Cell::new(0),
            epoch: Cell::new(epoch),
        }
    }
}

impl TextMeasurer for CountingMeasurer {
    fn measure_text(
        &self,
        text: &str,
        font_size: f32,
        max_width: Option<f32>,
        multiline: bool,
    ) -> Size {
        self.calls.set(self.calls.get() + 1);
        NaiveMeasurer.measure_text(text, font_size, max_width, multiline)
    }

    fn memo_epoch(&self) -> Option<u64> {
        self.epoch.get()
    }
}

fn text_tree() -> (UiTree, NodeId) {
    let mut tree = UiTree::new();
    let root = tree.mount_spec(
        box_spec(LayoutStyle {
            width: Length::Px(200.0),
            height: Length::Px(100.0),
            ..LayoutStyle::default()
        })
        .child(NodeSpec::new(
            NodeKind::text(TextContent::label("measured once")),
            LayoutStyle::default(),
        )),
    );
    let text = tree.get(root).unwrap().children[0];
    (tree, text)
}

#[test]
fn setting_an_identical_style_leaves_layout_clean() {
    let mut tree = UiTree::new();
    let node = tree.mount_spec(box_spec(sized(40.0)));
    tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    assert!(tree.last_stats().layout_skipped);
    assert!(tree.set_style(node, sized(40.0)));
    tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    assert!(tree.last_stats().layout_skipped);
    assert!(tree.set_style(node, sized(60.0)));
    tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    assert!(!tree.last_stats().layout_skipped);
    assert_eq!(tree.rect(node).unwrap().w, 60.0);
}

#[test]
fn resyncing_unchanged_text_leaves_layout_clean_until_it_changes() {
    let (mut tree, text) = text_tree();
    tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    assert!(tree.sync_text_leaf_if_changed(text));
    tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    assert!(tree.last_stats().layout_skipped);
    assert!(tree.sync_text_leaf(text));
    tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    assert!(!tree.last_stats().layout_skipped);
}

#[test]
fn a_remeasured_leaf_reuses_its_size_until_the_measurer_epoch_moves() {
    let measurer = CountingMeasurer::new(Some(1));
    let (mut tree, text) = text_tree();
    tree.compute_layout(VIEWPORT, &measurer).unwrap();
    let first = measurer.calls.get();
    assert!(first > 0);
    tree.mark_layout_dirty(text);
    tree.compute_layout(VIEWPORT, &measurer).unwrap();
    assert_eq!(measurer.calls.get(), first);
    measurer.epoch.set(Some(2));
    tree.mark_layout_dirty(text);
    tree.compute_layout(VIEWPORT, &measurer).unwrap();
    assert!(measurer.calls.get() > first);
}

#[test]
fn a_measurer_without_an_epoch_is_asked_every_time() {
    let measurer = CountingMeasurer::new(None);
    let (mut tree, text) = text_tree();
    tree.compute_layout(VIEWPORT, &measurer).unwrap();
    let first = measurer.calls.get();
    tree.mark_layout_dirty(text);
    tree.compute_layout(VIEWPORT, &measurer).unwrap();
    assert!(measurer.calls.get() > first);
}

#[test]
fn restyling_under_the_editor_scale_matches_a_tree_built_with_that_style() {
    let scale = Some(1.5);
    let mut restyled = UiTree::new();
    let node = restyled.mount_spec(box_spec(sized(20.0)));
    restyled.set_gpui_layout_scale(scale);
    restyled.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    restyled.set_style(node, sized(80.0));
    restyled.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    let mut fresh = UiTree::new();
    let expected = fresh.mount_spec(box_spec(sized(80.0)));
    fresh.set_gpui_layout_scale(scale);
    fresh.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    assert_eq!(restyled.rect(node), fresh.rect(expected));
}

fn labelled(text: &str) -> aqua_ui::element::Element {
    use aqua_ui::element::{Box as UiBox, Text};
    UiBox::new()
        .layout("column")
        .child(Text::new("stable heading").build())
        .child(Text::new(text).build())
        .build()
}

#[test]
fn refreshing_identical_elements_keeps_layout_clean_until_a_label_changes() {
    use aqua_reactive::Runtime;
    use aqua_ui::binding::View;
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    view.set_content(&mut tree, vec![labelled("first")])
        .unwrap();
    tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    view.set_content(&mut tree, vec![labelled("first")])
        .unwrap();
    tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    assert!(tree.last_stats().layout_skipped);
    view.set_content(&mut tree, vec![labelled("second")])
        .unwrap();
    tree.compute_layout(VIEWPORT, &NaiveMeasurer).unwrap();
    assert!(!tree.last_stats().layout_skipped);
}

#[test]
fn refreshing_identical_elements_does_not_remeasure_unchanged_labels() {
    use aqua_reactive::Runtime;
    use aqua_ui::binding::View;
    let measurer = CountingMeasurer::new(Some(1));
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    view.set_content(&mut tree, vec![labelled("first")])
        .unwrap();
    tree.compute_layout(VIEWPORT, &measurer).unwrap();
    let initial = measurer.calls.get();
    assert!(initial > 0);
    view.set_content(&mut tree, vec![labelled("first")])
        .unwrap();
    tree.compute_layout(VIEWPORT, &measurer).unwrap();
    assert_eq!(measurer.calls.get(), initial);
    view.set_content(&mut tree, vec![labelled("second")])
        .unwrap();
    tree.compute_layout(VIEWPORT, &measurer).unwrap();
    assert!(measurer.calls.get() > initial);
}
