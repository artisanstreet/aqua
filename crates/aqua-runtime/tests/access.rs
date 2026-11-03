use aqua_core::NodeId;
use aqua_protocol::{A11yAction, A11yActionData, A11yActionRequest, Input};
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::Text;
use aqua_ui::tree::{NodeKind, TextBacking};

fn field(runtime: &Runtime) -> NodeId {
    runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|id| matches!(runtime.tree().get(*id).unwrap().kind, NodeKind::Text(_)))
        .unwrap()
}

#[test]
fn accessible_multiline_text_exposes_shaped_graphemes_and_byte_selections() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(400.0, 200.0, 2.0);
    runtime
        .mount(|_| {
            Text::builder()
                .arg("é👩‍💻\nאב suffix")
                .editable(true)
                .multiline(true)
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let node = field(&runtime);
    let (_, _, items) = runtime.a11y_nodes().unwrap();
    let text = items
        .iter()
        .find(|item| item.semantic.node == node)
        .unwrap()
        .text
        .as_ref()
        .unwrap();
    assert_eq!(
        text.runs
            .iter()
            .map(|run| run.text.as_str())
            .collect::<String>(),
        "é👩‍💻\nאב suffix"
    );
    assert!(text.runs.len() >= 2);
    assert_eq!(text.runs[0].character_lengths, vec![2, 11, 1]);
    assert!(text.runs.iter().any(|run| run.rtl));
    for run in &text.runs {
        assert_eq!(
            run.character_lengths
                .iter()
                .map(|length| usize::from(*length))
                .sum::<usize>(),
            run.text.len()
        );
        assert_eq!(run.character_positions.len(), run.character_lengths.len());
        assert!(run.bounds.h > 20.0);
    }
    runtime
        .step(Input::Access(A11yActionRequest {
            node,
            action: A11yAction::Other("set-text-selection"),
            data: A11yActionData::TextByteSelection {
                anchor_node: node,
                anchor_byte: 2,
                focus_node: node,
                focus_byte: 13,
            },
        }))
        .unwrap();
    let NodeKind::Text(content) = &runtime.tree().get(node).unwrap().kind else {
        panic!()
    };
    let TextBacking::Editable(editor) = &content.backing else {
        panic!()
    };
    assert_eq!(editor.selected_text(), Some("👩‍💻"));
    let (_, _, items) = runtime.a11y_nodes().unwrap();
    assert_eq!(
        items
            .iter()
            .find(|item| item.semantic.node == node)
            .unwrap()
            .text
            .as_ref()
            .unwrap()
            .selection,
        Some((2, 13))
    );
}

#[test]
fn password_fields_do_not_expose_text_runs_or_accept_byte_selection() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime
        .mount(|_| {
            Text::builder()
                .arg("secret")
                .editable(true)
                .password(true)
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let node = field(&runtime);
    let (_, _, items) = runtime.a11y_nodes().unwrap();
    assert!(items
        .iter()
        .find(|item| item.semantic.node == node)
        .unwrap()
        .text
        .is_none());
    runtime
        .step(Input::Access(A11yActionRequest {
            node,
            action: A11yAction::Other("set-text-selection"),
            data: A11yActionData::TextByteSelection {
                anchor_node: node,
                anchor_byte: 0,
                focus_node: node,
                focus_byte: 6,
            },
        }))
        .unwrap();
    let NodeKind::Text(content) = &runtime.tree().get(node).unwrap().kind else {
        panic!()
    };
    let TextBacking::Editable(editor) = &content.backing else {
        panic!()
    };
    assert!(editor.selected_text().is_none());
}

#[test]
fn accessibility_does_not_truncate_oversized_graphemes() {
    let source = format!("a{}", "\u{301}".repeat(200));
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime
        .mount(|_| Text::builder().arg(source.clone()).editable(true).build())
        .unwrap();
    runtime.build_frame().unwrap();
    let node = field(&runtime);
    let (_, _, items) = runtime.a11y_nodes().unwrap();
    let text = items
        .iter()
        .find(|item| item.semantic.node == node)
        .unwrap()
        .text
        .as_ref()
        .unwrap();
    assert_eq!(
        text.runs
            .iter()
            .map(|run| run.text.as_str())
            .collect::<String>(),
        source
    );
    for run in &text.runs {
        assert!(run.character_lengths.iter().all(|length| *length > 0));
        assert_eq!(
            run.character_lengths
                .iter()
                .map(|length| usize::from(*length))
                .sum::<usize>(),
            run.text.len()
        );
        let mut offset = run.byte_start;
        for length in &run.character_lengths {
            offset += usize::from(*length);
            assert!(source.is_char_boundary(offset));
        }
    }
    runtime
        .step(Input::Access(A11yActionRequest {
            node,
            action: A11yAction::Other("set-text-selection"),
            data: A11yActionData::TextByteSelection {
                anchor_node: node,
                anchor_byte: 0,
                focus_node: node,
                focus_byte: 3,
            },
        }))
        .unwrap();
    let NodeKind::Text(content) = &runtime.tree().get(node).unwrap().kind else {
        panic!()
    };
    let TextBacking::Editable(editor) = &content.backing else {
        panic!()
    };
    assert!(editor.selection().caret == 0 || editor.selection().caret == source.len());
}

#[test]
fn accessible_readonly_selection_updates_the_logical_copy_source() {
    use aqua_text::document::{SelectionSource, SelectionSourceId, SourceSlice};
    use std::sync::Arc;
    let source = "prefix 👩‍💻 suffix";
    let start = "prefix ".len();
    let end = start + "👩‍💻".len();
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .register_selection_sources([SelectionSource {
            id: SelectionSourceId(10),
            order: vec![0],
            text: Arc::from(source),
            separator: Arc::from("\n"),
            copies: Vec::new(),
        }])
        .unwrap();
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime
        .mount(|_| {
            Text::builder()
                .arg("👩‍💻")
                .selection_source(SourceSlice {
                    source: SelectionSourceId(10),
                    range: start..end,
                })
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let node = field(&runtime);
    runtime
        .step(Input::Access(A11yActionRequest {
            node,
            action: A11yAction::Other("set-text-selection"),
            data: A11yActionData::TextByteSelection {
                anchor_node: node,
                anchor_byte: 0,
                focus_node: node,
                focus_byte: "👩‍💻".len(),
            },
        }))
        .unwrap();
    assert_eq!(runtime.selected_text().as_deref(), Some("👩‍💻"));
}

#[test]
fn accessible_empty_editor_keeps_placeholder_out_of_text_ranges() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(400.0, 200.0, 1.0);
    runtime
        .mount(|_| {
            Text::builder()
                .editable(true)
                .placeholder("Search...")
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let node = field(&runtime);
    let value = |runtime: &Runtime| {
        let (_, _, items) = runtime.a11y_nodes().unwrap();
        items
            .iter()
            .find(|item| item.semantic.node == node)
            .unwrap()
            .text
            .as_ref()
            .unwrap()
            .runs
            .iter()
            .map(|run| run.text.as_str())
            .collect::<String>()
    };
    assert_eq!(value(&runtime), "");
    runtime.tree_mut().set_text(node, "typed");
    runtime.build_frame().unwrap();
    assert_eq!(value(&runtime), "typed");
    runtime.tree_mut().set_text(node, "");
    runtime.build_frame().unwrap();
    assert_eq!(value(&runtime), "");
}
