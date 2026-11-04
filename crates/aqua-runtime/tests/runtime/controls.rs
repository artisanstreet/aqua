use std::cell::RefCell;
use std::rc::Rc;

use aqua_core::{
    InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId, Point, PointerButton, PointerEvent,
    PointerKind,
};
use aqua_runtime::{PopupOptions, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::{
    controls::{Control, SelectOption, SliderRange},
    element::ui,
    overlay::CloseReason,
    style::{px, Axis, Style},
    tree::NodeKind,
};

fn key(key: NamedKey) -> InputEvent {
    InputEvent::KeyDown(KeyEvent {
        key: Key::Named(key),
        modifiers: Modifiers::NONE,
        repeat: false,
    })
}
fn pointer(at: Point, down: bool, contact: u64) -> InputEvent {
    let event = PointerEvent {
        contact,
        kind: PointerKind::Mouse,
        position: at,
        buttons: u8::from(down),
    };
    if down {
        InputEvent::PointerDown(event, PointerButton::Primary)
    } else {
        InputEvent::PointerUp(event, PointerButton::Primary)
    }
}
fn mounted(element: aqua_ui::element::Element) -> Runtime {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.mount(|_| element).unwrap();
    runtime.set_viewport(600.0, 400.0, 1.0);
    runtime.build_frame().unwrap();
    runtime
}
fn text_node(runtime: &Runtime, expected: &str) -> NodeId {
    runtime.tree().traverse().into_iter().find(|node| matches!(&runtime.tree().get(*node).unwrap().kind, NodeKind::Text(text) if text.text() == expected)).unwrap()
}

#[test]
fn slider_normalizes_ranges_tracks_captured_pointer_and_routes_keyboard_steps() {
    let range = SliderRange::new(f64::NAN, 10.0, 0.0);
    assert_eq!(range.snap(f64::INFINITY), 0.0);
    assert_eq!(SliderRange::new(10.0, 0.0, 3.0).snap(10.0), 10.0);
    let changes = Rc::new(RefCell::new(Vec::new()));
    let recorded = changes.clone();
    let mut runtime = mounted(
        ui::Slider::new(10)
            .range(10.0, 20.0, 1.0)
            .style(Style::new().width(px(200.0)).height(px(24.0)))
            .on_change(move |value| recorded.borrow_mut().push(value))
            .build(),
    );
    runtime.handle_input(&pointer(Point::new(50.0, 12.0), true, 7));
    runtime.handle_input(&InputEvent::PointerMove(PointerEvent {
        contact: 7,
        kind: PointerKind::Mouse,
        position: Point::new(500.0, 12.0),
        buttons: 1,
    }));
    runtime.handle_input(&pointer(Point::new(500.0, 12.0), false, 7));
    runtime.handle_input(&key(NamedKey::Home));
    runtime.handle_input(&key(NamedKey::Right));
    runtime.handle_input(&key(NamedKey::PageUp));
    assert_eq!(*changes.borrow(), vec![13.0, 20.0, 10.0, 11.0, 20.0]);
    let root = runtime.tree().root().unwrap();
    runtime.tree_mut().get_mut(root).unwrap().semantics.disabled = true;
    runtime.handle_input(&key(NamedKey::Left));
    assert_eq!(*changes.borrow(), vec![13.0, 20.0, 10.0, 11.0, 20.0]);
}

#[test]
fn select_skips_disabled_options_opens_bound_portal_and_restores_focus_after_commit() {
    let changes = Rc::new(RefCell::new(Vec::new()));
    let recorded = changes.clone();
    let mut runtime = mounted(
        ui::Dropdown::new("Engine")
            .options(vec![
                SelectOption::new("a", "Alpha"),
                SelectOption::new("b", "Beta").disabled(true),
                SelectOption::new("g", "Gamma"),
            ])
            .selected(0)
            .on_select(move |index| recorded.borrow_mut().push(index))
            .style(Style::new().width(px(220.0)).height(px(32.0)))
            .build(),
    );
    let owner = runtime.tree().root().unwrap();
    runtime.tree_mut().set_focused(Some(owner));
    runtime.handle_input(&key(NamedKey::Down));
    assert_eq!(*changes.borrow(), vec![2]);
    runtime.handle_input(&key(NamedKey::Enter));
    runtime.build_frame().unwrap();
    assert!(runtime.tree().select_options(owner).unwrap().is_open());
    runtime.handle_input(&key(NamedKey::Home));
    runtime.handle_input(&key(NamedKey::Enter));
    assert_eq!(*changes.borrow(), vec![2, 0]);
    assert!(!runtime.tree().select_options(owner).unwrap().is_open());
    assert_eq!(runtime.tree().focused(), Some(owner));
    runtime.handle_input(&pointer(Point::new(20.0, 16.0), true, 0));
    runtime.handle_input(&pointer(Point::new(20.0, 16.0), false, 0));
    runtime.build_frame().unwrap();
    let gamma = runtime.tree().traverse().into_iter().rfind(|node| matches!(&runtime.tree().get(*node).unwrap().kind, NodeKind::Text(text) if text.text() == "Gamma")).unwrap();
    let bounds = runtime.tree().rect(gamma).unwrap();
    let at = Point::new(bounds.x + 3.0, bounds.y + 4.0);
    runtime.handle_input(&pointer(at, true, 0));
    runtime.handle_input(&pointer(at, false, 0));
    assert_eq!(*changes.borrow(), vec![2, 0, 2]);
    assert_eq!(runtime.tree().focused(), Some(owner));
}

#[test]
fn tabs_use_roving_keyboard_focus_and_emit_selection_before_tab_exits() {
    let changes = Rc::new(RefCell::new(Vec::new()));
    let recorded = changes.clone();
    let mut runtime = mounted(
        ui::Box::new()
            .style(Style::new().axis(Axis::Column))
            .child(
                ui::Tabs::new()
                    .tabs(vec![
                        "General".into(),
                        "Appearance".into(),
                        "Advanced".into(),
                    ])
                    .on_select(move |index| recorded.borrow_mut().push(index))
                    .build(),
            )
            .child(
                ui::Button::new()
                    .child(ui::Text::builder().arg("after").build())
                    .build(),
            )
            .build(),
    );
    let tabs = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(runtime.tree().control(*node), Some(Control::Tabs(_))))
        .unwrap();
    let children = runtime.tree().get(tabs).unwrap().children.clone();
    runtime.tree_mut().set_focused(Some(children[0]));
    runtime.handle_input(&key(NamedKey::Right));
    assert_eq!(runtime.tree().focused(), Some(children[1]));
    runtime.handle_input(&key(NamedKey::End));
    assert_eq!(runtime.tree().focused(), Some(children[2]));
    assert_eq!(*changes.borrow(), vec![1, 2]);
    runtime.handle_input(&key(NamedKey::Tab));
    assert_eq!(
        runtime.tree().focused(),
        runtime
            .tree()
            .get(text_node(&runtime, "after"))
            .unwrap()
            .parent
    );
}

