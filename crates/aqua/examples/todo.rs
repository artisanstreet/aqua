//! Reactive todo list on the canonical `App` builder: the builder-first path.
//!
//! - `Aqua::App().config(Aqua::Config().window(Aqua::Window()...)).run(todo_app)`
//!   with `fn todo_app(cx: &Context)` returning `cx.view(explicit deps, render)`;
//!   `Context::scope()` owns signals, `Context::material().is_glass()` picks
//!   the glass styling. No hidden subscriptions: the dep tuple is the whole
//!   subscription set.
//! - Builder-first authoring with a real structural seam: `mount_reactive`
//!   rebuilds from explicit signal deps; keyed rows preserve node identity,
//!   focus, and edits across Add/remove.
//! - Two-way bound input (`bind`), targeted prop updates without rebuilds.
//! - A styled, responsive window by default; --headless runs the deterministic pump.
//!
//! Styling is fully canonical (`aqua::Style` arrays via `.styles([...])`):
//! no `LayoutStyle` literals, no separate paint props. Later entries win
//! per field; see `docs/STYLING.md`.
//!
//! Run windowed (standard): `cargo run -p aqua --example todo`
//! Run headless (CI, deterministic pump): `cargo run -p aqua --example todo -- --headless`

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use aqua::config::{Material, Theme, Titlebar};
use aqua::prelude::*;
use aqua::{auto, pct, px, Aqua, Context, IntoView, Style, UiApp};
use aqua_core::Insets;
use aqua_reactive::{ReadSignal, Scope};
use aqua_ui::element::Element;
use std::cell::RefCell;
use std::rc::Rc;
use taffy::style::{AlignItems, JustifyContent};

#[derive(Clone, Debug, PartialEq)]
struct Todo {
    id: u64,
    title: String,
    done: bool,
}

/// Explicit structural dependency: the todo list signal.
type TodoDeps = (ReadSignal<Vec<Todo>>, ReadSignal<bool>);
/// Structural render input: current todos, in dep order.
type TodoInput = (Vec<Todo>, bool);

// Shared, const styles. Native Acrylic supplies the desktop blur; these
// premultiplied-alpha surfaces add a quiet tint and edge highlights.
const INK: Oklch = Oklch::new(0.21, 0.006, 260.0);
const TEXT: Oklch = Oklch::WHITE.with_alpha(0.93);
const MUTED: Oklch = Oklch::WHITE.with_alpha(0.56);
const EDGE: Oklch = Oklch::WHITE.with_alpha(0.10);
const ACCENT: Oklch = Oklch::new(0.65, 0.17, 252.0);

const PAGE: Style = Aqua::Style()
    .width(pct(100.0))
    .height(pct(100.0))
    .column()
    .align(AlignItems::CENTER)
    .padding(px(28.0))
    .background(INK);
const GLASS_PAGE: Style = Aqua::Style().background(INK.with_alpha(0.26));
// Overlay-titlebar drag zone: empty ~40-logical strip at the very top.
// Transparent (glass shows through); native controls live top-right.
// Keeps content clear of them with no extra title text or eyebrows.
const TOP_STRIP: Style = Aqua::Style().width(pct(100.0)).height(px(40.0));
const CONTENT: Style = Aqua::Style()
    .width(pct(100.0))
    .max_width(px(640.0))
    .column()
    .gap(22.0);
const ROW: Style = Aqua::Style()
    .row()
    .align(AlignItems::CENTER)
    .gap(10.0)
    .width(pct(100.0));
const HEADER: Style = Aqua::Style()
    .row()
    .align(AlignItems::CENTER)
    .justify(JustifyContent::SPACE_BETWEEN)
    .width(pct(100.0));
const TASKS: Style = Aqua::Style().column().gap(8.0);
const HERO: Style = Aqua::Style().font_size(27.0).text_color(TEXT);
const PILL_BOX: Style = Aqua::Style()
    .background(Oklch::WHITE.with_alpha(0.06))
    .radius(px(9.0))
    .padding_insets(Insets::hv(10.0, 6.0));
const PILL_TX: Style = Aqua::Style().font_size(12.0).text_color(MUTED);

const BTN_BASE: Style = Aqua::Style()
    .align(AlignItems::CENTER)
    .justify(JustifyContent::CENTER)
    .flex_shrink(0.0)
    .clip(true);
