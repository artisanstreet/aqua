use aqua_core::{
    InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId, Point, PointerButton, PointerEvent,
    PointerKind,
};
use aqua_protocol::Instant;
use aqua_runtime::Runtime;
use aqua_text::{LineMode, Selection, SharedFonts, SharedRuns, TextLayout, TextStyle, WrapMode};
use aqua_ui::{
    element::ui::{Box as UiBox, Text as UiText},
    style::{px, Axis, Style},
    tree::{NodeKind, TextBacking},
};

fn fixture(editable: bool, texts: &[&str]) -> (Runtime, Vec<NodeId>, SharedFonts) {
    let fonts = SharedFonts::new();
    let mut runtime = Runtime::new(fonts.clone(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().axis(Axis::Column))
                .children(texts.iter().map(|text| {
                    UiText::builder()
                        .arg(*text)
                        .editable(editable)
                        .multiline(true)
                        .selectable(true)
                        .style(Style::new().width(px(220.0)).height(px(60.0)))
                        .build()
                }))
                .build()
        })
        .unwrap();
    runtime.set_viewport(400.0, 400.0, 1.0);
    runtime.build_frame().unwrap();
    let nodes = runtime
        .tree()
        .traverse()
        .into_iter()
        .filter(|node| {
            runtime
                .tree()
                .get(*node)
                .is_some_and(|record| matches!(record.kind, NodeKind::Text(_)))
        })
        .collect();
    (runtime, nodes, fonts)
}

fn position(runtime: &Runtime, node: NodeId, fonts: &SharedFonts, byte: usize) -> Point {
    let record = runtime.tree().get(node).unwrap();
    let NodeKind::Text(text) = &record.kind else {
        panic!()
    };
    let rect = runtime.tree().rect(node).unwrap();
    let mut layout = TextLayout::new();
    layout.set_text(
        text.text(),
        &TextStyle {
            size_px: 14.0,
            line_height_px: 16.8,
            ..TextStyle::default()
        },
    );
    layout.set_line_mode(LineMode::Multi);
    layout.set_wrap(WrapMode::Word);
    layout.set_max_width(Some(rect.w));
    layout.layout(&mut fonts.lock());
    let caret = layout.caret(byte).unwrap();
    Point::new(rect.x + caret.x, rect.y + caret.y_top + caret.height * 0.5)
}

fn pointer(at: Point, buttons: u8) -> PointerEvent {
    PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: at,
        buttons,
    }
}

fn down(runtime: &mut Runtime, at: Point, time: f64) {
    runtime.advance_clock(time);
    runtime.handle_input(&InputEvent::PointerDown(
        pointer(at, 1),
        PointerButton::Primary,
    ));
}

fn up(runtime: &mut Runtime, at: Point) {
    runtime.handle_input(&InputEvent::PointerUp(
        pointer(at, 0),
        PointerButton::Primary,
    ));
}

fn selection(runtime: &Runtime, node: NodeId) -> Selection {
    let NodeKind::Text(text) = &runtime.tree().get(node).unwrap().kind else {
        panic!()
    };
    match &text.backing {
        TextBacking::Editable(editor) => editor.selection(),
        TextBacking::Static(_) => text.selection.unwrap_or_default(),
    }
}

fn key(runtime: &mut Runtime, named: NamedKey, modifiers: Modifiers) {
    assert!(runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(named),
        modifiers,
        repeat: false
    })));
}

#[test]
fn drag_extends_and_reverses_across_read_only_leaves_in_source_order() {
    let (mut runtime, nodes, fonts) = fixture(false, &["hello world", "second paragraph"]);
    let start = position(&runtime, nodes[0], &fonts, 6);
    let end = position(&runtime, nodes[1], &fonts, 6);
    down(&mut runtime, start, 1.0);
    assert!(runtime.handle_input(&InputEvent::PointerMove(pointer(end, 1))));
    up(&mut runtime, end);
    assert_eq!(runtime.selected_text().as_deref(), Some("world\nsecond"));
    down(&mut runtime, end, 2.0);
    runtime.handle_input(&InputEvent::PointerMove(pointer(start, 1)));
    up(&mut runtime, start);
    assert_eq!(runtime.selected_text().as_deref(), Some("world\nsecond"));
    runtime.build_frame().unwrap();
    assert_eq!(runtime.frame_work().text_source_bytes, 0);
}