#[test]
fn modal_bound_fields_survive_background_refresh_and_dispose_bindings_on_close() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    let (read, write) = runtime.scope().signal("draft".to_owned());
    runtime
        .mount(|_| {
            ui::Box::new()
                .style(Style::new().width(px(500.0)).height(px(350.0)))
                .child(
                    ui::Button::new()
                        .child(ui::Text::builder().arg("open").build())
                        .build()
                        .with_key(1u64),
                )
                .build()
        })
        .unwrap();
    runtime.set_viewport(600.0, 400.0, 1.0);
    runtime.build_frame().unwrap();
    let owner = runtime
        .tree()
        .get(text_node(&runtime, "open"))
        .unwrap()
        .parent
        .unwrap();
    runtime.tree_mut().set_focused(Some(owner));
    let closed = Rc::new(RefCell::new(Vec::new()));
    let closed_events = closed.clone();
    let popup = runtime
        .open_popup(
            owner,
            ui::Box::new()
                .style(
                    Style::new()
                        .width(px(260.0))
                        .height(px(140.0))
                        .axis(Axis::Column),
                )
                .child(ui::Text::builder().arg(read.clone()).build())
                .child(
                    ui::Text::builder()
                        .bind((read.clone(), write.clone()))
                        .style(Style::new().width(px(220.0)).height(px(32.0)))
                        .build(),
                )
                .build(),
            PopupOptions::dialog().on_close(move |reason| closed_events.borrow_mut().push(reason)),
        )
        .unwrap();
    runtime.build_frame().unwrap();
    runtime.handle_input(&InputEvent::TextInput {
        text: "!".to_owned(),
    });
    runtime.build_frame().unwrap();
    assert_eq!(read.get(), "!draft");
    runtime
        .refresh(vec![ui::Box::new()
            .style(Style::new().width(px(500.0)).height(px(350.0)))
            .child(
                ui::Button::new()
                    .child(ui::Text::builder().arg("open").build())
                    .build()
                    .with_key(1u64),
            )
            .build()])
        .unwrap();
    runtime.build_frame().unwrap();
    assert!(runtime.host().is_open(popup));
    runtime.handle_input(&key(NamedKey::Escape));
    assert_eq!(*closed.borrow(), vec![CloseReason::Escape]);
    assert_eq!(runtime.tree().focused(), Some(owner));
    let count = runtime.tree().traverse().len();
    write.set("after close".to_owned());
    runtime.build_frame().unwrap();
    assert_eq!(runtime.tree().traverse().len(), count);
    assert!(!runtime.close_popup(popup));
    assert_eq!(*closed.borrow(), vec![CloseReason::Escape]);
}

