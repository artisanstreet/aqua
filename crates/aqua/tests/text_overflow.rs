use aqua::{
    ui::{Box as UiBox, Text},
    Style, UiApp,
};
use aqua_core::DisplayItem;
use aqua_text::{overflow::OverflowMode, RunId, SharedFonts, SharedRuns};
use aqua_ui::style::Length;

#[test]
fn constrained_text_keeps_full_accessibility_and_source_geometry() {
    let source = "A long document name ending with notes.txt";
    let runs = SharedRuns::new();
    let mut app = UiApp::new(SharedFonts::new(), runs.clone());
    app.mount(move |_| {
        UiBox::new()
            .width(90.0)
            .child(
                Text::new(source)
                    .selectable(true)
                    .overflow(OverflowMode::StartEllipsis)
                    .style(
                        Style::new()
                            .width(Length::Pct(100.0))
                            .min_width(Length::Px(0.0)),
                    )
                    .build(),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(300.0, 100.0, 1.0);
    app.build_frame().unwrap();
    let run = app
        .scene()
        .ordered()
        .find_map(|(_, item)| match item {
            DisplayItem::Text { run, .. } => Some(run.clone()),
            _ => None,
        })
        .unwrap();
    let registry = runs.lock();
    let layout = registry
        .resolve_expected(RunId(run.id), run.revision)
        .unwrap()
        .layout;
    assert!(layout.display_text().starts_with('…'));
    assert!(layout.display_text().len() < source.len());
    assert_eq!(layout.source_len(), source.len());
    let start = source.len() - "notes.txt".len();
    assert!(!layout.selection_rects(start..source.len()).is_empty());
    drop(registry);
    let (_, _, nodes) = app.a11y_nodes().unwrap();
    let accessible = nodes
        .iter()
        .filter_map(|node| node.text.as_ref())
        .flat_map(|text| text.runs.iter())
        .map(|run| run.text.as_str())
        .collect::<String>();
    assert_eq!(accessible, source);
}
