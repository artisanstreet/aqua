use aqua::ui::{Box as UiBox, Text as UiText};
use aqua::UiApp;
use aqua_core::{InputEvent, NodeId};
use aqua_platform::geometry::LogicalSize;
use aqua_platform::{AquaWindowId, ObservedWindowState};
use aqua_render::{Renderer, Scene};
use aqua_text::{SharedFonts, SharedRuns};

fn fonts_and_runs() -> (SharedFonts, SharedRuns) {
    (SharedFonts::new(), SharedRuns::new())
}

fn observed(w: f64, h: f64) -> ObservedWindowState {
    ObservedWindowState {
        id: AquaWindowId::new(1).unwrap(),
        title: "ui-app-test".to_string(),
        inner_size: LogicalSize::new(w, h),
        scale_factor: 1.0,
        focused: true,
        visible: true,
        occluded: false,
    }
}

fn first_text_node(app: &UiApp) -> NodeId {
    // BFS from the root for the first text node (robust to synthetic roots).
    let root = app.tree().root().expect("mounted root");
    let mut queue = vec![root];
    while let Some(id) = queue.pop() {
        let rec = app.tree().get(id).unwrap();
        if matches!(rec.kind, aqua_ui::NodeKind::Text(_)) {
            return id;
        }
        queue.extend(rec.children.iter().copied());
    }
    panic!("no text node mounted");
}

// -- mount ---------------------------------------------------------------------

#[test]
fn mount_renders_static_text_to_scene() {
    let (fonts, runs) = fonts_and_runs();
    let mut app = UiApp::new(fonts, runs);
    app.mount(|_scope| {
        UiBox::builder()
            .child(UiText::builder().arg("hello").build())
            .child(UiText::builder().arg("world").build())
            .build()
    })
    .unwrap();
    let mut scene = Scene::new();
    let deadline = app
        .build_scene_into(&mut scene, &observed(800.0, 600.0))
        .expect("build");
    assert_eq!(deadline, None, "static content requests no deadline");
    assert_eq!(app.build_error(), None);
    assert!(!scene.is_empty(), "static text emits scene ops");
    assert!(app.frame_stats().scene_ops > 0);
}

// -- two-way editing ------------------------------------------------------------

#[test]
fn two_way_edit_binding_roundtrip() {
    let (fonts, runs) = fonts_and_runs();
    let mut app = UiApp::new(fonts, runs);
    let (read, write) = app.scope().signal("hi".to_string());
    let writer = write.clone();
    app.mount(|_scope| {
        UiBox::builder()
            .child(UiText::builder().bind((read, write)).build())
            .build()
    })
    .unwrap();
    let mut scene = Scene::new();
    app.build_scene_into(&mut scene, &observed(800.0, 600.0))
        .unwrap();

    // Host → app: focus the editable, route a keystroke through UiApp.
    let node = first_text_node(&app);
    app.tree_mut().set_focusable(node, true);
    app.tree_mut().set_focused(Some(node));
    assert!(app.handle_input(&InputEvent::TextInput {
        text: " there".to_string()
    }));
    // TextChanged routes back through `write` (try_set, infallible).
    app.sync().unwrap();
    let mut scene2 = Scene::new();
    app.build_scene_into(&mut scene2, &observed(800.0, 600.0))
        .unwrap();
    assert_eq!(app.build_error(), None);

    // App → host: programmatic write flows through the binding to the tree.
    writer.set("programmatic".to_string());
    app.sync().unwrap();
    let mut scene3 = Scene::new();
    app.build_scene_into(&mut scene3, &observed(800.0, 600.0))
        .unwrap();
    let text = match &app.tree().get(node).unwrap().kind {
        aqua_ui::NodeKind::Text(t) => t.text().to_string(),
        _ => panic!("expected text"),
    };
    assert_eq!(text, "programmatic");
}

// -- keyed structural refresh ----------------------------------------------------

