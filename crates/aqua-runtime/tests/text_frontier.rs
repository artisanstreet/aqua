use aqua_runtime::{PopupOptions, Runtime};
use aqua_text::{CaretAffinity, SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box, Text};
use aqua_ui::style::{px, Style};
use aqua_ui::tree::NodeKind;

fn text_node(app: &Runtime, value: &str) -> aqua_core::NodeId {
    app.tree().traverse().into_iter().find(|id| matches!(&app.tree().get(*id).unwrap().kind, NodeKind::Text(text) if text.text() == value)).unwrap()
}

#[test]
fn retained_frontier_checks_boundaries_refresh_and_removed_nodes() {
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(300.0, 200.0, 1.25);
    app.mount(|_| {
        Text::new("é\nsecond")
            .multiline(true)
            .build()
            .with_key("first")
    })
    .unwrap();
    let node = text_node(&app, "é\nsecond");
    assert!(app
        .text_frontier(node, 0, CaretAffinity::Downstream)
        .is_none());
    app.build_frame().unwrap();
    let first = app
        .text_frontier(node, 0, CaretAffinity::Downstream)
        .unwrap();
    let second = app
        .text_frontier(node, 3, CaretAffinity::Downstream)
        .unwrap();
    assert!(first.line_bottom > 0.0);
    assert!(second.line_bottom > first.line_bottom);
    assert!(second.block_height >= second.line_bottom);
    assert_eq!(first.content_origin, second.content_origin);
    assert!(app
        .text_frontier(node, 1, CaretAffinity::Downstream)
        .is_none());
    assert!(app
        .text_frontier(node, 100, CaretAffinity::Downstream)
        .is_none());
    app.refresh(vec![Text::new("changed").build().with_key("first")])
        .unwrap();
    assert!(app
        .text_frontier(node, 0, CaretAffinity::Downstream)
        .is_none());
    app.build_frame().unwrap();
    assert!(app
        .text_frontier(node, 0, CaretAffinity::Downstream)
        .is_some());
    app.refresh(vec![Text::new("other").build().with_key("other")])
        .unwrap();
    app.build_frame().unwrap();
    assert!(app
        .text_frontier(node, 0, CaretAffinity::Downstream)
        .is_none());
}

#[test]
fn retained_frontier_supports_popup_text_after_frame_and_fails_after_close() {
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(300.0, 200.0, 1.0);
    app.mount(|_| Box::new().build()).unwrap();
    app.build_frame().unwrap();
    let owner = app.tree().root().unwrap();
    let popup = app
        .open_popup(
            owner,
            Box::new()
                .style(Style::new().width(px(200.0)).height(px(100.0)))
                .child(Text::new("popup\nline").multiline(true).build())
                .build(),
            PopupOptions::dialog(),
        )
        .unwrap();
    app.build_frame().unwrap();
    let node = text_node(&app, "popup\nline");
    let frontier = app
        .text_frontier(node, 6, CaretAffinity::Downstream)
        .unwrap();
    assert!(frontier.line_bottom > 0.0);
    assert!(frontier.block_height >= frontier.line_bottom);
    assert!(app.close_popup(popup));
    assert!(app
        .text_frontier(node, 0, CaretAffinity::Downstream)
        .is_none());
}

#[test]
fn retained_frontier_survives_unrelated_tree_changes_after_the_frame() {
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(300.0, 200.0, 1.0);
    app.mount(|_| Text::new("stable").build().with_key("stable"))
        .unwrap();
    let node = text_node(&app, "stable");
    app.build_frame().unwrap();
    assert!(app
        .text_frontier(node, 0, CaretAffinity::Downstream)
        .is_some());
    app.refresh(vec![
        Text::new("stable").build().with_key("stable"),
        Text::new("sibling").build().with_key("sibling"),
    ])
    .unwrap();
    assert!(app
        .text_frontier(node, 0, CaretAffinity::Downstream)
        .is_some());
    let sibling = text_node(&app, "sibling");
    assert!(app
        .text_frontier(sibling, 0, CaretAffinity::Downstream)
        .is_none());
}