const BTN_PRIMARY: Style = Aqua::Style()
    .width(px(76.0))
    .height(px(48.0))
    .background(ACCENT.with_alpha(0.88))
    .radius(px(12.0))
    .border_of(1.0, Oklch::WHITE.with_alpha(0.18));
const BTN_REMOVE: Style = Aqua::Style()
    .width(px(66.0))
    .height(px(30.0))
    .background(Oklch::WHITE.with_alpha(0.04))
    .radius(px(8.0));
const BTN_PRIMARY_TX: Style = Aqua::Style().font_size(14.0).text_color(TEXT);
const BTN_REMOVE_TX: Style = Aqua::Style().font_size(12.0).text_color(MUTED);

const FIELD_BOX: Style = Aqua::Style()
    .flex_grow(1.0)
    .min_width(px(0.0))
    .background(Oklch::WHITE.with_alpha(0.045))
    .radius(px(12.0))
    .clip(true);
const FIELD_FOCUS: Style = Aqua::Style().border_of(1.5, ACCENT.with_alpha(0.85));
const FIELD_IDLE: Style = Aqua::Style().border_of(1.0, EDGE);
const FIELD_TEXT: Style = Aqua::Style()
    .width(pct(100.0))
    .height(px(48.0))
    .padding(px(14.0))
    .font_size(16.0)
    .text_color(TEXT);

const CHECK_ROW: Style = Aqua::Style()
    .flex_grow(1.0)
    .width(px(0.0))
    .min_width(px(0.0))
    .min_height(px(34.0))
    .align(AlignItems::CENTER)
    .gap(12.0);
const TITLE_BASE: Style = Aqua::Style()
    .flex_grow(1.0)
    .width(px(0.0))
    .min_width(px(0.0))
    .font_size(15.0)
    .text_wrap(true);
const TITLE_TODO: Style = Aqua::Style().text_color(TEXT);
const TITLE_DONE: Style = Aqua::Style().text_color(MUTED);
const ROW_BASE: Style = Aqua::Style()
    .padding_insets(Insets {
        left: 14.0,
        right: 10.0,
        top: 9.0,
        bottom: 9.0,
    })
    .radius(px(12.0))
    .border_of(1.0, EDGE);
const ROW_TODO: Style = Aqua::Style().background(Oklch::WHITE.with_alpha(0.055));
const ROW_DONE: Style = Aqua::Style().background(Oklch::WHITE.with_alpha(0.025));
const EMPTY_BOX: Style = Aqua::Style()
    .height(px(136.0))
    .width(pct(100.0))
    .align(AlignItems::CENTER)
    .justify(JustifyContent::CENTER)
    .background(Oklch::WHITE.with_alpha(0.025))
    .radius(px(14.0))
    .border_of(1.0, Oklch::WHITE.with_alpha(0.055));
const EMPTY_TITLE: Style = Aqua::Style().font_size(15.0).text_color(MUTED);

fn action_button(label: &str, primary: bool, action: impl Fn() + 'static) -> Element {
    // Direct button paint: the Button carries background/radius/clip itself
    // through `.styles` — no decorative wrapper Box.
    let (variant, label_style) = if primary {
        (BTN_PRIMARY, BTN_PRIMARY_TX)
    } else {
        (BTN_REMOVE, BTN_REMOVE_TX)
    };
    Aqua::Button()
        .styles([BTN_BASE, variant])
        .on_activate(action)
        .child(Aqua::Text(label).styles([label_style]).build())
        .build()
}

fn todo_row(t: &Todo, todos_w: &aqua_reactive::WriteSignal<Vec<Todo>>) -> Element {
    let toggle_write = todos_w.clone();
    let toggle_id = t.id;
    let remove_write = todos_w.clone();
    let remove_id = t.id;
    let title_style = if t.done { TITLE_DONE } else { TITLE_TODO };
    let row_style = if t.done { ROW_DONE } else { ROW_TODO };
    Aqua::Box()
        .styles([ROW, ROW_BASE, row_style])
        .child(
            Aqua::Checkbox("")
                .checked(t.done)
                .styles([CHECK_ROW])
                .on_toggle(move || {
                    toggle_write.update(|v| {
                        if let Some(item) = v.iter_mut().find(|x| x.id == toggle_id) {
                            item.done = !item.done;
                        }
                    })
                })
                .child(
                    Aqua::Text(t.title.clone())
                        .styles([TITLE_BASE, title_style])
                        .build(),
                )
                .build(),
        )
        .child(action_button("Remove", false, move || {
            remove_write.update(|v| v.retain(|x| x.id != remove_id))
        }))
        .build()
        .with_key(t.id)
}

