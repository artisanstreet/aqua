use aqua_core::DisplayItem;
use aqua_runtime::Runtime;
use aqua_text::{
    FontFamily, FontSlant, FontWeight, RunId, SharedFonts, SharedRuns, StyledSpan, TextLayout,
    TextStyle,
};
use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
use aqua_ui::style::Style;
use aqua_ui::tree::NodeKind;

fn run(runtime: &Runtime) -> (RunId, u64) {
    runtime
        .scene()
        .ordered()
        .find_map(|(_, item)| match item {
            DisplayItem::Text { run, .. } => Some((RunId(run.id), run.revision)),
            _ => None,
        })
        .unwrap()
}

#[test]
fn inherited_font_style_and_child_size_use_identical_measurement_and_shaping() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let inherited = TextStyle {
        families: vec![FontFamily::Monospace],
        size_px: 20.0,
        line_height_px: 32.0,
        weight: FontWeight::BOLD,
        slant: FontSlant::Italic,
        letter_spacing_em: Some(0.1),
        ..Default::default()
    };
    let mut expected_style = inherited.clone();
    expected_style.size_px = 30.0;
    expected_style.line_height_px = 48.0;
    let mut oracle = TextLayout::new();
    oracle.set_text("aWi文", &expected_style);
    let expected = oracle.layout(&mut fonts.lock()).size;
    let mut runtime = Runtime::new(fonts, runs.clone());
    runtime.set_viewport(800.0, 600.0, 1.0);
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().text_style(inherited))
                .child(UiText::builder().arg("aWi文").font_size(30.0).build())
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let (id, _) = run(&runtime);
    assert_eq!(
        runs.lock().resolve(id).unwrap().layout.last_scaled_size(),
        Some(expected)
    );
    let node = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|id| matches!(runtime.tree().get(*id).unwrap().kind, NodeKind::Text(_)))
        .unwrap();
    let measured = runtime.tree().rect(node).unwrap();
    assert!((measured.w - expected.0).abs() < 0.5);
    assert!((measured.h - expected.1).abs() < 0.01);
}

#[test]
fn rich_spans_keep_utf8_offsets_and_same_text_restyles_invalidate_the_run() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let spans = vec![
        StyledSpan::plain("é "),
        StyledSpan::new(
            "代码",
            TextStyle {
                families: vec![FontFamily::Monospace],
                weight: FontWeight::BOLD,
                ..Default::default()
            },
        ),
        StyledSpan::plain(" suffix"),
    ];
    let source: String = spans.iter().map(|span| span.text.as_str()).collect();
    let mut oracle = TextLayout::new();
    oracle.set_spans(spans.clone());
    let expected = oracle.layout(&mut fonts.lock()).size;
    let mut runtime = Runtime::new(fonts, runs.clone());
    runtime.set_viewport(800.0, 600.0, 1.0);
    runtime
        .mount(|_| UiText::builder().spans(spans).selectable(true).build())
        .unwrap();
    runtime.build_frame().unwrap();
    let node = runtime.tree().traverse()[0];
    let (id, revision) = run(&runtime);
    {
        let registry = runs.lock();
        let layout = registry.resolve(id).unwrap().layout;
        assert_eq!(layout.source_len(), source.len());
        assert_eq!(layout.last_scaled_size(), Some(expected));
        let highlights = layout.selection_rects("é ".len().."é 代码".len());
        assert!(!highlights.is_empty());
        assert!(highlights.iter().all(|rect| rect.w > 0.0));
    }
    runtime
        .tree_mut()
        .edit_text(node, |text| text.spans[1].style.size_px = 28.0)
        .unwrap();
    runtime.build_frame().unwrap();
    let (restyled_id, restyled_revision) = run(&runtime);
    assert_eq!(restyled_id, id);
    assert!(restyled_revision > revision);
    assert_eq!(runtime.frame_work().text_runs_shaped, 1);
    assert!(
        runs.lock()
            .resolve(id)
            .unwrap()
            .layout
            .last_scaled_size()
            .unwrap()
            .0
            > expected.0
    );
    runtime.tree_mut().set_text(node, "streamed replacement");
    runtime.build_frame().unwrap();
    assert_eq!(
        runs.lock().resolve(id).unwrap().layout.source_len(),
        "streamed replacement".len()
    );
}