#[test]
fn double_click_selects_word_with_spaces_and_triple_click_selects_paragraph() {
    let (mut runtime, nodes, fonts) = fixture(true, &["hello   world\nnext line"]);
    let at = position(&runtime, nodes[0], &fonts, 2);
    down(&mut runtime, at, 1.0);
    up(&mut runtime, at);
    down(&mut runtime, at, 1.1);
    up(&mut runtime, at);
    assert_eq!(runtime.selected_text().as_deref(), Some("hello   "));
    down(&mut runtime, at, 1.2);
    up(&mut runtime, at);
    assert_eq!(runtime.selected_text().as_deref(), Some("hello   world\n"));
    down(&mut runtime, at, 1.3);
    up(&mut runtime, at);
    assert_eq!(runtime.selected_text().as_deref(), Some("hello   world\n"));
}

#[test]
fn shift_click_extends_existing_caret_and_cut_paste_are_undoable() {
    let (mut runtime, nodes, fonts) = fixture(true, &["hello world"]);
    let first = position(&runtime, nodes[0], &fonts, 0);
    let last = position(&runtime, nodes[0], &fonts, 5);
    down(&mut runtime, first, 1.0);
    up(&mut runtime, first);
    runtime.handle_input(&InputEvent::ModifiersChanged(Modifiers::SHIFT));
    down(&mut runtime, last, 2.0);
    up(&mut runtime, last);
    assert_eq!(runtime.selected_text().as_deref(), Some("hello"));
    assert!(runtime.delete_selected_text());
    assert!(runtime.paste_text("hi\r\nthere"));
    assert_eq!(selection(&runtime, nodes[0]).caret, 8);
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('z'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    }));
    let NodeKind::Text(text) = &runtime.tree().get(nodes[0]).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.text(), " world");
}

#[test]
fn visual_navigation_keeps_horizontal_goal_and_ctrl_down_moves_a_paragraph() {
    let (mut runtime, nodes, fonts) = fixture(true, &["abcdefghij\nx\nabcdefghij"]);
    let at = position(&runtime, nodes[0], &fonts, 7);
    down(&mut runtime, at, 1.0);
    up(&mut runtime, at);
    key(&mut runtime, NamedKey::Down, Modifiers::NONE);
    assert_eq!(selection(&runtime, nodes[0]).caret, 12);
    key(&mut runtime, NamedKey::Down, Modifiers::NONE);
    assert_eq!(selection(&runtime, nodes[0]).caret, 20);
    key(
        &mut runtime,
        NamedKey::Up,
        Modifiers::CONTROL.union(Modifiers::SHIFT),
    );
    assert_eq!(selection(&runtime, nodes[0]).anchor, 20);
    assert_eq!(selection(&runtime, nodes[0]).caret, 12);
}

#[test]
fn lost_focus_cancels_selection_capture_and_stale_nodes_do_not_copy() {
    let (mut runtime, nodes, fonts) = fixture(false, &["hello world"]);
    let start = position(&runtime, nodes[0], &fonts, 0);
    let end = position(&runtime, nodes[0], &fonts, 5);
    down(&mut runtime, start, 1.0);
    runtime.handle_input(&InputEvent::Focused(false));
    runtime.handle_input(&InputEvent::PointerMove(pointer(end, 1)));
    assert_eq!(runtime.selected_text(), None);
    runtime
        .step(aqua_protocol::Input::Tick(Instant::from_secs_f64(3.0)))
        .unwrap();
    runtime.refresh(Vec::new()).unwrap();
    assert_eq!(runtime.selected_text(), None);
}

#[test]
fn selecting_registered_sources_copies_offscreen_rows_and_keeps_exact_separators() {
    use aqua_text::document::{SelectionSource, SelectionSourceId, SourceSlice};
    use std::sync::Arc;

    let (mut runtime, nodes, fonts) = fixture(false, &["first", "last"]);
    for (id, text, separator) in [
        (1, "first", ""),
        (2, "offscreen", "\n\n"),
        (3, "last", "\n"),
    ] {
        runtime
            .register_selection_source(SelectionSource {
                id: SelectionSourceId(id),
                order: vec![id],
                text: Arc::from(text),
                separator: Arc::from(separator),
                copies: Vec::new(),
            })
            .unwrap();
    }
    assert!(runtime.bind_selection_source(
        nodes[0],
        SourceSlice {
            source: SelectionSourceId(1),
            range: 0..5
        }
    ));
    assert!(runtime.bind_selection_source(
        nodes[1],
        SourceSlice {
            source: SelectionSourceId(3),
            range: 0..4
        }
    ));
    let first = position(&runtime, nodes[0], &fonts, 0);
    let last = position(&runtime, nodes[1], &fonts, 4);
    down(&mut runtime, first, 1.0);
    runtime.handle_input(&InputEvent::PointerMove(pointer(last, 1)));
    up(&mut runtime, last);
    assert_eq!(
        runtime.selected_text().as_deref(),
        Some("first\n\noffscreen\nlast")
    );
    runtime.unbind_selection_source(nodes[0]);
    runtime.unbind_selection_source(nodes[1]);
    runtime.refresh(Vec::new()).unwrap();
    assert_eq!(
        runtime.selected_text().as_deref(),
        Some("first\n\noffscreen\nlast")
    );
    runtime.unregister_selection_source(SelectionSourceId(1));
    assert_eq!(runtime.selected_text(), None);
}

