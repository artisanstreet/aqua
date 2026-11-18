use aqua_core::NodeId;
use aqua_protocol::{A11yAction, A11yActionData, A11yActionRequest};
mod support;
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
    support::access(
        &mut runtime,
        A11yActionRequest {
            node,
            action: A11yAction::Other("set-text-selection"),
            data: A11yActionData::TextByteSelection {
                anchor_node: node,
                anchor_byte: 2,
                focus_node: node,
                focus_byte: 13,
            },
        },
    );
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
    support::access(
        &mut runtime,
        A11yActionRequest {
            node,
            action: A11yAction::Other("set-text-selection"),
            data: A11yActionData::TextByteSelection {
                anchor_node: node,
                anchor_byte: 0,
                focus_node: node,
                focus_byte: 6,
            },
        },
    );
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
    support::access(
        &mut runtime,
        A11yActionRequest {
            node,
            action: A11yAction::Other("set-text-selection"),
            data: A11yActionData::TextByteSelection {
                anchor_node: node,
                anchor_byte: 0,
                focus_node: node,
                focus_byte: 3,
            },
        },
    );
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
    support::access(
        &mut runtime,
        A11yActionRequest {
            node,
            action: A11yAction::Other("set-text-selection"),
            data: A11yActionData::TextByteSelection {
                anchor_node: node,
                anchor_byte: 0,
                focus_node: node,
                focus_byte: "👩‍💻".len(),
            },
        },
    );
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

