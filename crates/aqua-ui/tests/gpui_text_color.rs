use aqua_core::{DisplayItem, GpuiHsla, Oklch, Size};
use aqua_reactive::Runtime;
use aqua_ui::{
    binding::View,
    element::{Box as UiBox, Text},
    layout::NaiveMeasurer,
    style::Style,
    tree::UiTree,
};

#[test]
fn raw_colors_inherit_override_and_cover_placeholder_text() {
    let base = GpuiHsla {
        h: 0.1,
        s: 0.2,
        l: 0.3,
        a: 0.4,
    };
    let own = GpuiHsla {
        h: 0.5,
        s: 0.6,
        l: 0.7,
        a: 0.8,
    };
    let placeholder = GpuiHsla {
        h: 0.9,
        s: 0.4,
        l: 0.2,
        a: 1.0,
    };
    let content = UiBox::new()
        .layout("column")
        .style(Style::new().gpui_text_color(base))
        .child(Text::new("inherited").build())
        .child(Text::new("own").gpui_color(own).build())
        .child(Text::new("native").color(Oklch::WHITE).build())
        .child(
            Text::new("")
                .editable(true)
                .placeholder("hint")
                .gpui_placeholder_color(placeholder)
                .build(),
        )
        .build();
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    view.set_content(&mut tree, vec![content]).unwrap();
    tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
        .unwrap();
    let mut active = None;
    let mut colors = Vec::new();
    for (_, item) in tree.emit_scene() {
        match item {
            DisplayItem::PushGpuiTextColor(color) => active = Some(color),
            DisplayItem::PopGpuiTextColor => active = None,
            DisplayItem::Text { .. } => colors.push(active),
            _ => {}
        }
    }
    assert_eq!(colors, vec![Some(base), Some(own), None, Some(placeholder)]);
}