fn todo_view(
    scope: &Scope,
    initial: Vec<Todo>,
    glass: bool,
) -> (TodoDeps, impl FnMut(TodoInput) -> Element + 'static) {
    let (todos_r, todos_w) = scope.signal(initial);
    let (draft_r, draft_w) = scope.signal(String::new());
    let (focused_r, focused_w) = scope.signal(false);
    let next_id = Rc::new(RefCell::new(1u64));
    let add: Rc<dyn Fn()> = {
        let todos_w = todos_w.clone();
        let draft_r = draft_r.clone();
        let draft_w = draft_w.clone();
        Rc::new(move || {
            let title = draft_r.get().trim().to_string();
            if title.is_empty() {
                return;
            }
            let id = *next_id.borrow();
            *next_id.borrow_mut() = id + 1;
            todos_w.update(|v| {
                v.push(Todo {
                    id,
                    title,
                    done: false,
                })
            });
            draft_w.set(String::new());
        })
    };
    let render = move |(todos, focused): TodoInput| -> Element {
        let done = todos.iter().filter(|t| t.done).count();
        let total = todos.len();
        let count = format!("{done}/{total}");
        let add_submit = Rc::clone(&add);
        let add_click = Rc::clone(&add);
        let focus_write = focused_w.clone();
        let field_style = if focused { FIELD_FOCUS } else { FIELD_IDLE };

        let header = Aqua::Box()
            .styles([HEADER])
            .child(Aqua::Text("Tasks").styles([HERO]).build())
            .child(
                Aqua::Box()
                    .styles([PILL_BOX])
                    .child(Aqua::Text(count).styles([PILL_TX]).build())
                    .build(),
            )
            .build();

        let field = Aqua::Box()
            .styles([FIELD_BOX, field_style])
            .child(
                Aqua::Text("")
                    .bind((draft_r.clone(), draft_w.clone()))
                    .styles([FIELD_TEXT])
                    .placeholder("New task")
                    .on_focus(move |value| {
                        focus_write.set_if_changed(value);
                    })
                    .on_submit(move || add_submit())
                    .build(),
            )
            .build();
        let entry_row = Aqua::Box()
            .styles([ROW])
            .child(field)
            .child(action_button("Add", true, move || add_click()))
            .build();

        // Rows from a lazy iterator over the snapshot: keyed, with actions.
        // Arrays, Vecs, and lazy iterators feed `.children` in call order.
        let rows = todos.iter().map(|t| todo_row(t, &todos_w));
        let mut tasks = Aqua::Box().styles([TASKS]);
        if todos.is_empty() {
            tasks = tasks.child(
                Aqua::Box()
                    .styles([EMPTY_BOX])
                    .child(Aqua::Text("No tasks").styles([EMPTY_TITLE]).build())
                    .build(),
            );
        } else {
            tasks = tasks.children(rows);
        }

        let content = Aqua::Box()
            .styles([CONTENT])
            .child(header)
            .child(entry_row)
            .child(tasks.build())
            .build();

        let mut page = Aqua::Box().styles([PAGE]);
        if glass {
            page = page.style(GLASS_PAGE);
        }
        page.child(Aqua::Box().styles([TOP_STRIP]).build())
            .child(content)
            .build()
    };
    ((todos_r, focused_r), render)
}

/// Canonical setup: explicit dependencies via `Context::view`, glass chosen
/// from the resolved material (framework-resolved, no manual
/// capability/prefs polling in `main`).
fn todo_app(cx: &Context) -> impl IntoView {
    let glass = cx.material().is_glass();
    let (deps, render) = todo_view(cx.scope(), Vec::new(), glass);
    cx.view(deps, render)
}