#[test]
fn keyed_aligned_reveal_finds_popup_and_rejects_ambiguous_keys() {
    use aqua_ui::collection::{CollectionRevealAlignment, List};
    use aqua_ui::element::Element;

    fn collection() -> Element {
        let mut descriptor = List::new()
            .items(0..100usize)
            .key(|index| *index)
            .row(|index| ui::Text::builder().arg(format!("line-{index}")).build())
            .style(Style::new().width(px(200.0)).height(px(120.0)))
            .estimated_row_height(px(24.0))
            .virtualize()
            .descriptor();
        descriptor.fixed_row_height = true;
        Element::collection(descriptor).with_key("resource-lines")
    }

    let mut runtime = mounted(
        ui::Box::new()
            .style(Style::new().width(px(500.0)).height(px(350.0)))
            .child(
                ui::Button::new()
                    .child(ui::Text::builder().arg("open").build())
                    .build(),
            )
            .build(),
    );
    runtime.set_viewport(600.0, 400.0, 1.0);
    runtime.build_frame().unwrap();
    let owner = runtime
        .tree()
        .get(text_node(&runtime, "open"))
        .unwrap()
        .parent
        .unwrap();
    let first = runtime
        .open_popup(owner, collection(), PopupOptions::dialog())
        .unwrap();
    assert!(!runtime.reveal_collection_item_aligned(
        "resource-lines",
        10,
        CollectionRevealAlignment::Start
    ));
    assert!(runtime
        .collection_item_target_aligned("resource-lines", 10, CollectionRevealAlignment::Start)
        .is_none());
    runtime.build_frame().unwrap();
    let initial = runtime
        .tree()
        .scroll_offset(runtime.host().popup_root(first).unwrap())
        .unwrap();
    assert_eq!(
        runtime
            .collection_item_target_aligned("resource-lines", 10, CollectionRevealAlignment::Start)
            .unwrap()
            .y,
        240.0
    );
    assert_eq!(
        runtime
            .tree()
            .scroll_offset(runtime.host().popup_root(first).unwrap())
            .unwrap(),
        initial
    );
    assert!(runtime.reveal_collection_item_aligned(
        "resource-lines",
        10,
        CollectionRevealAlignment::Start
    ));
    let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("resource-lines"));
    let first_node = runtime.host().popup_root(first).unwrap();
    assert_eq!(runtime.tree().get(first_node).unwrap().key, Some(key));
    assert_eq!(runtime.tree().scroll_offset(first_node).unwrap().y, 240.0);
    let second = runtime
        .open_popup(owner, collection(), PopupOptions::dialog())
        .unwrap();
    runtime.build_frame().unwrap();
    let before = runtime.tree().scroll_offset(first_node).unwrap();
    assert!(runtime
        .collection_item_target_aligned("resource-lines", 20, CollectionRevealAlignment::Start)
        .is_none());
    assert!(!runtime.reveal_collection_item_aligned(
        "resource-lines",
        20,
        CollectionRevealAlignment::Start
    ));
    assert_eq!(runtime.tree().scroll_offset(first_node).unwrap(), before);
    assert!(runtime.close_popup(second));
    assert!(runtime.reveal_collection_item_aligned(
        "resource-lines",
        20,
        CollectionRevealAlignment::Start
    ));
    assert_eq!(runtime.tree().scroll_offset(first_node).unwrap().y, 480.0);
    assert!(runtime.close_popup(first));
    assert!(!runtime.reveal_collection_item_aligned(
        "resource-lines",
        10,
        CollectionRevealAlignment::Start
    ));
}

