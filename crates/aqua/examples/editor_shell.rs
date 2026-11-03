#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::sync::Arc;

use aqua::prelude::*;
use aqua_core::{Role, Transform2D};
use aqua_reactive::{ReadSignal, WriteSignal};
use aqua_ui::element::Element;
use aqua_ui::tree::ActionKind;

const PAGE: Oklch = Oklch::new(0.17, 0.008, 255.0);
const SURFACE: Oklch = Oklch::new(0.22, 0.009, 255.0);
const CONTROL: Oklch = Oklch::new(0.29, 0.012, 255.0);
const TEXT: Oklch = Oklch::new(0.95, 0.004, 255.0);
const MUTED: Oklch = Oklch::new(0.76, 0.012, 255.0);
const ACCENT: Oklch = Oklch::new(0.76, 0.13, 190.0);

#[derive(Clone)]
struct Model {
    tab: usize,
    scale: f64,
    engine: usize,
    attached: Vec<u64>,
    installed: Vec<u64>,
    sources: Vec<u64>,
    status: String,
}

#[derive(Clone)]
struct ThreadReference(u64);

type TextSignal = (ReadSignal<String>, WriteSignal<String>);

fn label(text: impl Into<String>) -> Element {
    ui_builders::Text::builder()
        .arg(text.into())
        .style(Style::new().text_color(TEXT))
        .build()
}

fn button(text: &str, action: impl Fn() + 'static) -> Element {
    ui_builders::Button::new()
        .style(
            Style::new()
                .width(pct(100.0))
                .height(px(36.0))
                .padding(px(9.0))
                .background(CONTROL)
                .focus_ring(1.5, ACCENT)
                .radius(px(6.0))
                .flex_shrink(0.0),
        )
        .child(label(text))
        .on_activate(action)
        .build()
}

fn heading(text: &str) -> Element {
    ui_builders::Text::builder()
        .arg(text)
        .style(Style::new().font_size(20.0).text_color(TEXT))
        .build()
        .role(Role::Heading(2))
}

fn panel(width: f32) -> aqua::ui::Box {
    ui_builders::Box::new().style(
        Style::new()
            .column()
            .width(px(width))
            .height(pct(100.0))
            .padding(px(16.0))
            .gap(12.0)
            .background(SURFACE)
            .flex_shrink(0.0),
    )
}

fn palette(query: TextSignal, model: WriteSignal<Model>, filter: String) -> Element {
    ui_builders::Box::new()
        .style(
            Style::new()
                .column()
                .width(px(420.0))
                .height(px(240.0))
                .padding(px(16.0))
                .gap(12.0)
                .background(SURFACE)
                .radius(px(10.0)),
        )
        .child(
            ui_builders::Text::builder()
                .bind(query)
                .placeholder("Search commands")
                .style(
                    Style::new()
                        .width(pct(100.0))
                        .height(px(40.0))
                        .padding(px(10.0))
                        .background(PAGE)
                        .focus_ring(1.5, ACCENT)
                        .text_color(TEXT),
                )
                .build()
                .with_key("command-query")
                .aria_label("Command search"),
        )
        .child(
            List::new()
                .items(
                    ["Open settings", "New workspace", "Manage sources"]
                        .into_iter()
                        .filter(|label| label.to_lowercase().contains(&filter.to_lowercase())),
                )
                .key(|label: &&str| *label)
                .row(move |name| {
                    let name = *name;
                    let model = model.clone();
                    button(name, move || {
                        model.update(|state| state.status = format!("Command: {name}"))
                    })
                })
                .style(Style::new().width(pct(100.0)).height(px(150.0)).gap(8.0))
                .build(),
        )
        .build()
}

fn chart() -> Element {
    ui_builders::Canvas::new().style(Style::new().width(pct(100.0)).height(px(150.0))).on_paint(|bounds| {
        let path = vec![PathCommand::Move(Point::new(0.0, 95.0)), PathCommand::Cubic { first: Point::new(90.0, 90.0), second: Point::new(100.0, 20.0), to: Point::new(180.0, 45.0) }, PathCommand::Cubic { first: Point::new(250.0, 75.0), second: Point::new(280.0, 10.0), to: Point::new(400.0, 20.0) }];
        let drawing = Drawing { view_box: Rect::from_edges(0.0, 0.0, 400.0, 120.0), paths: vec![PaintedPath::stroke(path, Stroke::new(Brush::Solid(ACCENT), 2.5))] };
        vec![DisplayItem::Vector { bounds, source: VectorSource::Drawing(Arc::new(drawing)), opacity: 1.0 }, DisplayItem::Vector { bounds: Rect::from_edges(bounds.x, bounds.y, bounds.x + 24.0, bounds.y + 24.0), source: VectorSource::tinted_svg("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'><path fill='#89cfc7' d='M3 18V6h3v12zm7 0V3h3v15zm7 0v-8h3v8z'/></svg>", ACCENT), opacity: 1.0 }]
    }).build().aria_label("Activity chart")
}