/// Headless deterministic pump: mount → type → Add → toggle → remove,
/// every step through the real input path (hit-test → arm on press,
/// `Activate` on release inside bounds — the correct control contract).
/// Prints scene/run counts at each step; asserts the full row lifecycle.
fn headless_demo() -> Result<(), String> {
    use aqua_core::InputEvent;
    use aqua_ui::tree::NodeKind;

    let aqua = Aqua::new(AquaConfig::default())?;
    let mut app = UiApp::new(aqua.fonts().clone(), aqua.runs().clone());
    let (deps, render) = todo_view(app.scope(), Vec::new(), false);
    app.mount_reactive(deps, render)
        .map_err(|e| format!("mount failed: {e}"))?;
    app.set_viewport(760.0, 560.0, 1.0);

    let build = |app: &mut UiApp| {
        app.sync().map_err(|e| e.to_string())?;
        app.build_frame().map(|_| ()).map_err(|e| e.to_string())
    };
    /// Real press: primary down arms the activatable ancestor, primary up
    /// inside its bounds fires `Activate` exactly once.
    fn press(app: &mut UiApp, at: aqua_core::Point) {
        use aqua_core::{InputEvent, PointerButton, PointerEvent, PointerKind};
        let event = |pressed: bool| {
            let pe = PointerEvent {
                contact: 0,
                kind: PointerKind::Mouse,
                position: at,
                buttons: if pressed { 1 } else { 0 },
            };
            if pressed {
                InputEvent::PointerDown(pe, PointerButton::Primary)
            } else {
                InputEvent::PointerUp(pe, PointerButton::Primary)
            }
        };
        assert!(app.handle_input(&event(true)), "press down consumed");
        // Release dispatches through the armed-press path (may not
        // "consume" per the router's bool, but must fire Activate).
        app.handle_input(&event(false));
    }
    fn center_of(app: &UiApp, node: aqua_core::NodeId) -> aqua_core::Point {
        let r = app.tree().rect(node).expect("node rect");
        aqua_core::Point::new(r.x + r.w * 0.5, r.y + r.h * 0.5)
    }
    /// Label text node by content; its parent box is the lowered control.
    fn labeled(app: &UiApp, label: &str) -> aqua_core::NodeId {
        use aqua_ui::tree::NodeKind;
        app.tree()
            .traverse()
            .into_iter()
            .filter_map(|n| {
                let is_label = matches!(
                    app.tree().get(n).map(|r| &r.kind),
                    Some(NodeKind::Text(t)) if t.text() == label
                );
                is_label
                    .then(|| app.tree().get(n).and_then(|r| r.parent))
                    .flatten()
            })
            .next()
            .unwrap_or_else(|| panic!("control labeled {label:?}"))
    }
    fn row_titles(app: &UiApp) -> Vec<String> {
        use aqua_ui::tree::NodeKind;
        app.tree()
            .traverse()
            .into_iter()
            .filter_map(|n| match app.tree().get(n).map(|r| &r.kind) {
                Some(NodeKind::Text(t)) if !t.is_editable() && !t.text().is_empty() => {
                    Some(t.text().to_string())
                }
                _ => None,
            })
            .collect()
    }

    build(&mut app)?;
    println!(
        "mounted: scene ops={} text_runs={} build_error={:?}",
        app.scene().ordered().count(),
        app.text_run_count(),
        app.build_error()
    );

    // Type into the bound field through the real input path.
    let field = app
        .tree()
        .traverse()
        .into_iter()
        .find(|n| matches!(app.tree().get(*n).map(|r| &r.kind), Some(NodeKind::Text(t)) if t.is_editable()))
        .expect("bound input field");
    app.tree_mut().set_focused(Some(field));
    assert!(app.handle_input(&InputEvent::TextInput {
        text: "Buy milk".to_string()
    }));
    build(&mut app)?;
    println!("typed: text_runs={}", app.text_run_count());

    // Add through the real pointer path (down arms, up fires).
    let at = center_of(&app, labeled(&app, "Add"));
    press(&mut app, at);
    build(&mut app)?;
    let rows = row_titles(&app);
    println!(
        "after add: rows={rows:?} scene_ops={}",
        app.scene().ordered().count()
    );
    assert!(rows.iter().any(|t| t == "Buy milk"), "added row renders");

    // Toggle the row through its checkbox (down+up on the control).
    let at = center_of(&app, labeled(&app, "Buy milk"));
    press(&mut app, at);
    build(&mut app)?;
    let header = row_titles(&app);
    println!("after toggle: rows={header:?}");
    assert!(
        header.iter().any(|t| t.contains("1/1")),
        "toggle marks one done: {header:?}"
    );

    // Remove through the row's Remove button (down+up).
    let at = center_of(&app, labeled(&app, "Remove"));
    press(&mut app, at);
    build(&mut app)?;
    let rows = row_titles(&app);
    println!(
        "after remove: rows={rows:?} scene_ops={} text_runs={} build_error={:?}",
        app.scene().ordered().count(),
        app.text_run_count(),
        app.build_error()
    );
    assert!(!rows.iter().any(|t| t == "Buy milk"), "removed row is gone");
    assert_eq!(app.build_error(), None);
    Ok(())
}