#[test]
fn registered_source_bindings_reject_gaps_and_ctrl_a_includes_unmounted_sources() {
    use aqua_text::document::{SelectionSource, SelectionSourceId, SourceSlice};
    use std::sync::Arc;

    let (mut runtime, nodes, fonts) = fixture(false, &["shown"]);
    for (id, text) in [(1, "shown"), (2, "hidden")] {
        runtime
            .register_selection_source(SelectionSource {
                id: SelectionSourceId(id),
                order: vec![id],
                text: Arc::from(text),
                separator: Arc::from("\n"),
                copies: Vec::new(),
            })
            .unwrap();
    }
    assert!(!runtime.bind_selection_source(
        nodes[0],
        SourceSlice {
            source: SelectionSourceId(2),
            range: 0..6
        }
    ));
    assert!(runtime.bind_selection_source(
        nodes[0],
        SourceSlice {
            source: SelectionSourceId(1),
            range: 0..5
        }
    ));
    let at = position(&runtime, nodes[0], &fonts, 2);
    down(&mut runtime, at, 1.0);
    up(&mut runtime, at);
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('a'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    }));
    assert_eq!(runtime.selected_text().as_deref(), Some("shown\nhidden"));
}

#[test]
fn stationary_drag_scrolls_on_frame_deadlines_and_stops_on_release() {
    use aqua_ui::element::Scroll;

    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            Scroll::new()
                .style(Style::new().width(px(220.0)).height(px(100.0)))
                .child(
                    UiBox::new()
                        .style(
                            Style::new()
                                .axis(Axis::Column)
                                .width(px(200.0))
                                .height(px(1200.0))
                                .flex_shrink(0.0),
                        )
                        .children((0..40).map(|index| {
                            UiText::builder()
                                .arg(format!("paragraph {index}"))
                                .selectable(true)
                                .style(
                                    Style::new()
                                        .width(px(200.0))
                                        .height(px(30.0))
                                        .flex_shrink(0.0),
                                )
                                .build()
                        }))
                        .build(),
                )
                .build()
        })
        .unwrap();
    runtime.set_viewport(300.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let root = runtime.tree().root().unwrap();
    down(&mut runtime, Point::new(5.0, 8.0), 1.0);
    let edge = Point::new(80.0, 110.0);
    runtime.handle_input(&InputEvent::PointerMove(pointer(edge, 1)));
    let first = runtime
        .step(aqua_protocol::Input::Frame(Instant::from_secs_f64(1.01)))
        .unwrap();
    assert!(first.wake_at.is_some());
    let before = runtime.tree().scroll_offset(root).unwrap().y;
    for frame in 2..12 {
        runtime
            .step(aqua_protocol::Input::Frame(Instant::from_secs_f64(
                1.0 + frame as f64 / 60.0,
            )))
            .unwrap();
    }
    assert!(runtime.tree().scroll_offset(root).unwrap().y > before + 20.0);
    assert!(runtime
        .selected_text()
        .is_some_and(|text| text.contains("paragraph 3")));
    up(&mut runtime, edge);
    let stopped = runtime
        .step(aqua_protocol::Input::Frame(Instant::from_secs_f64(1.3)))
        .unwrap();
    assert_eq!(stopped.wake_at, None);
    let offset = runtime.tree().scroll_offset(root).unwrap();
    runtime
        .step(aqua_protocol::Input::Frame(Instant::from_secs_f64(1.5)))
        .unwrap();
    assert_eq!(runtime.tree().scroll_offset(root), Some(offset));
    down(&mut runtime, Point::new(5.0, 8.0), 2.0);
    runtime.handle_input(&InputEvent::PointerMove(pointer(edge, 1)));
    let dragging = runtime
        .step(aqua_protocol::Input::Frame(Instant::from_secs_f64(2.02)))
        .unwrap();
    assert!(dragging.wake_at.is_some());
    let offset = runtime.tree().scroll_offset(root).unwrap();
    runtime.tree_mut().set_semantics(
        root,
        aqua_ui::tree::NodeSemantics {
            disabled: true,
            ..Default::default()
        },
    );
    let blocked = runtime
        .step(aqua_protocol::Input::Frame(Instant::from_secs_f64(2.1)))
        .unwrap();
    assert_eq!(blocked.wake_at, None);
    assert_eq!(runtime.tree().scroll_offset(root), Some(offset));
}