#[test]
fn declarative_popup_and_pointer_context_menu_mount_interactive_bound_content() {
    let activated = Rc::new(RefCell::new(0));
    let choices = activated.clone();
    let mut runtime = mounted(
        ui::Box::new()
            .style(
                Style::new()
                    .axis(Axis::Column)
                    .width(px(400.0))
                    .height(px(200.0)),
            )
            .child(
                ui::Button::new()
                    .style(Style::new().width(px(120.0)).height(px(32.0)))
                    .child(ui::Text::builder().arg("settings").build())
                    .build()
                    .popup(PopupOptions::dialog(), || {
                        ui::Box::new()
                            .style(Style::new().width(px(200.0)).height(px(80.0)))
                            .child(
                                ui::Text::builder()
                                    .arg("name")
                                    .editable(true)
                                    .style(Style::new().width(px(180.0)).height(px(32.0)))
                                    .build(),
                            )
                            .build()
                    }),
            )
            .child(
                ui::Button::new()
                    .style(Style::new().width(px(120.0)).height(px(32.0)))
                    .child(ui::Text::builder().arg("item").build())
                    .build()
                    .context_menu(move || {
                        let choices = choices.clone();
                        ui::Box::new()
                            .style(Style::new().width(px(140.0)).height(px(40.0)))
                            .child(
                                ui::Button::new()
                                    .style(Style::new().width(px(140.0)).height(px(32.0)))
                                    .child(ui::Text::builder().arg("inspect").build())
                                    .on_activate(move || *choices.borrow_mut() += 1)
                                    .build(),
                            )
                            .build()
                    }),
            )
            .build(),
    );
    runtime.handle_input(&pointer(Point::new(10.0, 12.0), true, 0));
    runtime.handle_input(&pointer(Point::new(10.0, 12.0), false, 0));
    runtime.build_frame().unwrap();
    runtime.handle_input(&InputEvent::TextInput {
        text: "new ".into(),
    });
    assert!(runtime.tree().traverse().iter().any(|node| matches!(&runtime.tree().get(*node).unwrap().kind, NodeKind::Text(text) if text.text() == "new name")));
    runtime.handle_input(&key(NamedKey::Escape));
    let at = Point::new(40.0, 45.0);
    runtime.handle_input(&InputEvent::PointerDown(
        PointerEvent {
            contact: 0,
            kind: PointerKind::Mouse,
            position: at,
            buttons: 2,
        },
        PointerButton::Secondary,
    ));
    runtime.build_frame().unwrap();
    let inspect = text_node(&runtime, "inspect");
    let rect = runtime.tree().rect(inspect).unwrap();
    assert!(rect.x >= at.x && rect.y >= at.y);
    runtime.handle_input(&key(NamedKey::Enter));
    assert_eq!(*activated.borrow(), 1);
}

#[test]
fn delayed_tooltip_uses_runtime_deadline_preserves_focus_and_cancels_on_leave() {
    use aqua_protocol::{Input, Instant};

    let mut runtime = mounted(
        ui::Button::new()
            .style(Style::new().width(px(120.0)).height(px(32.0)))
            .child(ui::Text::builder().arg("usage").build())
            .build()
            .tooltip(0.5, || {
                ui::Box::new()
                    .style(Style::new().width(px(200.0)).height(px(40.0)))
                    .child(ui::Text::builder().arg("remaining quota").build())
                    .build()
            }),
    );
    let owner = runtime.tree().root().unwrap();
    runtime.tree_mut().set_focused(Some(owner));
    runtime.advance_clock(1.0);
    runtime.handle_input(&InputEvent::PointerMove(PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(5.0, 8.0),
        buttons: 0,
    }));
    let pending = runtime
        .step(Input::Frame(Instant::from_secs_f64(1.1)))
        .unwrap();
    assert_eq!(pending.wake_at, Some(Instant::from_secs_f64(1.5)));
    assert!(runtime.host().render_plan().is_empty());
    runtime
        .step(Input::Frame(Instant::from_secs_f64(1.6)))
        .unwrap();
    assert!(!runtime.host().render_plan().is_empty());
    assert_eq!(runtime.tree().focused(), Some(owner));
    runtime.handle_input(&InputEvent::PointerMove(PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(400.0, 200.0),
        buttons: 0,
    }));
    assert!(runtime.host().render_plan().is_empty());
    assert_eq!(
        runtime
            .step(Input::Frame(Instant::from_secs_f64(2.0)))
            .unwrap()
            .wake_at,
        None
    );
}

