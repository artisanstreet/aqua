use std::cell::RefCell;
use std::rc::Rc;

use aqua_core::{
    CursorIcon, DisplayItem, InputEvent, Key, KeyEvent, Modifiers, NamedKey, Point, PointerButton,
    PointerEvent, PointerKind,
};
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns, StyledSpan, TextStyle};
use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{px, Axis, Style};
use aqua_ui::tree::NodeKind;

fn pointer(at: Point, buttons: u8) -> PointerEvent {
    PointerEvent {
        contact: 42,
        kind: PointerKind::Pen,
        position: at,
        buttons,
    }
}

#[test]
fn public_text_links_route_id_range_and_modifiers_without_firing_on_drag() {
    let actions = Rc::new(RefCell::new(Vec::new()));
    let callback = actions.clone();
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().axis(Axis::Column))
                .child(
                    UiText::new("open reference")
                        .interaction_range(7, 0..4)
                        .style(Style::new().width(px(200.0)).height(px(40.0)))
                        .build()
                        .on_input(
                            InputKind::TextRangeActivated,
                            EventPhase::Bubble,
                            move |event| {
                                let RoutedInput::TextRangeActivated { id, range, pointer } =
                                    &event.input
                                else {
                                    panic!()
                                };
                                callback.borrow_mut().push((
                                    *id,
                                    range.clone(),
                                    pointer.contact,
                                    event.modifiers,
                                ));
                                InputResponse::handled()
                            },
                        ),
                )
                .build()
        })
        .unwrap();
    runtime.set_viewport(300.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let at = Point::new(5.0, 8.0);
    runtime.handle_input(&InputEvent::ModifiersChanged(Modifiers::CONTROL));
    runtime.handle_input(&InputEvent::PointerMove(pointer(at, 0)));
    assert_eq!(runtime.cursor(), CursorIcon::Pointer);
    runtime.handle_input(&InputEvent::PointerDown(
        pointer(at, 1),
        PointerButton::Primary,
    ));
    runtime.handle_input(&InputEvent::PointerUp(
        pointer(at, 0),
        PointerButton::Primary,
    ));
    assert_eq!(*actions.borrow(), vec![(7, 0..4, 42, Modifiers::CONTROL)]);
    runtime.advance_clock(1.0);
    runtime.handle_input(&InputEvent::PointerDown(
        pointer(at, 1),
        PointerButton::Primary,
    ));
    runtime.handle_input(&InputEvent::PointerMove(pointer(Point::new(90.0, 8.0), 1)));
    runtime.handle_input(&InputEvent::PointerUp(
        pointer(Point::new(90.0, 8.0), 0),
        PointerButton::Primary,
    ));
    assert_eq!(actions.borrow().len(), 1);
}

#[test]
fn platform_file_drop_reaches_public_typed_listener_at_hovered_target() {
    let files = Rc::new(RefCell::new(Vec::new()));
    let output = files.clone();
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().width(px(200.0)).height(px(100.0)))
                .build()
                .on_input(InputKind::FilesDropped, EventPhase::Bubble, move |event| {
                    let RoutedInput::FilesDropped { paths, .. } = &event.input else {
                        panic!()
                    };
                    output.borrow_mut().extend(paths.iter().cloned());
                    InputResponse::handled()
                })
        })
        .unwrap();
    runtime.set_viewport(300.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    runtime.handle_input(&InputEvent::PointerMove(pointer(Point::new(10.0, 10.0), 0)));
    let dropped = std::path::PathBuf::from("/tmp/attachment.png");
    assert!(runtime.drop_files(vec![dropped.clone()]));
    assert_eq!(*files.borrow(), vec![dropped]);
}

#[test]
fn styled_text_emits_retained_underlines_and_strikes_in_the_scene() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiText::builder()
                .spans(vec![StyledSpan::new(
                    "decorated",
                    TextStyle {
                        underline: true,
                        strikethrough: true,
                        ..Default::default()
                    },
                )
                .color([200, 30, 20, 255])])
                .build()
        })
        .unwrap();
    runtime.set_viewport(300.0, 100.0, 1.0);
    runtime.build_frame().unwrap();
    let text = runtime
        .scene()
        .ordered()
        .filter(|(_, item)| matches!(item, DisplayItem::Text { .. }))
        .count();
    let strokes: Vec<_> = runtime
        .scene()
        .ordered()
        .filter_map(|(_, item)| match item {
            DisplayItem::Rect { bounds, fill, .. } if bounds.h <= 2.0 && fill.alpha > 0.0 => {
                Some(*bounds)
            }
            _ => None,
        })
        .collect();
    assert_eq!(text, 1);
    assert_eq!(strokes.len(), 2);
    assert!(strokes[0].y != strokes[1].y);
}