#[test]
fn large_read_only_transcript_selects_in_source_order_and_reuses_base_scene() {
    let messages: Vec<_> = (0..1000).map(|index| format!("message {index}")).collect();
    let references: Vec<_> = messages.iter().map(String::as_str).collect();
    let (mut runtime, nodes, fonts) = fixture(false, &references);
    runtime.set_viewport(400.0, 62000.0, 1.0);
    runtime.build_frame().unwrap();
    let first = position(&runtime, nodes[0], &fonts, 0);
    let last = position(&runtime, nodes[999], &fonts, messages[999].len());
    down(&mut runtime, first, 1.0);
    runtime.handle_input(&InputEvent::PointerMove(pointer(last, 1)));
    up(&mut runtime, last);
    assert_eq!(runtime.selected_text(), Some(messages.join("\n")));
    runtime.build_frame().unwrap();
    assert!(runtime.frame_work().tree_scene_reused);
    assert_eq!(runtime.frame_work().text_nodes_checked, 0);
}

#[test]
fn dragging_registered_selection_keeps_anchor_after_its_geometry_unmounts() {
    use aqua_text::document::{SelectionSource, SelectionSourceId, SourceSlice};
    use std::sync::Arc;

    let (mut runtime, nodes, fonts) = fixture(false, &["first", "middle", "last"]);
    for (index, text) in ["first", "middle", "last"].into_iter().enumerate() {
        let id = index as u64;
        runtime
            .register_selection_source(SelectionSource {
                id: SelectionSourceId(id),
                order: vec![id],
                text: Arc::from(text),
                separator: Arc::from("\n"),
                copies: Vec::new(),
            })
            .unwrap();
        assert!(runtime.bind_selection_source(
            nodes[index],
            SourceSlice {
                source: SelectionSourceId(id),
                range: 0..text.len()
            }
        ));
    }
    let first = position(&runtime, nodes[0], &fonts, 0);
    down(&mut runtime, first, 1.0);
    runtime.unbind_selection_source(nodes[0]);
    runtime.tree_mut().remove(nodes[0]);
    runtime.build_frame().unwrap();
    let last = position(&runtime, nodes[2], &fonts, 4);
    runtime.handle_input(&InputEvent::PointerMove(pointer(last, 1)));
    up(&mut runtime, last);
    assert_eq!(
        runtime.selected_text().as_deref(),
        Some("first\nmiddle\nlast")
    );
}

#[test]
fn visual_end_retains_upstream_wrap_affinity_for_ime_and_arrow_navigation() {
    use aqua_text::CaretAffinity;
    let (mut runtime, nodes, fonts) = fixture(
        true,
        &["alpha beta gamma delta epsilon zeta eta theta iota kappa"],
    );
    let start = position(&runtime, nodes[0], &fonts, 0);
    down(&mut runtime, start, 1.0);
    up(&mut runtime, start);
    let first = runtime.ime_anchor().unwrap();
    key(&mut runtime, NamedKey::End, Modifiers::NONE);
    let NodeKind::Text(text) = &runtime.tree().get(nodes[0]).unwrap().kind else {
        panic!()
    };
    let TextBacking::Editable(editor) = &text.backing else {
        panic!()
    };
    assert_eq!(editor.caret_position().affinity, CaretAffinity::Upstream);
    let byte = editor.selection().caret;
    assert!(byte > 0 && byte < text.text().len());
    runtime.build_frame().unwrap();
    let end = runtime.ime_anchor().unwrap();
    assert_eq!(end.y, first.y);
    key(&mut runtime, NamedKey::Right, Modifiers::NONE);
    assert!(runtime.ime_anchor().unwrap().y > end.y);
    key(&mut runtime, NamedKey::Left, Modifiers::NONE);
    assert!(runtime.ime_anchor().unwrap().y >= end.y);
    assert_eq!(selection(&runtime, nodes[0]).caret, byte);
    key(&mut runtime, NamedKey::Up, Modifiers::NONE);
    key(&mut runtime, NamedKey::End, Modifiers::NONE);
    key(&mut runtime, NamedKey::Right, Modifiers::SHIFT);
    assert!(!selection(&runtime, nodes[0]).is_caret());
}

