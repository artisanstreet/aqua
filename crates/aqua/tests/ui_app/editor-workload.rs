use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use aqua::prelude::*;
use aqua::ui_app::{dispatch_clipboard, ClipboardOutcome};
use aqua_reactive::{ReadSignal, WriteSignal};
use aqua_shell_win32::services::MemoryClipboard;
use aqua_ui::element::Element;
use aqua_ui::tree::NodeKind;

#[derive(Clone)]
struct Message {
    id: u64,
    body: String,
    height: f32,
}

fn content(message: &Message) -> String {
    format!("Message {}: {}", message.id, message.body)
}

fn spans(message: &Message) -> Vec<StyledSpan> {
    vec![
        StyledSpan::new(format!("Message {}: ", message.id), TextStyle::default()),
        StyledSpan::new(
            message.body.clone(),
            TextStyle {
                weight: FontWeight::BOLD,
                ..TextStyle::default()
            },
        )
        .color([100, 180, 240, 255]),
    ]
}

fn source(message: &Message) -> SelectionSource {
    SelectionSource {
        id: SelectionSourceId(message.id),
        order: vec![message.id],
        text: Arc::from(content(message)),
        separator: Arc::from("\n\n"),
        copies: Vec::new(),
    }
}

fn view(
    messages: &[Message],
    composer: (ReadSignal<String>, WriteSignal<String>),
    rows_built: Rc<Cell<usize>>,
) -> Element {
    ui_builders::Box::new()
        .style(
            Style::new()
                .axis(Axis::Column)
                .width(px(760.0))
                .height(px(640.0)),
        )
        .child(
            List::new()
                .items(messages.iter().cloned())
                .key(|message: &Message| message.id)
                .row(move |message| {
                    rows_built.set(rows_built.get() + 1);
                    ui_builders::Text::builder()
                        .spans(spans(message))
                        .selection_source(SourceSlice {
                            source: SelectionSourceId(message.id),
                            range: 0..content(message).len(),
                        })
                        .multiline(true)
                        .style(
                            Style::new()
                                .width(px(740.0))
                                .min_height(px(message.height))
                                .flex_shrink(0.0),
                        )
                        .build()
                })
                .virtualize()
                .estimated_row_height(px(48.0))
                .overscan(4)
                .style(
                    Style::new()
                        .width(px(760.0))
                        .height(px(520.0))
                        .flex_shrink(0.0),
                )
                .build(),
        )
        .child(
            ui_builders::Text::builder()
                .bind(composer)
                .multiline(true)
                .style(
                    Style::new()
                        .width(px(740.0))
                        .height(px(100.0))
                        .flex_shrink(0.0),
                )
                .build(),
        )
        .build()
}

fn frame(app: &mut UiApp) {
    app.sync().unwrap();
    app.build_frame().unwrap();
}

fn source_nodes(app: &UiApp) -> Vec<(NodeId, u64)> {
    app.tree()
        .traverse()
        .into_iter()
        .filter_map(|node| {
            let NodeKind::Text(text) = &app.tree().get(node)?.kind else {
                return None;
            };
            if text.is_editable() {
                return None;
            }
            let id = text.selection_source.as_ref()?.slice.source.0;
            Some((node, id))
        })
        .collect()
}

fn visual_y(app: &UiApp, node: NodeId) -> f32 {
    let mut y = app.tree().rect(node).unwrap().y;
    let mut parent = app.tree().get(node).unwrap().parent;
    while let Some(node) = parent {
        let record = app.tree().get(node).unwrap();
        if let NodeKind::Scroll(scroll) = &record.kind {
            y -= scroll.offset.y;
        }
        parent = record.parent;
    }
    y
}

fn shortcut(key: Key, modifiers: Modifiers) -> InputEvent {
    InputEvent::KeyDown(KeyEvent {
        key,
        modifiers,
        repeat: false,
    })
}