#[test]
fn keyed_structural_refresh_preserves_focus_and_edit() {
    let (fonts, runs) = fonts_and_runs();
    let mut app = UiApp::new(fonts, runs);
    app.mount(|_scope| {
        UiBox::builder()
            .child(UiText::builder().arg("row-a").build().with_key("a"))
            .build()
    })
    .unwrap();
    let mut scene = Scene::new();
    app.build_scene_into(&mut scene, &observed(800.0, 600.0))
        .unwrap();
    let node = first_text_node(&app);
    app.tree_mut().set_focusable(node, true);
    app.tree_mut().set_focused(Some(node));
    // Structural refresh: same key (identity kept), new row added.
    app.refresh(vec![UiBox::builder()
        .child(UiText::builder().arg("row-b").build().with_key("b"))
        .child(UiText::builder().arg("row-a!").build().with_key("a"))
        .build()])
        .unwrap();
    app.sync().unwrap();
    let mut scene2 = Scene::new();
    app.build_scene_into(&mut scene2, &observed(800.0, 600.0))
        .unwrap();
    assert_eq!(app.build_error(), None);
    assert_eq!(app.tree().focused(), Some(node), "focus follows the key");
}

// -- disposal ---------------------------------------------------------------------

#[test]
fn dispose_runs_cleanups_and_releases_runs() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let (fonts, runs) = fonts_and_runs();
    let mut app = UiApp::new(fonts, runs);
    let cleaned = Rc::new(RefCell::new(false));
    let cleaned2 = cleaned.clone();
    app.mount(|scope| {
        scope.on_cleanup(move || *cleaned2.borrow_mut() = true);
        UiBox::builder()
            .child(UiText::builder().arg("bye").build())
            .build()
    })
    .unwrap();
    let mut scene = Scene::new();
    app.build_scene_into(&mut scene, &observed(800.0, 600.0))
        .unwrap();
    app.dispose();
    assert!(*cleaned.borrow(), "view cleanup ran exactly once");
    // Second dispose is a no-op, never double-frees.
    app.dispose();
}

// -- strict real-pixels artifact ----------------------------------------------------

fn headless_or_skip(w: u32, h: u32) -> Option<Renderer> {
    match pollster::block_on(Renderer::headless(w, h)) {
        Ok(r) => Some(r),
        Err(e) => {
            let strict = std::env::var("AQUA_REQUIRE_GPU").is_ok_and(|v| v == "1");
            if matches!(e, aqua_render::RenderError::AdapterNotFound(_)) && !strict {
                eprintln!("SKIP-NO-ADAPTER ui-app artifact (no adapter): {e}");
                return None;
            }
            panic!("ui-app artifact GPU init failed (must not skip): {e:?}");
        }
    }
}

#[test]
fn ui_app_todo_artifact_real_text() {
    use aqua::ui::Element;
    let Some(mut r) = headless_or_skip(480, 320) else {
        return;
    };
    let (fonts, runs) = fonts_and_runs();
    // The renderer must shape with the SAME font system + run registry the
    // UiApp measures with — otherwise pixels and layout disagree.
    let backend = aqua_render::TextBackend::new(fonts.clone(), runs.clone());
    let fmt = r.format();
    r.set_text_provider(Box::new(aqua_render::GlyphonProvider::new(
        r.device(),
        r.queue(),
        fmt,
        backend,
    )));

    let mut app = UiApp::new(fonts, runs);
    let (read, write) = app.scope().signal("buy oat milk".to_string());
    let rows: Vec<(u64, String)> =
        vec![(1, "buy milk".to_string()), (2, "write report".to_string())];
    let el: Element = UiBox::builder()
        .layout(aqua_ui::style::Axis::Column)
        .gap(8)
        .child(UiText::new("Todos").build())
        .children(
            rows.iter()
                .map(|todo| UiText::new(todo.1.as_str()).build().with_key(todo.0)),
        )
        .child(
            UiText::builder()
                .bind((read, write))
                .placeholder("New todo...")
                .build(),
        )
        .child(
            aqua::ui::Button::new()
                .on_activate(|| {})
                .child(UiText::new("Add").build())
                .build(),
        )
        .build();
    app.mount(|_scope| el).unwrap();
    let mut scene = Scene::new();
    app.build_scene_into(&mut scene, &observed(480.0, 320.0))
        .unwrap();
    assert_eq!(app.build_error(), None);
    assert!(!scene.is_empty());
    let plan = r.prepare(&mut scene);
    assert!(!plan.is_clean(), "real UI content produces work");
    r.draw(&plan).expect("draw");
    let px = r.readback().expect("readback");
    assert_eq!(px.len(), 480 * 320 * 4);
    // Glyph pixels vs transparent background: white antialiased text has
    // high channels with significant alpha; background is (0,0,0,0).
    let glyph_px = px
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|c| c[3] > 100 && c[0] > 150 && c[1] > 150 && c[2] > 150)
        .count();
    assert!(
        glyph_px > 500,
        "real shaped text must paint hundreds of glyph pixels, got {glyph_px}"
    );
    if let Ok(path) = std::env::var("AQUA_PNG_OUT") {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(parent).expect("artifact dir");
        }
        let img = image::RgbaImage::from_raw(480, 320, px).expect("raw buffer");
        img.save(&path).expect("png save");
        eprintln!("artifact saved to {path}");
    }
}