#[test]
fn prevented_custom_key_does_not_run_editable_default_twice() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiText::new("abc").editable(true).build().on_input(
                InputKind::KeyDown,
                EventPhase::Capture,
                |event| match &event.input {
                    RoutedInput::Native(InputEvent::KeyDown(KeyEvent {
                        key: Key::Named(NamedKey::Backspace),
                        ..
                    })) => InputResponse::handled(),
                    _ => InputResponse::default(),
                },
            )
        })
        .unwrap();
    runtime.set_viewport(300.0, 100.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime.tree().root().unwrap();
    runtime.tree_mut().set_focused(Some(node));
    runtime.tree_mut().edit_text(node, |text| {
        if let aqua_ui::tree::TextBacking::Editable(editor) = &mut text.backing {
            editor.move_to_end(false);
        }
    });
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::Backspace),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
    let NodeKind::Text(text) = &runtime.tree().get(node).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.text(), "abc");
}

#[test]
fn overlay_routing_preserves_original_contact_and_button_without_duplicate_callbacks() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let output = seen.clone();
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().width(px(200.0)).height(px(100.0)))
                .build()
                .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
                    let RoutedInput::Native(InputEvent::PointerDown(pointer, button)) =
                        &event.input
                    else {
                        panic!()
                    };
                    output
                        .borrow_mut()
                        .push((pointer.contact, pointer.kind, *button));
                    InputResponse::default()
                })
        })
        .unwrap();
    runtime.set_viewport(300.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    runtime.handle_input(&InputEvent::PointerDown(
        pointer(Point::new(10.0, 10.0), 2),
        PointerButton::Secondary,
    ));
    assert_eq!(
        *seen.borrow(),
        vec![(42, PointerKind::Pen, PointerButton::Secondary)]
    );
}

#[test]
fn popup_outside_dismissal_swallows_underlying_custom_pointer_actions() {
    use aqua_core::Rect;
    use aqua_ui::overlay::{DismissPolicy, Modality, OverlayKind, Placement};
    use aqua_ui::overlay_host::{OverlayHost, PopupRequest};
    use aqua_ui::style::{LayoutStyle, Length};
    use aqua_ui::tree::{BoxContent, NodeSpec};
    let clicks = Rc::new(std::cell::Cell::new(0));
    let output = clicks.clone();
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().width(px(300.0)).height(px(300.0)))
                .build()
                .on_input(InputKind::PointerDown, EventPhase::Bubble, move |_| {
                    output.set(output.get() + 1);
                    InputResponse::default()
                })
        })
        .unwrap();
    runtime.set_viewport(300.0, 300.0, 1.0);
    runtime.build_frame().unwrap();
    let owner = runtime.tree().root().unwrap();
    let mut host = std::mem::replace(
        runtime.host_mut(),
        OverlayHost::new(Rect::from_edges(0.0, 0.0, 300.0, 300.0)),
    );
    let overlay = host
        .open_popup(
            runtime.tree_mut(),
            PopupRequest {
                owner,
                parent: None,
                kind: OverlayKind::Popover,
                modality: Modality::NonModal,
                dismiss: DismissPolicy::LIGHT,
                content: NodeSpec::new(
                    NodeKind::Box(BoxContent::default()),
                    LayoutStyle {
                        width: Length::Px(80.0),
                        height: Length::Px(60.0),
                        ..Default::default()
                    },
                ),
                placement: Placement::centered_dialog(),
                autofocus: None,
                focus_root: false,
            },
        )
        .unwrap();
    *runtime.host_mut() = host;
    runtime.build_frame().unwrap();
    runtime.handle_input(&InputEvent::PointerDown(
        pointer(Point::new(5.0, 5.0), 1),
        PointerButton::Primary,
    ));
    assert!(!runtime.host().is_open(overlay));
    assert_eq!(clicks.get(), 0);
}

#[test]
fn transformed_editable_pointer_and_ime_anchor_share_caret_geometry() {
    use aqua_core::Transform2D;
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiText::new("hello world")
                .editable(true)
                .style(Style::new().width(px(220.0)).height(px(40.0)).transform(
                    Transform2D::scale(2.0, 2.0).then(Transform2D::translate(50.0, 20.0)),
                ))
                .build()
        })
        .unwrap();
    runtime.set_viewport(600.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime.tree().root().unwrap();
    runtime.tree_mut().set_focused(Some(node));
    let start = runtime.ime_anchor().unwrap();
    assert!((start.x - 50.0).abs() < 0.1);
    runtime.handle_input(&InputEvent::PointerDown(
        pointer(Point::new(start.x + 1.0, start.y + 5.0), 1),
        PointerButton::Primary,
    ));
    let clicked = runtime.ime_anchor().unwrap();
    assert!((clicked.x - start.x).abs() < 0.1);
    runtime.handle_input(&InputEvent::PointerUp(
        pointer(Point::new(start.x + 1.0, start.y + 5.0), 0),
        PointerButton::Primary,
    ));
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::End),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
    let end = runtime.ime_anchor().unwrap();
    assert!(end.x > start.x + 50.0);
    assert!((end.w - start.w).abs() < 0.1);
    runtime.handle_input(&InputEvent::PointerDown(
        pointer(Point::new(end.x - 1.0, end.y + 5.0), 1),
        PointerButton::Primary,
    ));
    assert!((runtime.ime_anchor().unwrap().x - end.x).abs() < 0.1);
}