fn pointer(at: Point) -> InputEvent {
    InputEvent::PointerDown(
        PointerEvent {
            contact: 0,
            kind: aqua_core::PointerKind::Mouse,
            position: at,
            buttons: 1,
        },
        PointerButton::Primary,
    )
}

#[test]
fn editor_conversation_workload_combines_virtualization_rich_text_selection_streaming_and_composer()
{
    let mut messages: Vec<_> = (0..20_000)
        .map(|id| Message {
            id,
            body: format!(
                "A retained response with emphasis, code token {id}, and streaming text."
            ),
            height: 36.0 + (id % 3) as f32 * 12.0,
        })
        .collect();
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut app = UiApp::new(fonts, runs.clone());
    let composer_initial = "alpha beta\ngamma delta";
    let (composer_read, composer_write) = app.scope().signal(composer_initial.to_owned());
    let rows_built = Rc::new(Cell::new(0));
    app.register_selection_sources(messages.iter().map(source))
        .unwrap();
    app.mount(|_| {
        view(
            &messages,
            (composer_read.clone(), composer_write),
            rows_built.clone(),
        )
    })
    .unwrap();
    app.set_viewport(800.0, 680.0, 1.0);
    frame(&mut app);
    assert!(source_nodes(&app).len() < 50);
    assert!(rows_built.get() < 100);

    let first = source_nodes(&app)
        .into_iter()
        .find(|(_, id)| *id == 0)
        .unwrap()
        .0;
    let target_y = visual_y(&app, first) + 8.0;
    app.handle_input(&pointer(Point::new(4.0, target_y)));
    app.handle_input(&shortcut(Key::Char('a'), Modifiers::CONTROL));
    let mut clipboard = MemoryClipboard::new();
    dispatch_clipboard(
        &mut app,
        &mut clipboard,
        &shortcut(Key::Char('c'), Modifiers::CONTROL),
    )
    .unwrap();
    let expected = messages
        .iter()
        .map(content)
        .collect::<Vec<_>>()
        .join("\n\n");
    assert_eq!(clipboard.get_text().unwrap(), Some(expected));

    let scroll = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(app.tree().get(*node).unwrap().kind, NodeKind::Scroll(_)))
        .unwrap();
    app.tree_mut().scroll_to(scroll, Point::new(0.0, 360_000.0));
    frame(&mut app);
    frame(&mut app);
    let resident = source_nodes(&app);
    let (anchor, anchor_id) = resident
        .iter()
        .copied()
        .find(|(node, _)| visual_y(&app, *node) > 60.0)
        .unwrap();
    let before = visual_y(&app, anchor);
    let (preceding, preceding_id) = resident
        .iter()
        .copied()
        .filter(|(node, id)| {
            *id < anchor_id && visual_y(&app, *node) + app.tree().rect(*node).unwrap().h <= 0.0
        })
        .max_by_key(|(_, id)| *id)
        .unwrap();
    let preceding_height = app.tree().rect(preceding).unwrap().h;
    messages[preceding_id as usize].body =
        "A growing streamed paragraph above the reading anchor. ".repeat(30);
    let updated = messages[preceding_id as usize].clone();
    app.tree_mut().edit_text(preceding, |text| {
        text.set_text(&content(&updated));
        text.spans = spans(&updated);
        if let Some(binding) = &mut text.selection_source {
            binding.slice.range.end = content(&updated).len();
        }
    });
    app.register_selection_source(source(&updated)).unwrap();
    frame(&mut app);
    frame(&mut app);
    let grown = source_nodes(&app)
        .into_iter()
        .find(|(_, id)| *id == preceding_id)
        .unwrap()
        .0;
    assert!(app.tree().rect(grown).unwrap().h > preceding_height * 2.0);
    let anchor = source_nodes(&app)
        .into_iter()
        .find(|(_, id)| *id == anchor_id)
        .unwrap()
        .0;
    assert!(
        (visual_y(&app, anchor) - before).abs() <= 1.0,
        "anchor {anchor_id} moved from {before} to {}",
        visual_y(&app, anchor)
    );

    app.tree_mut().scroll_to(scroll, Point::new(0.0, f32::MAX));
    frame(&mut app);
    frame(&mut app);
    let last = source_nodes(&app)
        .into_iter()
        .find(|(_, id)| *id == 19_999)
        .unwrap()
        .0;
    for token in [" More", " streamed", " output."] {
        messages[19_999].body.push_str(token);
        let updated = messages[19_999].clone();
        app.tree_mut().edit_text(last, |text| {
            text.set_text(&content(&updated));
            text.spans = spans(&updated);
            if let Some(binding) = &mut text.selection_source {
                binding.slice.range.end = content(&updated).len();
            }
        });
        app.register_selection_source(source(&updated)).unwrap();
        frame(&mut app);
        assert_eq!(app.frame_work().text_runs_shaped, 1);
        assert_eq!(app.frame_work().text_source_bytes, content(&updated).len());
    }
    let offset = app.tree().scroll_offset(scroll).unwrap();
    let bottom = app.tree().rect(last).unwrap().bottom();
    let viewport_bottom = app.tree().rect(scroll).unwrap().bottom();
    app.tree_mut()
        .scroll_to(scroll, Point::new(offset.x, bottom - viewport_bottom - 8.0));
    frame(&mut app);
    let NodeKind::Text(text) = &app.tree().get(last).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.spans, spans(&messages[19_999]));
    let text_item = app.scene().ordered().find_map(|(_, item)| match item {
        DisplayItem::Text { run, .. } if app.tree().get(last).is_some() => {
            let guard = runs.lock();
            guard
                .resolve(aqua_text::RunId(run.id))
                .filter(|handle| handle.layout.display_text() == content(&messages[19_999]))
                .map(|_| run.clone())
        }
        _ => None,
    });
    assert!(text_item.is_some());

    let composer = app.tree().traverse().into_iter().find(|node| matches!(&app.tree().get(*node).unwrap().kind, NodeKind::Text(text) if text.is_editable())).unwrap();
    let target_y = visual_y(&app, composer) + 8.0;
    app.handle_input(&pointer(Point::new(4.0, target_y)));
    app.handle_input(&shortcut(Key::Named(NamedKey::End), Modifiers::CONTROL));
    app.handle_input(&shortcut(Key::Named(NamedKey::Left), Modifiers::CONTROL));
    app.handle_input(&shortcut(
        Key::Named(NamedKey::Left),
        Modifiers::CONTROL.union(Modifiers::SHIFT),
    ));
    assert_eq!(app.selected_text().as_deref(), Some("gamma "));
    assert_eq!(
        dispatch_clipboard(
            &mut app,
            &mut clipboard,
            &shortcut(Key::Char('x'), Modifiers::CONTROL)
        )
        .unwrap(),
        ClipboardOutcome::Handled { redraw: true }
    );
    frame(&mut app);
    assert_eq!(composer_read.get(), "alpha beta\ndelta");
    clipboard.set_text("updated\r\n".to_owned()).unwrap();
    dispatch_clipboard(
        &mut app,
        &mut clipboard,
        &shortcut(Key::Char('v'), Modifiers::CONTROL),
    )
    .unwrap();
    frame(&mut app);
    assert_eq!(composer_read.get(), "alpha beta\nupdated\ndelta");
    for _ in 0..2 {
        app.handle_input(&shortcut(Key::Char('z'), Modifiers::CONTROL));
    }
    frame(&mut app);
    assert_eq!(composer_read.get(), composer_initial);
    frame(&mut app);
    assert!(app.frame_work().tree_scene_reused);
    assert_eq!(app.frame_work().text_source_bytes, 0);
    assert!(source_nodes(&app).len() < 50);
    assert!(rows_built.get() < 200);
}