/// Export the actual UI through the production renderer for visual review.
fn preview(path: &str) -> Result<(), String> {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut app = UiApp::new(fonts.clone(), runs.clone());
    let (deps, render) = todo_view(
        app.scope(),
        vec![
            Todo {
                id: 10,
                title: "Give the little things some attention".into(),
                done: false,
            },
            Todo {
                id: 11,
                title: "Make time for a walk".into(),
                done: false,
            },
            Todo {
                id: 12,
                title: "Start with one small thing".into(),
                done: true,
            },
        ],
        false,
    );
    app.mount_reactive(deps, render)
        .map_err(|e| e.to_string())?;
    let input = app.tree().traverse().into_iter().find(|id| matches!(&app.tree().get(*id).unwrap().kind, aqua_ui::tree::NodeKind::Text(t) if t.is_editable())).unwrap();
    app.tree_mut().set_focused(Some(input));
    app.set_viewport(760.0, 560.0, 1.0);
    app.build_frame().map_err(|e| e.to_string())?;
    let mut renderer =
        pollster::block_on(aqua_render::Renderer::headless(760, 560)).map_err(|e| e.to_string())?;
    renderer.set_text_provider(Box::new(aqua_render::GlyphonProvider::new(
        renderer.device(),
        renderer.queue(),
        renderer.format(),
        aqua_render::TextBackend::new(fonts, runs),
    )));
    let plan = renderer.prepare(app.scene_mut());
    renderer.draw(&plan).map_err(|e| e.to_string())?;
    let mut pixels = renderer.readback().map_err(|e| e.to_string())?;
    // The offscreen working target is linear. PNG viewers expect sRGB.
    for rgba in pixels.as_chunks_mut::<4>().0 {
        for channel in &mut rgba[..3] {
            let x = f32::from(*channel) / 255.0;
            let srgb = if x <= 0.0031308 {
                x * 12.92
            } else {
                1.055 * x.powf(1.0 / 2.4) - 0.055
            };
            *channel = (srgb.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    image::RgbaImage::from_raw(760, 560, pixels)
        .unwrap()
        .save(path)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn main() -> Result<(), aqua::Error> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(index) = args.iter().position(|arg| arg == "--preview") {
        let path = args
            .get(index + 1)
            .ok_or_else(|| aqua::Error::Init("--preview requires a PNG path".to_string()))?;
        return preview(path).map_err(aqua::Error::Init);
    }
    // Windowed by default (standard `cargo run --example todo`);
    // `--headless` runs the deterministic CI pump instead (low-level
    // UiApp pump; the canonical windowed path below uses `Aqua::App`).
    if std::env::args().any(|a| a == "--headless") {
        return headless_demo().map_err(aqua::Error::Init);
    }
    // Canonical factories: framework resolves material/prefs at startup;
    // `todo_app` picks glass styling from `cx.material().is_glass()`.
    // Overlay titlebar: full client area with native caption buttons
    // top-right; the empty TOP_STRIP below is the drag zone (default 40
    // logical px titlebar height — no explicit height needed).
    Aqua::App()
        .config(
            Aqua::Config().window(
                Aqua::Window()
                    .title("Aqua — Tasks")
                    .size(px(760.0), px(560.0))
                    .min_size(px(480.0), px(360.0))
                    .theme(Theme::Dark)
                    .material(Material::Glass)
                    .titlebar(Titlebar::Overlay),
            ),
        )
        .run(todo_app)
}

// Keep the `auto` helper referenced so the canonical length vocabulary
// (`px`/`pct`/`auto`) stays visibly complete at the facade; content-sized
// lengths are the default and need no explicit use in this showcase.
#[allow(dead_code)]
fn _length_vocabulary() -> aqua_ui::style::Length {
    auto()
}