#[test]
fn autocomplete_uses_routed_editor_snapshot_and_undo_preserves_original_caret() {
    use aqua_ui::input::TextEdit;
    use aqua_ui::tree::UiCommand;
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    let (value, write) = runtime.scope().signal("hello @sa".to_owned());
    runtime
        .mount(|_| {
            UiText::builder()
                .bind((value.clone(), write))
                .build()
                .on_input(InputKind::KeyDown, EventPhase::Capture, |event| {
                    if !matches!(
                        &event.input,
                        RoutedInput::Native(InputEvent::KeyDown(KeyEvent {
                            key: Key::Named(NamedKey::Tab),
                            ..
                        }))
                    ) {
                        return InputResponse::default();
                    }
                    let editor = event.editor.as_ref().unwrap();
                    let start = editor.text.rfind('@').unwrap();
                    InputResponse {
                        commands: vec![UiCommand::EditText {
                            node: event.target,
                            revision: Some(editor.revision),
                            edit: TextEdit::ReplaceRange {
                                range: start..editor.selection.caret,
                                text: "@sander ".into(),
                            },
                        }],
                        ..InputResponse::handled()
                    }
                })
        })
        .unwrap();
    runtime.set_viewport(400.0, 100.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime.tree().root().unwrap();
    runtime.tree_mut().set_focused(Some(node));
    runtime.tree_mut().apply_text_edit(
        node,
        None,
        TextEdit::SetSelection(aqua_text::Selection::caret_at(9)),
    );
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::Tab),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
    let completed = runtime.tree().editor_snapshot(node).unwrap();
    assert_eq!(completed.text, "hello @sander ");
    assert_eq!(value.get(), "hello @sander ");
    assert_eq!(completed.selection.caret, completed.text.len());
    assert_eq!(runtime.tree().focused(), Some(node));
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Char('z'),
        modifiers: Modifiers::CONTROL,
        repeat: false,
    }));
    let undone = runtime.tree().editor_snapshot(node).unwrap();
    assert_eq!(undone.text, "hello @sa");
    assert_eq!(undone.selection, aqua_text::Selection::caret_at(9));
    assert!(!runtime.tree_mut().apply_text_edit(
        node,
        Some(completed.revision),
        TextEdit::Insert("stale".into())
    ));
    assert_eq!(
        runtime.tree().editor_snapshot(node).unwrap().text,
        "hello @sa"
    );
}

#[test]
fn prompt_intercepts_enter_to_submit_and_shift_enter_keeps_default_line_break() {
    let submitted = Rc::new(RefCell::new(Vec::new()));
    let output = submitted.clone();
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiText::new("draft")
                .editable(true)
                .multiline(true)
                .build()
                .on_input(
                    InputKind::KeyDown,
                    EventPhase::Capture,
                    move |event| match &event.input {
                        RoutedInput::Native(InputEvent::KeyDown(KeyEvent {
                            key: Key::Named(NamedKey::Enter),
                            modifiers: Modifiers::NONE,
                            repeat: false,
                        })) => {
                            output
                                .borrow_mut()
                                .push(event.editor.as_ref().unwrap().text.clone());
                            InputResponse::handled()
                        }
                        _ => InputResponse::default(),
                    },
                )
        })
        .unwrap();
    runtime.set_viewport(400.0, 100.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime.tree().root().unwrap();
    runtime.tree_mut().set_focused(Some(node));
    runtime.tree_mut().apply_text_edit(
        node,
        None,
        aqua_ui::input::TextEdit::SetSelection(aqua_text::Selection::caret_at(5)),
    );
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
    assert_eq!(*submitted.borrow(), vec!["draft"]);
    assert_eq!(runtime.tree().editor_snapshot(node).unwrap().text, "draft");
    runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers::SHIFT,
        repeat: false,
    }));
    assert_eq!(
        runtime.tree().editor_snapshot(node).unwrap().text,
        "draft\n"
    );
}