#[test]
fn double_click_word_drag_grows_whole_words_and_reverses_without_losing_origin() {
    let (mut runtime, nodes, fonts) = fixture(true, &["alpha   beta gamma"]);
    let alpha = position(&runtime, nodes[0], &fonts, 2);
    let beta = position(&runtime, nodes[0], &fonts, 10);
    let gamma = position(&runtime, nodes[0], &fonts, 16);
    down(&mut runtime, beta, 1.0);
    up(&mut runtime, beta);
    down(&mut runtime, beta, 1.1);
    runtime.handle_input(&InputEvent::PointerMove(pointer(gamma, 1)));
    assert_eq!(runtime.selected_text().as_deref(), Some("beta gamma"));
    runtime.handle_input(&InputEvent::PointerMove(pointer(alpha, 1)));
    assert_eq!(runtime.selected_text().as_deref(), Some("alpha   beta "));
    up(&mut runtime, alpha);
}

fn selected_source_fixture() -> (Runtime, Vec<NodeId>, SharedFonts) {
    use aqua_text::document::{SelectionSource, SelectionSourceId, SourceSlice};
    let (mut runtime, nodes, fonts) = fixture(false, &["first", "last"]);
    for (index, text) in ["first", "last"].into_iter().enumerate() {
        let id = SelectionSourceId(index as u64);
        runtime
            .register_selection_source(SelectionSource {
                id,
                order: vec![index as u64],
                text: text.into(),
                separator: "\n".into(),
                copies: Vec::new(),
            })
            .unwrap();
        assert!(runtime.bind_selection_source(
            nodes[index],
            SourceSlice {
                source: id,
                range: 0..text.len()
            }
        ));
    }
    let at = position(&runtime, nodes[0], &fonts, 0);
    down(&mut runtime, at, 1.0);
    up(&mut runtime, at);
    assert!(runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('a'),
        modifiers: Modifiers::CONTROL,
        repeat: false
    })));
    assert_eq!(runtime.selected_text().as_deref(), Some("first\nlast"));
    (runtime, nodes, fonts)
}

#[test]
fn source_selection_does_not_copy_behind_a_new_modal() {
    use aqua_core::Rect;
    use aqua_ui::overlay::{DismissPolicy, Modality, OverlayKind, Placement};
    use aqua_ui::overlay_host::{OverlayHost, PopupRequest};
    use aqua_ui::style::{LayoutStyle, Length};
    use aqua_ui::tree::{BoxContent, NodeSpec};
    let (mut runtime, nodes, _) = selected_source_fixture();
    let mut host = std::mem::replace(
        runtime.host_mut(),
        OverlayHost::new(Rect::from_edges(0.0, 0.0, 400.0, 400.0)),
    );
    host.open_popup(
        runtime.tree_mut(),
        PopupRequest {
            owner: nodes[0],
            parent: None,
            kind: OverlayKind::Dialog,
            modality: Modality::Modal,
            dismiss: DismissPolicy::MANUAL,
            content: NodeSpec::new(
                NodeKind::Box(BoxContent::default()),
                LayoutStyle {
                    width: Length::Px(200.0),
                    height: Length::Px(100.0),
                    ..LayoutStyle::default()
                },
            ),
            placement: Placement::centered_dialog(),
            autofocus: None,
            focus_root: true,
        },
    )
    .unwrap();
    *runtime.host_mut() = host;
    assert_eq!(runtime.selected_text(), None);
}

#[test]
fn source_selection_rejects_disabled_owners_and_changed_mounted_text() {
    use aqua_ui::tree::NodeSemantics;
    let (mut runtime, nodes, _) = selected_source_fixture();
    let owner = runtime.tree().root().unwrap();
    runtime.tree_mut().set_semantics(
        owner,
        NodeSemantics {
            disabled: true,
            ..NodeSemantics::default()
        },
    );
    assert_eq!(runtime.selected_text(), None);
    runtime
        .tree_mut()
        .set_semantics(owner, NodeSemantics::default());
    assert_eq!(runtime.selected_text().as_deref(), Some("first\nlast"));
    runtime.tree_mut().set_text(nodes[1], "changed");
    assert_eq!(runtime.selected_text(), None);
}

