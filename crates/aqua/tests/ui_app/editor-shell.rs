use std::cell::{Cell, RefCell};
use std::rc::Rc;

use aqua::prelude::*;
use aqua_reactive::{ReadSignal, WriteSignal};
use aqua_ui::element::Element;
use aqua_ui::tree::NodeKind;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ThreadReference(u64);

#[derive(Clone)]
struct ShellState {
    volume: Rc<Cell<f64>>,
    engine: Rc<Cell<usize>>,
    settings_tab: Rc<Cell<usize>>,
    installed: Rc<RefCell<Vec<u64>>>,
    removed: Rc<RefCell<Vec<u64>>>,
    inspected: Rc<Cell<Option<u64>>>,
    attached: Rc<RefCell<Vec<ThreadReference>>>,
    command: Rc<RefCell<Option<String>>>,
    composer: (ReadSignal<String>, WriteSignal<String>),
    query: (ReadSignal<String>, WriteSignal<String>),
}

fn button(label: &str, callback: impl Fn() + 'static) -> Element {
    ui_builders::Button::new()
        .style(
            Style::new()
                .width(px(170.0))
                .height(px(30.0))
                .flex_shrink(0.0),
        )
        .child(ui_builders::Text::builder().arg(label).build())
        .on_activate(callback)
        .build()
}

fn column(width: f32) -> aqua::ui::Box {
    ui_builders::Box::new().style(
        Style::new()
            .axis(Axis::Column)
            .width(px(width))
            .height(px(600.0))
            .flex_shrink(0.0),
    )
}

fn shell(state: &ShellState) -> Element {
    let volume = state.volume.clone();
    let engine = state.engine.clone();
    let tab = state.settings_tab.clone();
    let inspector = state.inspected.clone();
    let drag =
        button("Thread 42", || {}).on_input(InputKind::PointerDown, EventPhase::Bubble, |_| {
            InputResponse {
                drag: DragRequest::Start(DragPayload::new(ThreadReference(42))),
                ..InputResponse::handled()
            }
        });
    let query = state.query.clone();
    let command = state.command.clone();
    let palette = button("Commands", || {}).popup_reactive(
        PopupOptions::dialog(),
        (query.0.clone(),),
        move |(filter,)| {
            let command = command.clone();
            ui_builders::Box::new()
                .style(
                    Style::new()
                        .axis(Axis::Column)
                        .width(px(320.0))
                        .height(px(160.0)),
                )
                .child(
                    ui_builders::Text::builder()
                        .bind(query.clone())
                        .style(Style::new().width(px(300.0)).height(px(32.0)))
                        .build(),
                )
                .child(
                    List::new()
                        .items(
                            ["Open settings", "New workspace", "Manage sources"]
                                .into_iter()
                                .filter(|label| {
                                    label.to_lowercase().contains(&filter.to_lowercase())
                                }),
                        )
                        .key(|label: &&str| *label)
                        .row(move |label| {
                            let command = command.clone();
                            let label = *label;
                            button(label, move || {
                                *command.borrow_mut() = Some(label.to_owned())
                            })
                        })
                        .style(Style::new().width(px(300.0)).height(px(100.0)))
                        .build(),
                )
                .build()
        },
    );
    let sidebar = column(180.0)
        .child(drag)
        .child(palette)
        .child(button("Workspace", || {}))
        .child(button("Inspect thread", move || inspector.set(Some(42))))
        .build();
    let settings = column(220.0)
        .child(
            ui_builders::Tabs::new()
                .tabs(vec!["General".into(), "Appearance".into()])
                .selected(state.settings_tab.get())
                .on_select(move |value| tab.set(value))
                .build(),
        )
        .child(
            ui_builders::Slider::new(state.volume.get())
                .range(0.0, 100.0, 5.0)
                .on_change(move |value| volume.set(value))
                .style(Style::new().width(px(200.0)).height(px(24.0)))
                .build()
                .aria_label("Text scale"),
        )
        .child(
            ui_builders::Dropdown::new("Engine")
                .options(vec![
                    SelectOption::new("fast", "Fast"),
                    SelectOption::new("disabled", "Unavailable").disabled(true),
                    SelectOption::new("precise", "Precise"),
                ])
                .selected(state.engine.get())
                .on_select(move |value| engine.set(value))
                .style(Style::new().width(px(200.0)).height(px(30.0)))
                .build(),
        )
        .build();
    let installed = state.installed.clone();
    let marketplace = column(180.0)
        .child(
            List::new()
                .items(0_u64..40)
                .key(|id: &u64| *id)
                .row(move |id| {
                    let installed = installed.clone();
                    let id = *id;
                    button(&format!("Install {id}"), move || {
                        installed.borrow_mut().push(id)
                    })
                })
                .virtualize()
                .estimated_row_height(px(30.0))
                .overscan(2)
                .style(Style::new().width(px(180.0)).height(px(300.0)))
                .build(),
        )
        .build();
    let removed = state.removed.clone();
    let sources = column(180.0)
        .child(
            List::new()
                .items(0_u64..20)
                .key(|id: &u64| *id)
                .row(move |id| {
                    let removed = removed.clone();
                    let id = *id;
                    button(&format!("Source {id}"), move || {
                        removed.borrow_mut().push(id)
                    })
                })
                .virtualize()
                .estimated_row_height(px(30.0))
                .overscan(2)
                .style(Style::new().width(px(180.0)).height(px(300.0)))
                .build(),
        )
        .build();
    let attached = state.attached.clone();
    let composer = ui_builders::Text::builder().bind(state.composer.clone()).multiline(true)
        .style(Style::new().width(px(220.0)).height(px(100.0))).build()
        .on_input(InputKind::DragOver, EventPhase::Bubble, |event| InputResponse {
            accept_drop: matches!(&event.input, RoutedInput::DragOver { payload, .. } if payload.get::<ThreadReference>().is_some()),
            ..Default::default()
        })
        .on_input(InputKind::Drop, EventPhase::Bubble, move |event| {
            if let RoutedInput::Drop { payload, .. } = &event.input {
                if let Some(reference) = payload.get::<ThreadReference>() { attached.borrow_mut().push(reference.clone()); }
            }
            InputResponse::handled()
        });
    let inspected = state.inspected.get();
    let inspector = column(230.0)
        .child(
            ui_builders::Text::builder()
                .arg(format!("Inspector {inspected:?}"))
                .build(),
        )
        .child(composer)
        .build();
    ui_builders::Box::new()
        .style(
            Style::new()
                .axis(Axis::Row)
                .width(px(1000.0))
                .height(px(600.0)),
        )
        .children([sidebar, settings, marketplace, sources, inspector])
        .build()
}