#[test]
fn platform_drop_position_overrides_stale_hover_position() {
    let hits = Rc::new(RefCell::new(Vec::new()));
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().axis(Axis::Column))
                .children((0..2).map(|index| {
                    let hits = hits.clone();
                    UiBox::new()
                        .style(Style::new().width(px(100.0)).height(px(50.0)))
                        .build()
                        .on_input(InputKind::FilesDropped, EventPhase::Bubble, move |_| {
                            hits.borrow_mut().push(index);
                            InputResponse::handled()
                        })
                }))
                .build()
        })
        .unwrap();
    runtime.set_viewport(300.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    runtime.handle_input(&InputEvent::PointerMove(pointer(Point::new(10.0, 10.0), 0)));
    assert!(runtime.drop_files_at(vec!["dropped.txt".into()], Some(Point::new(10.0, 75.0))));
    assert_eq!(*hits.borrow(), vec![1]);
}

#[test]
fn blur_cancels_preedit_and_modifiers_even_when_focus_callback_prevents_defaults() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let output = seen.clone();
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiText::new("before")
                .editable(true)
                .style(Style::new().width(px(200.0)).height(px(40.0)))
                .build()
                .on_input(InputKind::Focus, EventPhase::Capture, |_| {
                    InputResponse::handled()
                })
                .on_input(InputKind::PointerDown, EventPhase::Capture, move |event| {
                    output.borrow_mut().push(event.modifiers);
                    InputResponse::default()
                })
        })
        .unwrap();
    runtime.set_viewport(300.0, 100.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime.tree().root().unwrap();
    runtime.tree_mut().set_focused(Some(node));
    runtime.tree_mut().edit_editor(node, |editor| {
        editor.set_selection(aqua_text::Selection::new(0, 6));
        editor.set_composition("文", 3..3);
    });
    runtime.handle_input(&InputEvent::ModifiersChanged(Modifiers::SHIFT));
    runtime.handle_input(&InputEvent::Focused(false));
    let editor = runtime.tree().editor_snapshot(node).unwrap();
    assert!(!editor.composing);
    assert_eq!(editor.text, "before");
    runtime.handle_input(&InputEvent::PointerDown(
        pointer(Point::new(5.0, 5.0), 1),
        PointerButton::Primary,
    ));
    assert_eq!(*seen.borrow(), vec![Modifiers::NONE]);
}

#[test]
fn editable_mention_spans_expose_transformed_inline_cells_and_preserve_edit_history() {
    use aqua_core::Transform2D;
    use aqua_ui::input::TextEdit;
    let spans = |tail: &str| {
        vec![
            StyledSpan::plain("@")
                .color([0, 0, 0, 0])
                .inline_cell(20.0)
                .unwrap(),
            StyledSpan::plain(tail).color([40, 100, 220, 255]),
        ]
    };
    let style = Style::new()
        .width(px(220.0))
        .height(px(40.0))
        .transform(Transform2D::scale(1.5, 1.5).then(Transform2D::translate(30.0, 10.0)));
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            UiText::new("@name")
                .editable(true)
                .spans(spans("name"))
                .style(style.clone())
                .build()
        })
        .unwrap();
    runtime.set_viewport(500.0, 100.0, 1.0);
    runtime.build_frame().unwrap();
    let node = runtime.tree().root().unwrap();
    runtime.tree_mut().set_focused(Some(node));
    runtime.tree_mut().apply_text_edit(
        node,
        None,
        TextEdit::SetSelection(aqua_text::Selection::caret_at(1)),
    );
    let cell = runtime.text_range_rects(node, 0..1);
    assert_eq!(cell.len(), 1);
    assert!((cell[0].w - 30.0).abs() < 0.1);
    assert!((runtime.ime_anchor().unwrap().x - cell[0].right()).abs() < 0.1);
    runtime
        .tree_mut()
        .apply_text_edit(node, None, TextEdit::Insert("x".into()));
    assert!((runtime.text_range_rects(node, 0..1)[0].w - 30.0).abs() > 1.0);
    runtime
        .refresh(vec![UiText::new("@xname")
            .editable(true)
            .spans(spans("xname"))
            .style(style.clone())
            .build()])
        .unwrap();
    assert!((runtime.text_range_rects(node, 0..1)[0].w - 30.0).abs() < 0.1);
    runtime
        .tree_mut()
        .apply_text_edit(node, None, TextEdit::Undo);
    assert_eq!(runtime.tree().editor_snapshot(node).unwrap().text, "@name");
    runtime.tree_mut().edit_editor(node, |editor| {
        editor.set_composition("文", 3..3);
    });
    assert!(runtime.text_range_rects(node, 0..1).is_empty());
    runtime.tree_mut().edit_editor(node, |editor| {
        editor.cancel_composition();
    });
    runtime
        .refresh(vec![UiText::new("@name")
            .editable(true)
            .password(true)
            .spans(spans("name"))
            .style(style.clone())
            .build()])
        .unwrap();
    assert!(runtime.text_range_rects(node, 0..1).is_empty());
}