#[test]
fn source_selection_rejects_removed_document_owner() {
    let (mut runtime, _, _) = selected_source_fixture();
    let owner = runtime.tree().root().unwrap();
    assert!(runtime.tree_mut().remove(owner));
    assert_eq!(runtime.selected_text(), None);
}

#[test]
fn disabled_editor_ignores_ime_updates_and_commits() {
    use aqua_core::ImeEvent;
    use aqua_ui::tree::NodeSemantics;
    let (mut runtime, nodes, fonts) = fixture(true, &["original"]);
    let at = position(&runtime, nodes[0], &fonts, 3);
    down(&mut runtime, at, 1.0);
    up(&mut runtime, at);
    runtime.tree_mut().set_semantics(
        nodes[0],
        NodeSemantics {
            disabled: true,
            ..NodeSemantics::default()
        },
    );
    runtime.handle_input(&InputEvent::Ime(ImeEvent::Update {
        marked: "に".into(),
        cursor: None,
    }));
    runtime.handle_input(&InputEvent::Ime(ImeEvent::Commit {
        text: "日本".into(),
    }));
    let NodeKind::Text(text) = &runtime.tree().get(nodes[0]).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.text(), "original");
    let TextBacking::Editable(editor) = &text.backing else {
        panic!()
    };
    assert!(!editor.is_composing());
}

#[test]
fn source_select_all_does_not_capture_ctrl_alt_shortcuts() {
    let (mut runtime, nodes, fonts) = fixture(false, &["first", "last"]);
    let first = position(&runtime, nodes[0], &fonts, 1);
    let last = position(&runtime, nodes[0], &fonts, 3);
    down(&mut runtime, first, 1.0);
    runtime.handle_input(&InputEvent::PointerMove(pointer(last, 1)));
    up(&mut runtime, last);
    let before = runtime.selected_text();
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('a'),
        modifiers: Modifiers::CONTROL.union(Modifiers::ALT),
        repeat: false,
    }));
    assert_eq!(runtime.selected_text(), before);
}

#[test]
fn modal_without_autofocus_blocks_background_editing_keys_and_ime() {
    use aqua_core::{ImeEvent, Rect};
    use aqua_ui::overlay::{DismissPolicy, Modality, OverlayKind, Placement};
    use aqua_ui::overlay_host::{OverlayHost, PopupRequest};
    use aqua_ui::style::{LayoutStyle, Length};
    use aqua_ui::tree::{BoxContent, NodeSpec};
    let (mut runtime, nodes, fonts) = fixture(true, &["original"]);
    let at = position(&runtime, nodes[0], &fonts, 3);
    down(&mut runtime, at, 1.0);
    up(&mut runtime, at);
    let mut host = std::mem::replace(
        runtime.host_mut(),
        OverlayHost::new(Rect::from_edges(0.0, 0.0, 400.0, 400.0)),
    );
    host.open_popup(
        runtime.tree_mut(),
        PopupRequest {
            owner: nodes[0],
            parent: None,
            kind: OverlayKind::Dialog,
            modality: Modality::Modal,
            dismiss: DismissPolicy::MANUAL,
            content: NodeSpec::new(
                NodeKind::Box(BoxContent::default()),
                LayoutStyle {
                    width: Length::Px(200.0),
                    height: Length::Px(100.0),
                    ..LayoutStyle::default()
                },
            ),
            placement: Placement::centered_dialog(),
            autofocus: None,
            focus_root: false,
        },
    )
    .unwrap();
    *runtime.host_mut() = host;
    key(&mut runtime, NamedKey::Backspace, Modifiers::NONE);
    runtime.handle_input(&InputEvent::Ime(ImeEvent::Commit {
        text: "日本".into(),
    }));
    let NodeKind::Text(text) = &runtime.tree().get(nodes[0]).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.text(), "original");
}

#[test]
fn typing_then_navigating_before_a_frame_uses_current_text_geometry() {
    let (mut runtime, nodes, fonts) = fixture(true, &[""]);
    let at = position(&runtime, nodes[0], &fonts, 0);
    down(&mut runtime, at, 1.0);
    runtime.handle_input(&InputEvent::TextInput {
        text: "abc".to_owned(),
    });
    key(&mut runtime, NamedKey::Home, Modifiers::NONE);
    assert_eq!(selection(&runtime, nodes[0]).caret, 0);
    key(&mut runtime, NamedKey::End, Modifiers::NONE);
    assert_eq!(selection(&runtime, nodes[0]).caret, 3);
    key(&mut runtime, NamedKey::Left, Modifiers::NONE);
    assert_eq!(selection(&runtime, nodes[0]).caret, 2);
}

