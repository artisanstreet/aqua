use aqua_core::{
    scene::{GpuiTextShimmer, TextShimmer},
    DisplayItem, GpuiTextColor, Size,
};
use aqua_reactive::Runtime;
use aqua_ui::{
    binding::View,
    element::{Box as UiBox, Element, Text},
    layout::NaiveMeasurer,
    style::{Length, Style},
    tree::UiTree,
};
use std::time::Duration;

const SHIMMER: TextShimmer = TextShimmer {
    key: 7,
    spread: 50.0,
    target: GpuiTextColor {
        hue_degrees: 0.0,
        saturation: 0.0,
        lightness: 1.0,
        alpha: 1.0,
    },
    duration: Duration::from_secs(2),
    delay: Duration::ZERO,
};

fn label(text: &str, above: f32) -> Element {
    UiBox::new()
        .layout("column")
        .child(
            UiBox::new()
                .width(Length::Px(120.0))
                .height(Length::Px(above))
                .build(),
        )
        .child(
            UiBox::new()
                .width(Length::Px(120.0))
                .height(Length::Px(24.0))
                .style(Style::new().text_shimmer(SHIMMER))
                .child(Text::new(text).build())
                .build()
                .with_key("label"),
        )
        .build()
}

fn scopes(tree: &mut UiTree) -> Vec<GpuiTextShimmer> {
    tree.emit_scene()
        .into_iter()
        .filter_map(|(_, item)| match item {
            DisplayItem::PushGpuiTextShimmer(shimmer) => Some(shimmer),
            _ => None,
        })
        .collect()
}

#[test]
fn a_rebuilt_element_keeps_its_shimmer_in_the_very_next_scene() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    view.set_content(&mut tree, vec![label("Compacting", 0.0)])
        .unwrap();
    tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
        .unwrap();
    let first = scopes(&mut tree);
    assert_eq!(first.len(), 1, "the styled box opens one shimmer scope");
    assert_eq!(first[0].key, SHIMMER.key);
    assert_eq!(
        (
            first[0].map.bounds.y,
            first[0].map.bounds.w,
            first[0].map.bounds.h
        ),
        (0.0, 120.0, 24.0),
        "the sweep spans the element's own box"
    );

    view.set_content(&mut tree, vec![label("Compacted", 40.0)])
        .unwrap();
    tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
        .unwrap();
    let rebuilt = scopes(&mut tree);
    assert_eq!(
        rebuilt.len(),
        1,
        "a rebuild must not produce a scene without the shimmer"
    );
    assert_eq!(rebuilt[0].key, SHIMMER.key, "the clock identity survives");
    assert_eq!(
        rebuilt[0].map.bounds.y, 40.0,
        "the sweep follows the element when layout moves it"
    );
}