#[test]
fn captured_resize_drag_clamps_and_continues_across_refresh_without_accumulating_deltas() {
    let changes = Rc::new(RefCell::new(Vec::new()));
    let recorded = changes.clone();
    let mut runtime = mounted(
        ui::ResizeHandle::new(Axis::Row, 240.0, 180.0, 400.0)
            .style(Style::new().width(px(8.0)).height(px(200.0)))
            .on_resize(move |value| recorded.borrow_mut().push(value))
            .build(),
    );
    runtime.handle_input(&pointer(Point::new(4.0, 20.0), true, 3));
    runtime.handle_input(&InputEvent::PointerMove(PointerEvent {
        contact: 3,
        kind: PointerKind::Mouse,
        position: Point::new(84.0, 20.0),
        buttons: 1,
    }));
    let recorded = changes.clone();
    runtime
        .refresh(vec![ui::ResizeHandle::new(Axis::Row, 320.0, 180.0, 400.0)
            .style(Style::new().width(px(8.0)).height(px(200.0)))
            .on_resize(move |value| recorded.borrow_mut().push(value))
            .build()])
        .unwrap();
    runtime.handle_input(&InputEvent::PointerMove(PointerEvent {
        contact: 3,
        kind: PointerKind::Mouse,
        position: Point::new(104.0, 20.0),
        buttons: 1,
    }));
    runtime.handle_input(&InputEvent::PointerMove(PointerEvent {
        contact: 3,
        kind: PointerKind::Mouse,
        position: Point::new(404.0, 20.0),
        buttons: 1,
    }));
    runtime.handle_input(&pointer(Point::new(404.0, 20.0), false, 3));
    assert_eq!(*changes.borrow(), vec![320.0, 340.0, 400.0]);
}

#[test]
fn accessibility_controls_apply_range_actions_and_select_expansion_through_same_state_machine() {
    use aqua_protocol::{A11yAction, A11yActionData, A11yActionRequest, Input};
    let changes = Rc::new(RefCell::new(Vec::new()));
    let output = changes.clone();
    let mut runtime = mounted(
        ui::Box::new()
            .style(Style::new().axis(Axis::Column))
            .child(
                ui::Slider::new(10)
                    .range(0.0, 20.0, 2.0)
                    .on_change(move |value| output.borrow_mut().push(value))
                    .build(),
            )
            .child(
                ui::Dropdown::new("Engine")
                    .options(vec![
                        SelectOption::new("a", "Alpha"),
                        SelectOption::new("b", "Beta"),
                    ])
                    .selected(0)
                    .build(),
            )
            .build(),
    );
    let slider = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(runtime.tree().control(*node), Some(Control::Slider(_))))
        .unwrap();
    for (action, data) in [
        (A11yAction::Increment, A11yActionData::None),
        (A11yAction::Decrement, A11yActionData::None),
        (A11yAction::SetValue, A11yActionData::Numeric(19.5)),
        (A11yAction::SetValue, A11yActionData::Numeric(f64::NAN)),
    ] {
        runtime
            .step(Input::Access(A11yActionRequest {
                node: slider,
                action,
                data,
            }))
            .unwrap();
    }
    assert_eq!(*changes.borrow(), vec![12.0, 10.0, 20.0]);
    let numeric = runtime
        .tree()
        .emit_semantics()
        .into_iter()
        .find(|semantic| semantic.node == slider)
        .unwrap()
        .numeric_range
        .unwrap();
    assert_eq!(
        (numeric.min, numeric.max, numeric.value, numeric.step),
        (0.0, 20.0, 20.0, 2.0)
    );
    let select = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(runtime.tree().control(*node), Some(Control::Select(_))))
        .unwrap();
    runtime
        .step(Input::Access(A11yActionRequest {
            node: select,
            action: A11yAction::Expand,
            data: A11yActionData::None,
        }))
        .unwrap();
    runtime.build_frame().unwrap();
    assert!(runtime.tree().select_options(select).unwrap().is_open());
    assert_eq!(
        runtime
            .tree()
            .emit_semantics()
            .into_iter()
            .find(|semantic| semantic.node == select)
            .unwrap()
            .expanded,
        Some(true)
    );
    runtime
        .step(Input::Access(A11yActionRequest {
            node: select,
            action: A11yAction::Collapse,
            data: A11yActionData::None,
        }))
        .unwrap();
    assert!(!runtime.tree().select_options(select).unwrap().is_open());
    assert!(runtime.host().render_plan().is_empty());
}