#[test]
fn declared_source_documents_isolate_ctrl_a_and_rebind_recycled_text_nodes() {
    use aqua_text::document::{
        SelectionDocumentId, SelectionSource, SelectionSourceId, SourceSlice,
    };
    use std::sync::Arc;

    fn source(id: u64, text: &str) -> SelectionSource {
        SelectionSource {
            id: SelectionSourceId(id),
            order: vec![id],
            text: Arc::from(text),
            separator: Arc::from("\n"),
            copies: Vec::new(),
        }
    }
    fn leaf(text: &str, document: u64, source: u64) -> aqua_ui::element::Element {
        UiText::builder()
            .arg(text)
            .selection_source_in(
                SelectionDocumentId(document),
                SourceSlice {
                    source: SelectionSourceId(source),
                    range: 0..text.len(),
                },
            )
            .style(Style::new().width(px(220.0)).height(px(60.0)))
            .build()
    }
    let fonts = SharedFonts::new();
    let mut runtime = Runtime::new(fonts.clone(), SharedRuns::new());
    runtime
        .register_selection_sources([source(0, "conversation"), source(1, "offscreen reply")])
        .unwrap();
    runtime
        .register_selection_sources_in(
            SelectionDocumentId(1),
            [source(0, "inspector"), source(1, "offscreen inspector")],
        )
        .unwrap();
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().axis(Axis::Column))
                .children([leaf("conversation", 0, 0), leaf("inspector", 1, 0)])
                .build()
        })
        .unwrap();
    runtime.set_viewport(400.0, 400.0, 1.0);
    runtime.build_frame().unwrap();
    let nodes: Vec<_> = runtime
        .tree()
        .traverse()
        .into_iter()
        .filter(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Text(_)))
        .collect();
    let at = position(&runtime, nodes[0], &fonts, 2);
    down(&mut runtime, at, 1.0);
    up(&mut runtime, at);
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('a'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    }));
    assert_eq!(
        runtime.selected_text().as_deref(),
        Some("conversation\noffscreen reply")
    );
    let at = position(&runtime, nodes[1], &fonts, 2);
    down(&mut runtime, at, 2.0);
    up(&mut runtime, at);
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('a'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    }));
    assert_eq!(
        runtime.selected_text().as_deref(),
        Some("inspector\noffscreen inspector")
    );
    runtime
        .register_selection_source(source(1, "updated reply"))
        .unwrap();
    assert_eq!(
        runtime.selected_text().as_deref(),
        Some("inspector\noffscreen inspector")
    );
    runtime
        .refresh(vec![UiBox::new()
            .style(Style::new().axis(Axis::Column))
            .children([leaf("updated reply", 0, 1), leaf("inspector", 1, 0)])
            .build()])
        .unwrap();
    runtime.build_frame().unwrap();
    let nodes: Vec<_> = runtime
        .tree()
        .traverse()
        .into_iter()
        .filter(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Text(_)))
        .collect();
    let at = position(&runtime, nodes[0], &fonts, 2);
    down(&mut runtime, at, 3.0);
    up(&mut runtime, at);
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('a'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    }));
    assert_eq!(
        runtime.selected_text().as_deref(),
        Some("conversation\nupdated reply")
    );
}

#[test]
fn double_click_on_final_letter_right_half_selects_the_word_under_pointer() {
    let (mut runtime, nodes, fonts) = fixture(true, &["hello world"]);
    let left = position(&runtime, nodes[0], &fonts, 4);
    let right = position(&runtime, nodes[0], &fonts, 5);
    let at = Point::new(left.x + (right.x - left.x) * 0.8, left.y);
    down(&mut runtime, at, 1.0);
    up(&mut runtime, at);
    assert_eq!(selection(&runtime, nodes[0]).caret, 5);
    down(&mut runtime, at, 1.1);
    up(&mut runtime, at);
    assert_eq!(runtime.selected_text().as_deref(), Some("hello "));
}