fn shell(
    state: Model,
    model: WriteSignal<Model>,
    composer: TextSignal,
    query: TextSignal,
    image: aqua::images::ImageHandle,
) -> Element {
    let commands_query = query.clone();
    let commands_model = model.clone();
    let commands = button("Commands", || {})
        .popup_reactive(
            PopupOptions::dialog(),
            (query.0.clone(),),
            move |(filter,)| palette(commands_query.clone(), commands_model.clone(), filter),
        )
        .shortcut(Shortcut {
            key: Key::Char('k'),
            modifiers: Modifiers::CONTROL,
            action: ActionKind::Activate,
            repeat: false,
        });
    let thread = button("Thread 42", || {})
        .on_input(InputKind::PointerDown, EventPhase::Bubble, |_| {
            InputResponse {
                drag: DragRequest::Start(DragPayload::new(ThreadReference(42))),
                ..InputResponse::handled()
            }
        })
        .tooltip(0.4, || {
            ui_builders::Box::new()
                .style(
                    Style::new()
                        .width(px(240.0))
                        .height(px(40.0))
                        .padding(px(10.0))
                        .background(CONTROL)
                        .focus_ring(1.5, ACCENT),
                )
                .child(label("Drag this thread into the composer"))
                .build()
        });
    let workspace = model.clone();
    let sidebar = panel(210.0)
        .child(heading("Workspace"))
        .child(button("New workspace", move || {
            workspace.update(|state| state.status = "Workspace created".into())
        }))
        .child(commands)
        .child(thread)
        .child(
            ui_builders::Text::builder()
                .arg("Recent conversations")
                .style(Style::new().text_color(MUTED))
                .build(),
        )
        .children(
            ["Aqua readiness", "Source adapters", "Typography"]
                .into_iter()
                .map(|name| {
                    let model = model.clone();
                    button(name, move || {
                        model.update(|state| state.status = format!("Opened {name}"))
                    })
                }),
        )
        .build();
    let attach = model.clone();
    let compose = ui_builders::Text::builder().bind(composer).multiline(true).placeholder("Write a message…")
        .style(Style::new().width(pct(100.0)).height(px(100.0)).padding(px(12.0)).background(CONTROL).focus_ring(1.5, ACCENT).radius(px(8.0)).text_color(TEXT)).build().aria_label("Composer")
        .on_input(InputKind::DragOver, EventPhase::Bubble, |event| InputResponse { accept_drop: matches!(&event.input, RoutedInput::DragOver { payload, .. } if payload.get::<ThreadReference>().is_some()), ..Default::default() })
        .on_input(InputKind::Drop, EventPhase::Bubble, move |event| { if let RoutedInput::Drop { payload, .. } = &event.input { if let Some(reference) = payload.get::<ThreadReference>() { attach.update(|state| { state.attached.push(reference.0); state.status = format!("Attached thread {}", reference.0); }); } } InputResponse::handled() });
    let main = ui_builders::Box::new().style(Style::new().column().width(px(0.0)).min_width(px(260.0)).flex_grow(1.0).height(pct(100.0)).padding(px(24.0)).gap(18.0))
        .child(heading("Aqua workspace"))
        .child(ui_builders::Text::builder().arg("Review the conversation, adjust settings, and attach a thread to your next message.").multiline(true).style(Style::new().width(pct(100.0)).text_color(MUTED)).build())
        .child(ui_builders::Scroll::new().style(Style::new().width(pct(100.0)).height(px(0.0)).flex_grow(1.0)).child(ui_builders::Box::new().style(Style::new().column().width(pct(100.0)).gap(16.0))
            .child(ui_builders::Text::builder().spans(vec![StyledSpan::new("The whole app has to work.\n\n", TextStyle { weight: FontWeight::BOLD, ..TextStyle::default() }), StyledSpan::new("Text selection, controls, collections, overlays and the native window share one retained UI tree. Try selecting across these paragraphs, opening the command palette, or moving a thread into the composer.\n\nKeyboard: Ctrl+K opens commands. Escape closes the active popup. Settings controls accept arrows, Home, End and Tab.", TextStyle::default())]).selectable(true).multiline(true).style(Style::new().width(pct(100.0)).text_color(TEXT).font_size(15.0)).build())
            .child(chart())
            .child(ui_builders::Media::new(image).fit(ImageFit::Contain).style(Style::new().width(px(180.0)).height(px(90.0)).radius(px(8.0))).build())
            .child(ui_builders::Text::builder().arg("Transform geometry").editable(true).style(Style::new().width(px(200.0)).height(px(32.0)).background(CONTROL).focus_ring(1.5, ACCENT).text_color(TEXT).transform(Transform2D::scale(1.5, 1.5).then(Transform2D::translate(30.0, 10.0)))).build().aria_label("Transformed text"))
            .build()).build())
        .child(label(format!("Attached threads: {:?}", state.attached)))
        .child(compose)
        .child(ui_builders::Text::builder().arg(state.status.clone()).style(Style::new().text_color(ACCENT)).build().aria_label("Status"))
        .build();
    let tabs = model.clone();
    let mut inspector = panel(290.0).child(heading("Inspector")).child(
        ui_builders::Tabs::new()
            .tabs(vec!["Settings".into(), "Sources".into(), "Market".into()])
            .tab_style(
                Style::new()
                    .padding(px(6.0))
                    .height(px(32.0))
                    .text_color(MUTED)
                    .radius(px(5.0)),
            )
            .selected_tab_style(
                Style::new()
                    .background(CONTROL)
                    .focus_ring(1.5, ACCENT)
                    .text_color(TEXT)
                    .border_of(1.0, ACCENT),
            )
            .selected(state.tab)
            .on_select(move |tab| tabs.update(|state| state.tab = tab))
            .style(Style::new().width(pct(100.0)).gap(8.0))
            .build(),
    );
    match state.tab {
        0 => {
            let volume = model.clone();
            let engine = model.clone();
            inspector = inspector
                .child(label(format!("Text scale: {}%", state.scale)))
                .child(
                    ui_builders::Slider::new(state.scale)
                        .range(50.0, 150.0, 5.0)
                        .on_change(move |value| volume.update(|state| state.scale = value))
                        .style(
                            Style::new()
                                .width(pct(100.0))
                                .height(px(24.0))
                                .background(CONTROL)
                                .focus_ring(1.5, ACCENT),
                        )
                        .build()
                        .aria_label("Text scale"),
                )
                .child(label("Model"))
                .child(
                    ui_builders::Dropdown::new("Engine")
                        .options(vec![
                            SelectOption::new("fast", "Fast"),
                            SelectOption::new("offline", "Unavailable").disabled(true),
                            SelectOption::new("precise", "Precise"),
                        ])
                        .selected(state.engine)
                        .on_select(move |value| engine.update(|state| state.engine = value))
                        .style(
                            Style::new()
                                .width(pct(100.0))
                                .height(px(36.0))
                                .padding(px(9.0))
                                .background(CONTROL)
                                .focus_ring(1.5, ACCENT),
                        )
                        .build()
                        .aria_label("Model engine"),
                );
        }
        1 => {
            inspector = inspector.child(
                List::new()
                    .items(state.sources)
                    .key(|id: &u64| *id)
                    .row(move |id| {
                        let id = *id;
                        let model = model.clone();
                        button(&format!("Remove source {id}"), move || {
                            model.update(|state| {
                                state.sources.retain(|source| *source != id);
                                state.status = format!("Removed source {id}");
                            })
                        })
                    })
                    .virtualize()
                    .estimated_row_height(px(36.0))
                    .overscan(2)
                    .style(Style::new().width(pct(100.0)).height(px(460.0)))
                    .build(),
            );
        }
        _ => {
            let installed = state.installed;
            inspector = inspector.child(
                List::new()
                    .items(0_u64..100)
                    .key(|id: &u64| *id)
                    .row(move |id| {
                        let id = *id;
                        let model = model.clone();
                        let label = if installed.contains(&id) {
                            format!("Installed extension {id}")
                        } else {
                            format!("Install extension {id}")
                        };
                        button(&label, move || {
                            model.update(|state| {
                                if !state.installed.contains(&id) {
                                    state.installed.push(id);
                                }
                                state.status = format!("Installed extension {id}");
                            })
                        })
                    })
                    .virtualize()
                    .estimated_row_height(px(36.0))
                    .overscan(2)
                    .style(Style::new().width(pct(100.0)).height(px(460.0)))
                    .build(),
            );
        }
    }
    ui_builders::Box::new()
        .style(
            Style::new()
                .row()
                .width(pct(100.0))
                .height(pct(100.0))
                .background(PAGE)
                .focus_ring(1.5, ACCENT)
                .text_color(TEXT),
        )
        .children([sidebar, main, inspector.build()])
        .build()
}

fn editor_shell(cx: &Context) -> impl IntoView {
    let (read, write) = cx.scope().signal(Model {
        tab: 0,
        scale: 100.0,
        engine: 0,
        attached: Vec::new(),
        installed: Vec::new(),
        sources: (0..30).collect(),
        status: "Ready".into(),
    });
    let composer = cx.scope().signal(String::new());
    let query = cx.scope().signal(String::new());
    let image = cx
        .images()
        .decode(Arc::<[u8]>::from(
            &include_bytes!("assets/sample_rgba.png")[..],
        ))
        .expect("valid bounded image fixture");
    cx.view((read,), move |(state,)| {
        shell(
            state,
            write.clone(),
            composer.clone(),
            query.clone(),
            image.clone(),
        )
    })
}

fn main() -> Result<(), aqua::Error> {
    Aqua::App()
        .config(
            Aqua::Config().window(
                Aqua::Window()
                    .title("Aqua — Editor shell")
                    .size(px(1240.0), px(800.0))
                    .min_size(px(900.0), px(600.0))
                    .theme(Theme::Dark)
                    .material(Material::Solid),
            ),
        )
        .run(editor_shell)
}