#[test]
fn popup_structural_results_update_without_losing_caret_and_stop_observing_when_closed() {
    use std::cell::Cell;
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    let (query, write) = runtime.scope().signal(String::new());
    let after_close_write = write.clone();
    let builds = Rc::new(Cell::new(0));
    let observed = builds.clone();
    runtime
        .mount(|_| {
            ui::Button::new()
                .style(Style::new().width(px(120.0)).height(px(32.0)))
                .child(ui::Text::builder().arg("palette").build())
                .build()
                .popup_reactive(PopupOptions::dialog(), (query.clone(),), move |(filter,)| {
                    observed.set(observed.get() + 1);
                    ui::Box::new()
                        .style(
                            Style::new()
                                .axis(Axis::Column)
                                .width(px(260.0))
                                .height(px(100.0)),
                        )
                        .child(
                            ui::Text::builder()
                                .bind((query.clone(), write.clone()))
                                .style(Style::new().width(px(240.0)).height(px(30.0)))
                                .build()
                                .with_key("query"),
                        )
                        .children(
                            ["settings", "sources"]
                                .into_iter()
                                .filter(|value| value.contains(&filter))
                                .map(|value| {
                                    ui::Button::new()
                                        .child(ui::Text::builder().arg(value).build())
                                        .build()
                                        .with_key(value)
                                }),
                        )
                        .build()
                })
        })
        .unwrap();
    runtime.set_viewport(600.0, 400.0, 1.0);
    runtime.build_frame().unwrap();
    runtime.handle_input(&pointer(Point::new(10.0, 10.0), true, 0));
    runtime.handle_input(&pointer(Point::new(10.0, 10.0), false, 0));
    runtime.build_frame().unwrap();
    let focused = runtime.tree().focused();
    runtime.handle_input(&InputEvent::TextInput { text: "s".into() });
    runtime.build_frame().unwrap();
    runtime.handle_input(&InputEvent::TextInput { text: "e".into() });
    runtime.build_frame().unwrap();
    assert_eq!(runtime.tree().focused(), focused);
    assert!(runtime.tree().traverse().iter().any(|node| matches!(&runtime.tree().get(*node).unwrap().kind, NodeKind::Text(text) if text.text() == "se")));
    assert!(!runtime.tree().traverse().iter().any(|node| matches!(&runtime.tree().get(*node).unwrap().kind, NodeKind::Text(text) if text.text() == "sources")));
    runtime.handle_input(&key(NamedKey::Escape));
    let after_close = builds.get();
    after_close_write.set("sources".into());
    runtime.build_frame().unwrap();
    assert_eq!(builds.get(), after_close);
}

#[test]
fn declarative_action_removed_by_refresh_does_not_leave_a_live_stale_callback() {
    use std::cell::Cell;
    let calls = Rc::new(Cell::new(0));
    let output = calls.clone();
    let mut runtime = mounted(
        ui::Button::new()
            .style(Style::new().width(px(120.0)).height(px(32.0)))
            .on_activate(move || output.set(output.get() + 1))
            .build(),
    );
    runtime.handle_input(&pointer(Point::new(10.0, 10.0), true, 0));
    runtime.handle_input(&pointer(Point::new(10.0, 10.0), false, 0));
    assert_eq!(calls.get(), 1);
    runtime
        .refresh(vec![ui::Button::new()
            .style(Style::new().width(px(120.0)).height(px(32.0)))
            .build()])
        .unwrap();
    runtime.handle_input(&pointer(Point::new(10.0, 10.0), true, 0));
    runtime.handle_input(&pointer(Point::new(10.0, 10.0), false, 0));
    assert_eq!(calls.get(), 1);
}