fn node(app: &UiApp, label: &str) -> NodeId {
    app.tree().traverse().into_iter().find(|node| matches!(&app.tree().get(*node).unwrap().kind, NodeKind::Text(text) if text.text() == label)).unwrap()
}

fn key(app: &mut UiApp, key: NamedKey) {
    app.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(key),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
}

fn pointer(app: &mut UiApp, point: Point, down: bool) {
    let event = PointerEvent {
        contact: 9,
        kind: aqua_core::PointerKind::Mouse,
        position: point,
        buttons: u8::from(down),
    };
    app.handle_input(&if down {
        InputEvent::PointerDown(event, PointerButton::Primary)
    } else {
        InputEvent::PointerUp(event, PointerButton::Primary)
    });
}

fn click(app: &mut UiApp, label: &str) {
    let rect = app.tree().visual_rect(node(app, label)).unwrap();
    let at = Point::new(rect.x + 2.0, rect.y + 3.0);
    pointer(app, at, true);
    pointer(app, at, false);
}

#[test]
fn editor_shell_surface_controls_lists_typed_drag_palette_and_modal_focus_work_together() {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    let state = ShellState {
        volume: Rc::new(Cell::new(50.0)),
        engine: Rc::new(Cell::new(0)),
        settings_tab: Rc::new(Cell::new(0)),
        installed: Rc::default(),
        removed: Rc::default(),
        inspected: Rc::new(Cell::new(None)),
        attached: Rc::default(),
        command: Rc::default(),
        composer: app.scope().signal("draft".to_owned()),
        query: app.scope().signal(String::new()),
    };
    app.mount(|_| shell(&state)).unwrap();
    app.set_viewport(1000.0, 600.0, 1.0);
    app.build_frame().unwrap();
    click(&mut app, "Install 1");
    click(&mut app, "Source 2");
    click(&mut app, "Inspect thread");
    assert_eq!(*state.installed.borrow(), vec![1]);
    assert_eq!(*state.removed.borrow(), vec![2]);
    assert_eq!(state.inspected.get(), Some(42));
    let slider = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(app.tree().control(*node), Some(Control::Slider(_))))
        .unwrap();
    app.tree_mut().set_focused(Some(slider));
    key(&mut app, NamedKey::Right);
    assert_eq!(state.volume.get(), 55.0);
    let select = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(app.tree().control(*node), Some(Control::Select(_))))
        .unwrap();
    app.tree_mut().set_focused(Some(select));
    key(&mut app, NamedKey::Enter);
    app.build_frame().unwrap();
    key(&mut app, NamedKey::Down);
    key(&mut app, NamedKey::Enter);
    assert_eq!(state.engine.get(), 2);
    let tab = app
        .tree()
        .get(node(&app, "General"))
        .unwrap()
        .parent
        .unwrap();
    app.tree_mut().set_focused(Some(tab));
    key(&mut app, NamedKey::Right);
    assert_eq!(state.settings_tab.get(), 1);
    app.refresh(vec![shell(&state)]).unwrap();
    app.build_frame().unwrap();
    let thread = app.tree().visual_rect(node(&app, "Thread 42")).unwrap();
    pointer(&mut app, Point::new(thread.x + 2.0, thread.y + 2.0), true);
    let composer = node(&app, "draft");
    let rect = app.tree().visual_rect(composer).unwrap();
    let at = Point::new(rect.x + 10.0, rect.y + 10.0);
    app.handle_input(&InputEvent::PointerMove(PointerEvent {
        contact: 9,
        kind: aqua_core::PointerKind::Mouse,
        position: at,
        buttons: 1,
    }));
    pointer(&mut app, at, false);
    assert_eq!(*state.attached.borrow(), vec![ThreadReference(42)]);
    click(&mut app, "Commands");
    app.build_frame().unwrap();
    app.handle_input(&InputEvent::TextInput {
        text: "settings".into(),
    });
    app.build_frame().unwrap();
    assert_eq!(state.query.0.get(), "settings");
    assert!(!app.tree().traverse().iter().any(|node| matches!(&app.tree().get(*node).unwrap().kind, NodeKind::Text(text) if text.text() == "New workspace")));
    assert_eq!(state.composer.0.get(), "draft");
    click(&mut app, "Open settings");
    assert_eq!(*state.command.borrow(), Some("Open settings".to_owned()));
    key(&mut app, NamedKey::Escape);
    assert!(app.host().render_plan().is_empty());
    app.tree_mut().set_focused(Some(composer));
    app.handle_input(&InputEvent::TextInput {
        text: "hello ".into(),
    });
    app.build_frame().unwrap();
    assert_eq!(state.composer.0.get(), "hello draft");
}