#[test]
fn composing_ime_owns_editing_navigation_and_default_action_keys() {
    use aqua_core::ImeEvent;
    let (mut runtime, nodes, fonts) = fixture(true, &["prefix"]);
    let at = position(&runtime, nodes[0], &fonts, 6);
    down(&mut runtime, at, 1.0);
    up(&mut runtime, at);
    runtime.handle_input(&InputEvent::Ime(ImeEvent::Update {
        marked: "ni".into(),
        cursor: Some((2, 2)),
    }));
    for named in [
        NamedKey::Backspace,
        NamedKey::Delete,
        NamedKey::Left,
        NamedKey::Right,
        NamedKey::Up,
        NamedKey::Down,
        NamedKey::Home,
        NamedKey::End,
        NamedKey::Enter,
        NamedKey::Tab,
        NamedKey::Escape,
    ] {
        key(&mut runtime, named, Modifiers::NONE);
    }
    for character in ['a', 'z', 'y'] {
        assert!(runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
            key: Key::Char(character),
            modifiers: Modifiers::CONTROL,
            repeat: false
        })));
    }
    assert!(!runtime.paste_text("clipboard"));
    runtime.handle_input(&InputEvent::TextInput {
        text: "raw-key-text".into(),
    });
    let NodeKind::Text(text) = &runtime.tree().get(nodes[0]).unwrap().kind else {
        panic!()
    };
    let TextBacking::Editable(editor) = &text.backing else {
        panic!()
    };
    assert_eq!(editor.text(), "prefix");
    assert_eq!(editor.display_text(), "prefixni");
    assert!(editor.is_composing());
    assert_eq!(runtime.tree().focused(), Some(nodes[0]));
    runtime.handle_input(&InputEvent::Ime(ImeEvent::Update {
        marked: "".into(),
        cursor: None,
    }));
    runtime.handle_input(&InputEvent::Ime(ImeEvent::Commit { text: "你".into() }));
    let NodeKind::Text(text) = &runtime.tree().get(nodes[0]).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.text(), "prefix你");
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('z'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    }));
    let NodeKind::Text(text) = &runtime.tree().get(nodes[0]).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.text(), "prefix");
}

#[test]
fn empty_preedit_restores_replaced_selection_before_following_commit() {
    use aqua_core::ImeEvent;
    let (mut runtime, nodes, fonts) = fixture(true, &["original"]);
    let at = position(&runtime, nodes[0], &fonts, 0);
    down(&mut runtime, at, 1.0);
    up(&mut runtime, at);
    runtime
        .tree_mut()
        .set_text_selection(nodes[0], Some(Selection::new(0, 8)));
    runtime.handle_input(&InputEvent::Ime(ImeEvent::Update {
        marked: "candidate".into(),
        cursor: None,
    }));
    runtime.handle_input(&InputEvent::Ime(ImeEvent::Update {
        marked: "".into(),
        cursor: None,
    }));
    let NodeKind::Text(text) = &runtime.tree().get(nodes[0]).unwrap().kind else {
        panic!()
    };
    let TextBacking::Editable(editor) = &text.backing else {
        panic!()
    };
    assert!(!editor.is_composing());
    assert_eq!(editor.text(), "original");
    assert_eq!(editor.selection(), Selection::new(0, 8));
    runtime.handle_input(&InputEvent::Ime(ImeEvent::Commit { text: "你".into() }));
    let NodeKind::Text(text) = &runtime.tree().get(nodes[0]).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.text(), "你");
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('z'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    }));
    let NodeKind::Text(text) = &runtime.tree().get(nodes[0]).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.text(), "original");
}

#[test]
fn multi_click_uses_configured_time_and_spatial_limits_without_losing_single_clicks() {
    let (mut runtime, nodes, fonts) = fixture(true, &["hello   world"]);
    let at = position(&runtime, nodes[0], &fonts, 2);
    runtime.set_click_settings(
        aqua_core::ClickSettings::new(std::time::Duration::from_millis(250), Point::new(1.0, 1.0))
            .unwrap(),
    );
    down(&mut runtime, at, 1.0);
    up(&mut runtime, at);
    down(&mut runtime, at, 1.2);
    up(&mut runtime, at);
    assert_eq!(runtime.selected_text().as_deref(), Some("hello   "));
    down(&mut runtime, at, 1.6);
    up(&mut runtime, at);
    assert!(runtime.selected_text().is_none());
    let moved = Point::new(at.x + 2.0, at.y);
    down(&mut runtime, moved, 1.7);
    up(&mut runtime, moved);
    assert!(runtime.selected_text().is_none());
    down(&mut runtime, moved, 1.8);
    up(&mut runtime, moved);
    assert_eq!(runtime.selected_text().as_deref(), Some("hello   "));
}