#[test]
fn keyboard_focus_ring_tracks_roving_focus_and_rejects_focus_inside_disabled_ancestors() {
    use aqua_core::{DisplayItem, Oklch};
    let color = Oklch::new(0.75, 0.12, 190.0);
    let mut runtime = mounted(
        ui::Box::new()
            .style(Style::new().axis(Axis::Column))
            .children(["first", "second"].into_iter().map(|name| {
                ui::Button::new()
                    .style(
                        Style::new()
                            .width(px(100.0))
                            .height(px(32.0))
                            .focus_ring(2.0, color),
                    )
                    .child(ui::Text::builder().arg(name).build())
                    .build()
            }))
            .build(),
    );
    key_focus(&mut runtime, NamedKey::Tab);
    let first = runtime
        .tree()
        .get(text_node(&runtime, "first"))
        .unwrap()
        .parent
        .unwrap();
    assert_eq!(runtime.tree().focused(), Some(first));
    assert!(runtime.tree_mut().emit_scene().iter().any(|(node, item)| *node == first && matches!(item, DisplayItem::Rect { border_width: 2.0, border_color, .. } if *border_color == color)));
    key_focus(&mut runtime, NamedKey::Tab);
    assert!(!runtime
        .tree_mut()
        .emit_scene()
        .iter()
        .any(|(node, item)| *node == first
            && matches!(
                item,
                DisplayItem::Rect {
                    border_width: 2.0,
                    ..
                }
            )));
    let root = runtime.tree().root().unwrap();
    runtime.tree_mut().get_mut(root).unwrap().semantics.disabled = true;
    assert!(!runtime.tree_mut().set_focused(Some(first)));
}

fn key_focus(runtime: &mut Runtime, named: NamedKey) {
    runtime.handle_input(&key(named));
    runtime.build_frame().unwrap();
}

#[test]
fn static_multiline_authoring_measures_wrapped_paragraphs_and_survives_refresh() {
    let body = "First paragraph has enough words to wrap in the available width.\n\nThe second paragraph is visible too.";
    let content = |multiline| {
        ui::Text::builder()
            .arg(body)
            .multiline(multiline)
            .style(Style::new().width(px(160.0)))
            .build()
    };
    let mut runtime = mounted(content(true));
    let root = runtime.tree().root().unwrap();
    let wrapped_height = runtime.tree().rect(root).unwrap().h;
    assert!(wrapped_height > 60.0);
    runtime.refresh(vec![content(true)]).unwrap();
    runtime.build_frame().unwrap();
    assert_eq!(runtime.tree().rect(root).unwrap().h, wrapped_height);
    runtime.refresh(vec![content(false)]).unwrap();
    runtime.build_frame().unwrap();
    assert!(runtime.tree().rect(root).unwrap().h < wrapped_height / 2.0);
}

#[test]
fn select_typeahead_matches_unicode_case_and_cycles_repeated_characters() {
    let changes = Rc::new(RefCell::new(Vec::new()));
    let output = changes.clone();
    let mut runtime = mounted(
        ui::Dropdown::new("City")
            .options(vec![
                SelectOption::new("a", "Ångström"),
                SelectOption::new("b", "Ålesund"),
                SelectOption::new("c", "Örn"),
            ])
            .selected(2)
            .on_select(move |index| output.borrow_mut().push(index))
            .build(),
    );
    let owner = runtime.tree().root().unwrap();
    runtime.tree_mut().set_focused(Some(owner));
    for character in ['Å', 'Å', 'Å'] {
        runtime.handle_input(&InputEvent::KeyDown(KeyEvent {
            key: Key::Char(character),
            modifiers: Modifiers::SHIFT,
            repeat: false,
        }));
    }
    assert_eq!(*changes.borrow(), vec![0, 1, 0]);
}