#[test]
fn padded_single_line_field_keeps_caret_visible_at_multiple_scales() {
    use aqua_core::{DisplayItem, Key, KeyEvent, Modifiers, NamedKey};
    use std::{cell::Cell, rc::Rc};
    for scale in [1.0, 1.5, 2.0] {
        let (fonts, runs) = fonts_and_runs();
        let mut app = UiApp::new(fonts, runs);
        let submitted = Rc::new(Cell::new(0));
        let focus = Rc::new(Cell::new(false));
        let notify = submitted.clone();
        let focused = focus.clone();
        app.mount(|scope| {
            let (read, write) = scope.signal(String::new());
            UiBox::builder()
                .width(240)
                .height(120)
                .child(
                    UiText::builder()
                        .bind((read, write))
                        .width(180)
                        .height(48)
                        .padding(12)
                        .font_size(16)
                        .placeholder("New task")
                        .on_focus(move |value| focused.set(value))
                        .on_submit(move || notify.set(notify.get() + 1))
                        .build(),
                )
                .build()
        })
        .unwrap();
        let field = first_text_node(&app);
        app.tree_mut().set_focused(Some(field));
        assert!(focus.get());
        let mut scene = Scene::new();
        let mut window = observed(240.0, 120.0);
        window.scale_factor = scale;
        app.build_scene_into(&mut scene, &window).unwrap();
        let first = app.ime_anchor().unwrap();
        assert!(
            (first.x - 12.0).abs() < 0.1,
            "caret honors left padding at {scale}"
        );
        assert!((first.y - 12.0).abs() < 0.1);
        assert!(
            (first.h - 19.2).abs() < 0.5,
            "caret height is logical, not scaled twice: {first:?}"
        );
        app.handle_input(&InputEvent::TextInput {
            text: "This long task must scroll horizontally while I keep typing".into(),
        });
        app.build_scene_into(&mut scene, &window).unwrap();
        let anchor = app.ime_anchor().unwrap();
        assert!(
            anchor.x >= 12.0 && anchor.x + anchor.w <= 168.5,
            "caret remains inside content: {anchor:?}"
        );
        let text_x = scene
            .ordered()
            .find_map(|(_, item)| match item {
                DisplayItem::Text { at, .. } => Some(at[0]),
                _ => None,
            })
            .unwrap();
        assert!(text_x < 12.0, "long input scrolls its painted text");
        let key = |key, repeat| {
            InputEvent::KeyDown(KeyEvent {
                key: Key::Named(key),
                modifiers: Modifiers::NONE,
                repeat,
            })
        };
        app.handle_input(&key(NamedKey::Home, false));
        app.build_scene_into(&mut scene, &window).unwrap();
        assert!(
            (app.ime_anchor().unwrap().x - 12.0).abs() < 0.1,
            "Home scrolls back to the beginning"
        );
        app.handle_input(&key(NamedKey::Right, false));
        let caret = match &app.tree().get(field).unwrap().kind {
            aqua_ui::NodeKind::Text(t) => t.caret(),
            _ => None,
        };
        assert_eq!(
            caret,
            Some(1),
            "one arrow key moves once through the overlay router"
        );
        assert!(app.handle_input(&key(NamedKey::Enter, false)));
        app.handle_input(&key(NamedKey::Enter, true));
        assert_eq!(
            submitted.get(),
            1,
            "Enter submits once; key repeat does not duplicate tasks"
        );
        app.handle_input(&InputEvent::Ime(aqua_core::ImeEvent::Update {
            marked: "文".into(),
            cursor: Some((3, 3)),
        }));
        app.handle_input(&key(NamedKey::Enter, false));
        assert_eq!(
            submitted.get(),
            1,
            "Enter belongs to the IME while composing"
        );
    }
}