#[test]
fn transformed_accessibility_bounds_and_character_geometry_follow_visual_positions() {
    use aqua_core::{Point, Transform2D};
    use aqua_ui::style::{px, Style};
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(600.0, 200.0, 1.5);
    runtime
        .mount(|_| {
            Text::new("Aéאב")
                .editable(true)
                .style(Style::new().width(px(200.0)).height(px(40.0)))
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let node = field(&runtime);
    let snapshot = |runtime: &Runtime| {
        runtime
            .a11y_nodes()
            .unwrap()
            .2
            .into_iter()
            .find(|item| item.semantic.node == node)
            .unwrap()
    };
    let before = snapshot(&runtime);
    let mut paint = runtime.tree().paint(node).unwrap();
    paint.transform = Some(Transform2D::scale(2.0, 2.0).then(Transform2D::translate(30.0, 10.0)));
    runtime.tree_mut().set_paint(node, paint);
    runtime.build_frame().unwrap();
    let after = snapshot(&runtime);
    let original = before.bounds.unwrap();
    let transformed = after.bounds.unwrap();
    assert!((transformed.x - (original.x * 2.0 + 45.0)).abs() < 0.01);
    assert!((transformed.y - (original.y * 2.0 + 15.0)).abs() < 0.01);
    assert!((transformed.w - original.w * 2.0).abs() < 0.01);
    let a = before.text.unwrap();
    let b = after.text.unwrap();
    for (a, b) in a.runs.iter().zip(&b.runs) {
        assert!((b.bounds.x - (a.bounds.x * 2.0 + 45.0)).abs() < 0.01);
        assert!((b.bounds.y - (a.bounds.y * 2.0 + 15.0)).abs() < 0.01);
        assert!((b.bounds.h - a.bounds.h * 2.0).abs() < 0.01);
        for (a, b) in a.character_positions.iter().zip(&b.character_positions) {
            assert!((b - a * 2.0).abs() < 0.01);
        }
        for (a, b) in a.character_widths.iter().zip(&b.character_widths) {
            assert!((b - a * 2.0).abs() < 0.01);
        }
    }
    runtime.tree_mut().set_focused(Some(node));
    let caret = runtime.ime_anchor().unwrap();
    let first = &b.runs[0];
    assert!((first.bounds.x - caret.x * 1.5).abs() < 0.01);
    assert!((first.bounds.y - caret.y * 1.5).abs() < 0.01);
    assert_eq!(
        runtime
            .tree()
            .visual_transform(node)
            .unwrap()
            .apply(Point::ZERO),
        Point::new(30.0, 10.0)
    );
}

#[test]
fn cached_accessible_text_tracks_edits_reflow_and_scale() {
    use aqua_ui::style::{px, Style};
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut runtime = Runtime::new(fonts.clone(), runs.clone());
    runtime.set_viewport(400.0, 300.0, 1.0);
    runtime
        .mount(|_| {
            Text::new("alpha beta gamma delta")
                .editable(true)
                .multiline(true)
                .style(Style::new().width(px(200.0)))
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let node = field(&runtime);
    let text = |runtime: &Runtime| {
        runtime
            .a11y_nodes()
            .unwrap()
            .2
            .into_iter()
            .find(|item| item.semantic.node == node)
            .unwrap()
            .text
            .unwrap()
    };
    let initial = text(&runtime);
    assert_eq!(text(&runtime), initial);
    runtime.tree_mut().set_text(node, "changed é👩‍💻 אב words");
    runtime.build_frame().unwrap();
    let edited = text(&runtime);
    assert_ne!(edited.runs, initial.runs);
    assert_eq!(
        edited
            .runs
            .iter()
            .map(|run| run.text.as_str())
            .collect::<String>(),
        "changed é👩‍💻 אב words"
    );
    runtime
        .tree_mut()
        .set_style(node, Style::new().width(px(75.0)).resolve_layout());
    runtime.build_frame().unwrap();
    let wrapped = text(&runtime);
    assert!(wrapped.runs.len() > edited.runs.len());
    runtime.set_viewport(400.0, 300.0, 2.0);
    runtime.build_frame().unwrap();
    let scaled = text(&runtime);
    assert!(scaled.runs[0].bounds.h > wrapped.runs[0].bounds.h * 1.5);
    assert_eq!(text(&runtime), scaled);
}

mod semantics {
    use super::support;
    use aqua_core::{NodeId, NumericRangeSemantics, Point, Role};
    use aqua_protocol::{A11yAction, A11yActionData, A11yActionRequest, A11yNode, A11yScroll};
    use aqua_runtime::{PopupOptions, Runtime};
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::ui::{Box as UiBox, Scroll, Text};
    use aqua_ui::style::{px, Axis, Style};
    use aqua_ui::tree::AccessibilityRequest;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn nodes(runtime: &Runtime) -> Vec<A11yNode> {
        runtime.a11y_nodes().unwrap().2
    }

    fn labelled(runtime: &Runtime, label: &str) -> Option<NodeId> {
        nodes(runtime)
            .into_iter()
            .find(|node| node.semantic.label == label)
            .map(|node| node.semantic.node)
    }

    fn request(node: NodeId, action: A11yAction, data: A11yActionData) -> A11yActionRequest {
        A11yActionRequest { node, action, data }
    }

    #[test]
    fn an_open_dialog_hides_the_page_behind_it_from_assistive_tech() {
        let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
        runtime.set_viewport(300.0, 200.0, 1.0);
        runtime
            .mount(|_| {
                UiBox::new()
                    .child(
                        UiBox::new()
                            .style(Style::new().width(px(60.0)).height(px(20.0)))
                            .build()
                            .role(Role::Button)
                            .aria_label("Background action")
                            .focusable(true),
                    )
                    .build()
            })
            .unwrap();
        support::resize(&mut runtime, 300.0, 200.0, 1.0);
        runtime.build_frame().unwrap();
        assert!(labelled(&runtime, "Background action").is_some());
        let owner = runtime.tree().root().unwrap();
        let dialog = runtime
            .open_popup(
                owner,
                UiBox::new()
                    .style(Style::new().width(px(120.0)).height(px(80.0)))
                    .child(Text::builder().arg("Delete this thread?").build())
                    .build()
                    .role(Role::Dialog)
                    .aria_label("Confirm"),
                PopupOptions::dialog(),
            )
            .unwrap();
        runtime.build_frame().unwrap();
        assert!(labelled(&runtime, "Background action").is_none());
        assert!(labelled(&runtime, "Delete this thread?").is_some());
        let root = runtime.host().popup_root(dialog).unwrap();
        assert!(nodes(&runtime)
            .iter()
            .any(|node| node.semantic.node == root && node.semantic.modal));
    }

    #[test]
    fn custom_ranges_and_context_menus_accept_assistive_requests() {
        let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
        runtime.set_viewport(300.0, 200.0, 1.0);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        runtime
            .mount(move |_| {
                UiBox::new()
                    .style(Style::new().width(px(100.0)).height(px(20.0)))
                    .build()
                    .role(Role::Slider)
                    .aria_label("Position")
                    .aria_range(NumericRangeSemantics {
                        min: 0.0,
                        max: 60.0,
                        value: 10.0,
                        step: 5.0,
                    })
                    .focusable(true)
                    .on_accessibility_request(move |request| sink.borrow_mut().push(request))
            })
            .unwrap();
        runtime.build_frame().unwrap();
        let slider = labelled(&runtime, "Position").unwrap();
        for (action, data) in [
            (A11yAction::Increment, A11yActionData::None),
            (A11yAction::Decrement, A11yActionData::None),
            (A11yAction::SetValue, A11yActionData::Numeric(42.0)),
            (A11yAction::SetValue, A11yActionData::Value(" 12.5 ".into())),
            (A11yAction::SetValue, A11yActionData::Value("half".into())),
            (A11yAction::ShowContextMenu, A11yActionData::None),
        ] {
            support::access(&mut runtime, request(slider, action, data));
        }
        assert_eq!(
            *seen.borrow(),
            vec![
                AccessibilityRequest::Increment,
                AccessibilityRequest::Decrement,
                AccessibilityRequest::SetValue(42.0),
                AccessibilityRequest::SetValue(12.5),
                AccessibilityRequest::ShowContextMenu,
            ]
        );
    }

    fn scroller() -> Runtime {
        let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
        runtime.set_viewport(200.0, 150.0, 1.0);
        runtime
            .mount(|_| {
                Scroll::new()
                    .style(Style::new().width(px(200.0)).height(px(100.0)))
                    .child(
                        UiBox::new()
                            .style(
                                Style::new()
                                    .axis(Axis::Column)
                                    .width(px(200.0))
                                    .height(px(800.0))
                                    .flex_shrink(0.0),
                            )
                            .children((0..20).map(|row| {
                                UiBox::new()
                                    .style(
                                        Style::new()
                                            .width(px(200.0))
                                            .height(px(40.0))
                                            .flex_shrink(0.0),
                                    )
                                    .build()
                                    .role(Role::Button)
                                    .aria_label(format!("Row {row}"))
                                    .focusable(true)
                            }))
                            .build(),
                    )
                    .build()
                    .role(Role::ScrollRegion)
                    .aria_label("Results")
            })
            .unwrap();
        support::resize(&mut runtime, 200.0, 150.0, 1.0);
        runtime.build_frame().unwrap();
        runtime
    }

    fn scroll_y(runtime: &Runtime) -> (f64, f64) {
        let region = nodes(runtime)
            .into_iter()
            .find(|node| node.semantic.label == "Results")
            .unwrap();
        let scroll = region.semantic.scroll.unwrap();
        (scroll.y, scroll.y_max)
    }

    #[test]
    fn assistive_tech_can_read_and_move_scroll_positions() {
        let mut runtime = scroller();
        let region = labelled(&runtime, "Results").unwrap();
        assert_eq!(scroll_y(&runtime), (0.0, 700.0));
        let last = labelled(&runtime, "Row 19").unwrap();
        support::access(
            &mut runtime,
            request(last, A11yAction::ScrollIntoView, A11yActionData::None),
        );
        runtime.build_frame().unwrap();
        assert_eq!(scroll_y(&runtime).0, 700.0);
        support::access(
            &mut runtime,
            request(
                region,
                A11yAction::SetScrollOffset,
                A11yActionData::ScrollOffset { x: 0.0, y: 120.0 },
            ),
        );
        runtime.build_frame().unwrap();
        assert_eq!(scroll_y(&runtime).0, 120.0);
        support::access(
            &mut runtime,
            request(
                region,
                A11yAction::Scroll(A11yScroll::Up),
                A11yActionData::None,
            ),
        );
        runtime.build_frame().unwrap();
        assert_eq!(scroll_y(&runtime).0, 32.5);
        let first = labelled(&runtime, "Row 0").unwrap();
        support::access(
            &mut runtime,
            request(first, A11yAction::ScrollIntoView, A11yActionData::None),
        );
        runtime.build_frame().unwrap();
        assert_eq!(scroll_y(&runtime).0, 0.0);
        assert_eq!(runtime.tree().scroll_offset(region), Some(Point::ZERO));
    }

}

mod links {
    use super::support;
    use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId};
    use aqua_protocol::{A11yAction, A11yActionData, A11yActionRequest};
    use aqua_runtime::Runtime;
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::ui::{Box as UiBox, Text};
    use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
    use aqua_ui::style::{px, Style};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn key(runtime: &mut Runtime, key: Key, modifiers: Modifiers) {
        let event = KeyEvent {
            key,
            modifiers,
            repeat: false,
        };
        runtime.handle_input(&InputEvent::KeyDown(event));
        runtime.handle_input(&InputEvent::KeyUp(event));
    }

    fn tab(runtime: &mut Runtime, reverse: bool) {
        let modifiers = match reverse {
            true => Modifiers::SHIFT,
            false => Modifiers::NONE,
        };
        key(runtime, Key::Named(NamedKey::Tab), modifiers);
    }

    fn page(activated: Rc<RefCell<Vec<(u64, bool)>>>) -> Runtime {
        let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
        runtime.set_viewport(400.0, 200.0, 1.0);
        runtime
            .mount(move |_| {
                UiBox::new()
                    .child(
                        UiBox::new()
                            .style(Style::new().width(px(40.0)).height(px(20.0)))
                            .build()
                            .aria_label("Before")
                            .focusable(true),
                    )
                    .child(
                        Text::builder()
                            .arg("Read the docs or the guide today")
                            .interaction_range(11, 9..13)
                            .interaction_range(12, 21..26)
                            .style(Style::new().width(px(300.0)).height(px(40.0)))
                            .build()
                            .focusable(true)
                            .on_input(
                                InputKind::TextRangeActivated,
                                EventPhase::Bubble,
                                move |event| {
                                    if let RoutedInput::TextRangeActivated { id, pointer, .. } =
                                        &event.input
                                    {
                                        activated.borrow_mut().push((*id, pointer.is_some()));
                                    }
                                    InputResponse::handled()
                                },
                            ),
                    )
                    .child(
                        UiBox::new()
                            .style(Style::new().width(px(40.0)).height(px(20.0)))
                            .build()
                            .aria_label("After")
                            .focusable(true),
                    )
                    .build()
            })
            .unwrap();
        support::resize(&mut runtime, 400.0, 200.0, 1.0);
        runtime.build_frame().unwrap();
        runtime
    }

    fn labelled(runtime: &Runtime, label: &str) -> NodeId {
        runtime
            .a11y_nodes()
            .unwrap()
            .2
            .into_iter()
            .find(|node| node.semantic.label == label)
            .unwrap()
            .semantic
            .node
    }

    fn links(runtime: &Runtime) -> Vec<(String, bool)> {
        runtime
            .a11y_nodes()
            .unwrap()
            .2
            .into_iter()
            .find_map(|node| node.text.filter(|text| !text.links.is_empty()))
            .unwrap()
            .links
            .into_iter()
            .map(|link| (link.label, link.focused))
            .collect()
    }

    #[test]
    fn tab_walks_the_links_inside_a_paragraph_and_enter_opens_one() {
        let activated = Rc::new(RefCell::new(Vec::new()));
        let mut runtime = page(Rc::clone(&activated));
        let before = labelled(&runtime, "Before");
        let after = labelled(&runtime, "After");
        assert!(runtime.tree_mut().set_focused(Some(before)));
        tab(&mut runtime, false);
        assert_eq!(
            links(&runtime),
            vec![("docs".to_owned(), true), ("guide".to_owned(), false)]
        );
        tab(&mut runtime, false);
        assert_eq!(
            links(&runtime),
            vec![("docs".to_owned(), false), ("guide".to_owned(), true)]
        );
        key(&mut runtime, Key::Named(NamedKey::Enter), Modifiers::NONE);
        assert_eq!(*activated.borrow(), vec![(12, false)]);
        tab(&mut runtime, false);
        assert_eq!(runtime.tree().focused(), Some(after));
        tab(&mut runtime, true);
        assert_eq!(
            links(&runtime),
            vec![("docs".to_owned(), false), ("guide".to_owned(), true)]
        );
        tab(&mut runtime, true);
        tab(&mut runtime, true);
        assert_eq!(runtime.tree().focused(), Some(before));
    }

    #[test]
    fn assistive_tech_activates_a_link_by_its_index() {
        let activated = Rc::new(RefCell::new(Vec::new()));
        let mut runtime = page(Rc::clone(&activated));
        let paragraph = runtime
            .a11y_nodes()
            .unwrap()
            .2
            .into_iter()
            .find(|node| {
                node.text
                    .as_ref()
                    .is_some_and(|text| !text.links.is_empty())
            })
            .unwrap()
            .semantic
            .node;
        support::access(
            &mut runtime,
            A11yActionRequest {
                node: paragraph,
                action: A11yAction::Press,
                data: A11yActionData::Custom(0),
            },
        );
        assert_eq!(*activated.borrow(), vec![(11, false)]);
        assert_eq!(runtime.tree().link_focus(), Some((paragraph, 0)));
    }
}
