//! Independent black-box conformance for Aqua collections (`List`/`Table`).
//!
//! Ownership: this file is owned by the collections-conformance worker. It
//! exercises ONLY the public `Aqua` factory APIs (`Aqua::List`, `Aqua::Table`,
//! `Aqua::Column`, `fr`, `px`) through the real `UiApp` frame pipeline
//! (`mount` / `refresh` / `sync` / `build_scene_into` / `handle_input`).
//! No production code or peer tests are touched here.
//!
//! API basis: `aqua_ui::collection` (builders + sources, single source) as
//! re-exported by the `aqua` facade. Header factories are closures
//! (`Fn() -> Element`); tracks are `px(..)` / `pct(..)` / `fr(..)`; exact
//! `CollectionError` variants are NOT matched (the seam dedup is still owned
//! by the API worker) — invalid inputs are asserted to *surface loudly*
//! (mount/layout/frame `Err` or `build_error`), never to vanish or panic.
//!
//! Status: runtime realization (`View::compute_layout` collection support)
//! is still mid-edit, so virtualized-behavior tests are expected to fail
//! until the parent freeze. Findings are recorded honestly in
//! `docs/COLLECTION_VALIDATION.md`.

use std::cell::Cell;
use std::rc::Rc;

use aqua::{fr, px, Aqua, Style, UiApp};
use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey, Oklch, Point, Role, ScrollDelta};
use aqua_platform::geometry::LogicalSize;
use aqua_platform::{AquaWindowId, ObservedWindowState};
use aqua_render::Scene;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::tree::NodeKind;

#[derive(Clone)]
struct Item {
    key: u64,
    label: String,
    sig: usize,
}

impl Item {
    fn plain(key: u64) -> Self {
        Self {
            key,
            label: format!("row-{key}"),
            sig: 0,
        }
    }
}

#[derive(Clone)]
struct NameValue {
    key: u64,
    name: String,
    value: u64,
}

// ---------------------------------------------------------------------------
// Harness helpers (real UiApp frame pipeline, headless).
// ---------------------------------------------------------------------------

fn observed(w: f64, h: f64) -> ObservedWindowState {
    ObservedWindowState {
        id: AquaWindowId::new(1).expect("window id"),
        title: "collections-conformance".to_string(),
        inner_size: LogicalSize::new(w, h),
        scale_factor: 1.0,
        focused: true,
        visible: true,
        occluded: false,
    }
}

fn host() -> UiApp {
    UiApp::new(SharedFonts::new(), SharedRuns::new())
}

/// Flush effects and build one frame; valid content must stay error-free.
fn frame(app: &mut UiApp, scene: &mut Scene, w: f64, h: f64) {
    app.sync().expect("sync drains");
    app.build_scene_into(scene, &observed(w, h))
        .expect("frame builds");
    assert_eq!(app.build_error(), None, "frame must stay error-free");
}

/// Static (non-editable, non-empty) text contents currently mounted.
fn static_texts(app: &UiApp) -> Vec<String> {
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

fn row_texts(app: &UiApp, prefix: &str) -> Vec<String> {
    static_texts(app)
        .into_iter()
        .filter(|t| t.starts_with(prefix))
        .collect()
}

fn text_node(app: &UiApp, content: &str) -> Option<aqua_core::NodeId> {
    app.tree().traverse().into_iter().find(|n| {
        matches!(app.tree().get(*n).map(|r| &r.kind),
            Some(NodeKind::Text(t)) if t.text() == content)
    })
}

fn editable_node(app: &UiApp, content: &str) -> Option<aqua_core::NodeId> {
    app.tree().traverse().into_iter().find(|n| {
        matches!(app.tree().get(*n).map(|r| &r.kind),
            Some(NodeKind::Text(t)) if t.is_editable() && t.text() == content)
    })
}

fn node_text(app: &UiApp, id: aqua_core::NodeId) -> Option<String> {
    match app.tree().get(id).map(|r| &r.kind) {
        Some(NodeKind::Text(t)) => Some(t.text().to_string()),
        _ => None,
    }
}

/// All static text under a node. Collection row/cell wrappers carry the role
/// while their descendant text nodes carry the labels, so logical indices
/// must be derived from descendant text, never the wrapper label.
fn descendant_texts(app: &UiApp, id: aqua_core::NodeId) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![id];
    while let Some(n) = stack.pop() {
        let Some(rec) = app.tree().get(n) else {
            continue;
        };
        if let NodeKind::Text(t) = &rec.kind {
            if !t.is_editable() && !t.text().is_empty() {
                out.push(t.text().to_string());
            }
        }
        stack.extend(rec.children.iter().copied());
    }
    out
}

/// First scroll viewport in traversal order (the collection viewport when
/// virtualized; collections lower virtualized content into a `Scroll` node).
fn find_scroll(app: &UiApp) -> Option<aqua_core::NodeId> {
    app.tree().traverse().into_iter().find(|n| {
        matches!(
            app.tree().get(*n).map(|r| &r.kind),
            Some(NodeKind::Scroll(_))
        )
    })
}

/// Visual (screen-space) rect: committed layout rect minus every ancestor
/// `Scroll` offset. `UiTree::rect` intentionally returns layout-space coords
/// (scene emission and hit-testing apply the scroll translation), so any
/// assertion about what the user sees — or any synthetic press position —
/// must go through this helper, never raw `rect()`.
fn visual_rect(app: &UiApp, id: aqua_core::NodeId) -> Option<aqua_core::Rect> {
    let mut rect = app.tree().rect(id)?;
    let mut next = app.tree().get(id)?.parent;
    while let Some(pid) = next {
        let rec = app.tree().get(pid)?;
        if let NodeKind::Scroll(s) = &rec.kind {
            rect = rect.translate(-s.offset.x, -s.offset.y);
        }
        next = rec.parent;
    }
    Some(rect)
}

/// Header visibility regression (stretched/invisible-header artifact):
/// committed visual header height > 0, header rect and first body rect do
/// not overlap (header bottom at/above body top), header inside the root.
/// Label presence alone does not prove any of this.
fn assert_header_visible(app: &UiApp, header_label: &str, first_body_label: &str) {
    let h = visual_rect(app, text_node(app, header_label).expect("header mounted"))
        .expect("header rect");
    assert!(h.h > 0.0, "committed visual header height > 0, got {h:?}");
    let b = visual_rect(
        app,
        text_node(app, first_body_label).expect("first body row mounted"),
    )
    .expect("body rect");
    assert!(
        h.intersect(b).is_empty(),
        "header/body rects must not overlap: header {h:?} vs body {b:?}"
    );
    assert!(
        h.y + h.h <= b.y + 1.0,
        "header bottom at/above first body top: header {h:?} vs body {b:?}"
    );
    let root = app
        .tree()
        .root()
        .and_then(|r| app.tree().rect(r))
        .expect("root mounted");
    assert!(
        h.y >= root.y - 1.0 && h.y + h.h <= root.y + root.h + 1.0 && h.x >= root.x - 1.0,
        "header within root: header {h:?} vs root {root:?}"
    );
}

fn row_index_of(label: &str) -> Option<usize> {
    label.strip_prefix("row-")?.parse().ok()
}

fn list_element(
    items: Vec<Item>,
    calls: Rc<Cell<usize>>,
    virtualized: bool,
) -> aqua_ui::element::Element {
    let c = calls.clone();
    let row = move |it: &Item| {
        c.set(c.get() + 1);
        Aqua::Text(it.label.clone()).build()
    };
    if virtualized {
        Aqua::List()
            .items(items)
            .key(|it: &Item| it.key)
            .row(row)
            .virtualize()
            .build()
    } else {
        Aqua::List()
            .items(items)
            .key(|it: &Item| it.key)
            .row(row)
            .build()
    }
}

// ---------------------------------------------------------------------------
// 1. Unvirtualized List mounts every item through the row factories.
// ---------------------------------------------------------------------------

#[test]
fn unvirtualized_list_mounts_every_row() {
    let calls = Rc::new(Cell::new(0));
    let items: Vec<Item> = (0..32u64).map(Item::plain).collect();
    let mut app = host();
    app.mount(|_| list_element(items, calls.clone(), false))
        .unwrap();
    app.set_viewport(600.0, 800.0, 1.0);
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 600.0, 800.0);

    let rows = row_texts(&app, "row-");
    assert_eq!(rows.len(), 32, "OFF mounts every item: {rows:?}");
    assert_eq!(
        calls.get(),
        32,
        "row factory runs once per mounted row, never in bulk at build"
    );
    assert!(!scene.is_empty());
}

// ---------------------------------------------------------------------------
// 2. Unvirtualized Table mounts real headers + all body cells.
// ---------------------------------------------------------------------------

#[test]
fn unvirtualized_table_mounts_real_headers_and_all_cells() {
    let cell_calls = Rc::new(Cell::new(0));
    let header_calls = Rc::new(Cell::new(0));
    let items: Vec<NameValue> = (0..16u64)
        .map(|i| NameValue {
            key: i,
            name: format!("name-{i}"),
            value: 100 - i,
        })
        .collect();
    let cc = cell_calls.clone();
    let hc = header_calls.clone();
    let mut app = host();
    app.mount(|_| {
        Aqua::Table()
            .items(items.iter().cloned())
            .key(|it: &NameValue| it.key)
            .columns([
                Aqua::Column("name")
                    .header(move || {
                        hc.set(hc.get() + 1);
                        Aqua::Text("Name").build()
                    })
                    .cell(move |it: &NameValue| {
                        cc.set(cc.get() + 1);
                        Aqua::Text(it.name.clone()).build()
                    })
                    .width(fr(2.0))
                    .build(),
                Aqua::Column("value")
                    .header(|| Aqua::Text("Value").build())
                    .cell(|it: &NameValue| Aqua::Text(format!("{}", it.value)).build())
                    .width(px(90.0))
                    .build(),
            ])
            .styles([Style::new().width(px(700.0)).height(px(600.0))])
            .header_styles([Style::new().gap(4.0)])
            .row_styles([Style::new().gap(4.0)])
            .build()
    })
    .unwrap();
    app.set_viewport(700.0, 600.0, 1.0);
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 700.0, 600.0);

    let labels = static_texts(&app);
    assert!(labels.iter().any(|t| t == "Name"), "{labels:?}");
    assert!(labels.iter().any(|t| t == "Value"), "{labels:?}");
    assert_eq!(
        row_texts(&app, "name-").len(),
        16,
        "every body row mounted: {labels:?}"
    );
    assert_eq!(cell_calls.get(), 16, "cell factory once per mounted row");
    assert!(
        header_calls.get() >= 1,
        "header factory actually ran (real headers, not fake controls)"
    );

    // Body rows stack vertically at normal line height: strictly increasing
    // y with sane steps. Catches the AxisRow-style horizontal arrangement
    // (all y equal) and giant stretched rows (absurd steps) that factory
    // counts + x alignment cannot see.
    let ys: Vec<f32> = (0..16u64)
        .map(|i| {
            text_node(&app, &format!("name-{i}"))
                .and_then(|id| visual_rect(&app, id))
                .expect("body cell mounted")
                .y
        })
        .collect();
    assert!(
        ys.windows(2).all(|w| w[1] > w[0]),
        "body rows stack vertically, got {ys:?}"
    );
    for w in ys.windows(2) {
        let d = w[1] - w[0];
        assert!(
            (1.0..=120.0).contains(&d),
            "normal line-height step between rows, got {d} in {ys:?}"
        );
    }
    assert!(
        ys.iter().all(|y| (0.0..600.0).contains(y)),
        "all 16 body rows visible in the viewport: {ys:?}"
    );
    assert_header_visible(&app, "Name", "name-0");
}

// ---------------------------------------------------------------------------
// 3. Virtualized List over 100k rows bounds factory calls + mounted nodes.
// ---------------------------------------------------------------------------

#[test]
fn virtualized_list_100k_bounds_factory_calls_and_mounted_nodes() {
    let calls = Rc::new(Cell::new(0));
    let items: Vec<Item> = (0..100_000u64).map(Item::plain).collect();
    let mut app = host();
    app.mount(|_| list_element(items, calls.clone(), true))
        .unwrap();
    app.set_viewport(600.0, 600.0, 1.0);
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 600.0, 600.0);
    frame(&mut app, &mut scene, 600.0, 600.0);

    let rows = row_texts(&app, "row-");
    assert!(
        rows.len() < 400,
        "virtual ON mounts visible+overscan only, got {} mounted",
        rows.len()
    );
    assert!(
        calls.get() < 400,
        "factories run per mounted row only, got {} calls for 100k rows",
        calls.get()
    );
    assert!(
        app.tree().len() < 3000,
        "retained tree stays bounded, got {} nodes",
        app.tree().len()
    );
    assert!(rows.iter().any(|t| t == "row-0"), "window starts at top");
}

// ---------------------------------------------------------------------------
// 4. Virtualized Table over 20k rows bounds cell factories, keeps headers.
// ---------------------------------------------------------------------------

#[test]
fn virtualized_table_bounds_cell_factories_and_keeps_headers() {
    let cell_calls = Rc::new(Cell::new(0));
    let items: Vec<NameValue> = (0..20_000u64)
        .map(|i| NameValue {
            key: i,
            name: format!("name-{i}"),
            value: i,
        })
        .collect();
    let cc = cell_calls.clone();
    let mut app = host();
    app.mount(|_| {
        Aqua::Table()
            .items(items.iter().cloned())
            .key(|it: &NameValue| it.key)
            .columns([
                Aqua::Column("name")
                    .header(|| Aqua::Text("Name").build())
                    .cell(move |it: &NameValue| {
                        cc.set(cc.get() + 1);
                        Aqua::Text(it.name.clone()).build()
                    })
                    .width(fr(1.0))
                    .build(),
                Aqua::Column("value")
                    .header(|| Aqua::Text("Value").build())
                    .cell(|it: &NameValue| Aqua::Text(format!("{}", it.value)).build())
                    .width(px(90.0))
                    .build(),
            ])
            .styles([Style::new().width(px(700.0)).height(px(600.0))])
            .virtualize()
            .build()
    })
    .unwrap();
    app.set_viewport(700.0, 600.0, 1.0);
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 700.0, 600.0);
    frame(&mut app, &mut scene, 700.0, 600.0);

    let labels = static_texts(&app);
    assert!(labels.iter().any(|t| t == "Name"), "{labels:?}");
    assert!(labels.iter().any(|t| t == "Value"), "{labels:?}");
    assert!(
        cell_calls.get() < 600,
        "cell factories bounded to the window, got {} calls for 20k rows",
        cell_calls.get()
    );
    assert!(
        row_texts(&app, "name-").len() < 300,
        "mounted body rows bounded"
    );
}

// ---------------------------------------------------------------------------
// 5. Real scroll_to + InputEvent::Scroll move the window; resize changes it.
// ---------------------------------------------------------------------------

#[test]
fn scroll_and_resize_move_the_virtual_window() {
    let calls = Rc::new(Cell::new(0));
    let items: Vec<Item> = (0..100_000u64).map(Item::plain).collect();
    let mut app = host();
    app.mount(|_| list_element(items, calls.clone(), true))
        .unwrap();
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 600.0, 600.0);
    frame(&mut app, &mut scene, 600.0, 600.0);
    let scroll = find_scroll(&app).expect("virtualized list lowers to a Scroll viewport");

    // Real scroll_to near the end: the window must follow through the real
    // frame pipeline (no shadow state).
    assert!(app
        .tree_mut()
        .scroll_to(scroll, Point::new(0.0, 10_000_000.0)));
    frame(&mut app, &mut scene, 600.0, 600.0);
    frame(&mut app, &mut scene, 600.0, 600.0);
    let bottom: Vec<usize> = row_texts(&app, "row-")
        .iter()
        .filter_map(|t| row_index_of(t))
        .collect();
    assert!(!bottom.is_empty(), "rows mounted after scroll");
    assert!(
        *bottom.iter().max().unwrap() > 99_000,
        "scrolled to the tail, got max {:?}",
        bottom.iter().max()
    );

    // InputEvent::Scroll back to the top through UiApp dispatch.
    let at = Point::new(300.0, 300.0);
    assert!(app.handle_input(&InputEvent::Scroll {
        at,
        delta: ScrollDelta {
            x: 0.0,
            y: -10_000_000.0
        },
    }));
    frame(&mut app, &mut scene, 600.0, 600.0);
    frame(&mut app, &mut scene, 600.0, 600.0);
    let top: Vec<usize> = row_texts(&app, "row-")
        .iter()
        .filter_map(|t| row_index_of(t))
        .collect();
    assert!(
        *top.iter().min().unwrap() < 50,
        "scroll event jumped back to the head, got min {:?}",
        top.iter().min()
    );

    // Resize changes the visible range (smaller viewport => fewer rows).
    frame(&mut app, &mut scene, 600.0, 200.0);
    let small = row_texts(&app, "row-").len();
    frame(&mut app, &mut scene, 600.0, 1000.0);
    frame(&mut app, &mut scene, 600.0, 1000.0);
    let large = row_texts(&app, "row-").len();
    assert!(
        small < large,
        "resize changes the mounted range: 200px mounted {small}, 1000px mounted {large}"
    );
}

// ---------------------------------------------------------------------------
// 6. Variable row heights + keyed prepends preserve the reading anchor.
// ---------------------------------------------------------------------------

#[test]
fn variable_heights_and_prepend_preserve_reading_anchor() {
    let calls = Rc::new(Cell::new(0));
    let items: Vec<Item> = (0..2000u64).map(Item::plain).collect();
    let c = calls.clone();
    let mut app = host();
    app.mount(|_| {
        let cc = c.clone();
        Aqua::List()
            .items(items.iter().cloned())
            .key(|it: &Item| it.key)
            .row(move |it: &Item| {
                cc.set(cc.get() + 1);
                let h = 24.0 + ((it.key % 5) as f32) * 8.0;
                Aqua::Box()
                    .height(h)
                    .child(Aqua::Text(it.label.clone()).build())
                    .build()
            })
            .virtualize()
            .build()
    })
    .unwrap();
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 600.0, 400.0);
    let scroll = find_scroll(&app).expect("virtualized list lowers to a Scroll viewport");
    assert!(app.tree_mut().scroll_to(scroll, Point::new(0.0, 30_000.0)));
    frame(&mut app, &mut scene, 600.0, 400.0);
    frame(&mut app, &mut scene, 600.0, 400.0);
    snap_state(&app, scroll, "pre-prepend");
    /// Isolation snapshot for the driver: committed scroll offset, viewport rect,
    /// and content extent (distinguishes "scroll reset/clamped" from "anchor not
    /// restored" from "heights at estimate"). Printed on every run; captured with
    /// `--nocapture`.
    fn snap_state(app: &UiApp, scroll: aqua_core::NodeId, what: &str) {
        eprintln!(
            "anchor-test {what}: offset={:?} rect={:?} content={:?} nodes={}",
            app.tree().scroll_offset(scroll),
            app.tree().rect(scroll),
            app.tree().content_size(scroll),
            app.tree().len(),
        );
    }

    // Anchor = the middle visible row (content + position under the eye).
    // Visual y: the anchor contract is about screen position, and layout y
    // shifts with the scroll after prepends — compare what the user sees.
    let mut visible: Vec<(f32, String)> = row_texts(&app, "row-")
        .into_iter()
        .filter_map(|t| {
            let id = text_node(&app, &t)?;
            let r = visual_rect(&app, id)?;
            Some((r.y, t))
        })
        .collect();
    visible.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert!(visible.len() > 4, "window has depth to anchor");
    let (y0, anchor) = visible[visible.len() / 2].clone();

    // Prepend 20 rows with fresh keys; the anchor row must stay readable at
    // ~the same viewport position (stable key + within-row offset).
    let mut prepended: Vec<Item> = (0..20u64)
        .map(|j| Item {
            key: 2_000_000 + j,
            label: format!("row-{}", 2_000_000 + j),
            sig: 0,
        })
        .collect();
    let mut rest: Vec<Item> = (0..2000u64).map(Item::plain).collect();
    prepended.append(&mut rest);
    let c2 = calls.clone();
    app.refresh(vec![{
        let cc = c2.clone();
        Aqua::List()
            .items(prepended)
            .key(|it: &Item| it.key)
            .row(move |it: &Item| {
                cc.set(cc.get() + 1);
                let h = 24.0 + ((it.key % 5) as f32) * 8.0;
                Aqua::Box()
                    .height(h)
                    .child(Aqua::Text(it.label.clone()).build())
                    .build()
            })
            .virtualize()
            .build()
    }])
    .unwrap();
    frame(&mut app, &mut scene, 600.0, 400.0);
    frame(&mut app, &mut scene, 600.0, 400.0);
    snap_state(&app, scroll, "post-prepend");

    let id = text_node(&app, &anchor).expect("anchor row still mounted after prepend");
    let y1 = visual_rect(&app, id).expect("anchor rect").y;
    assert!(
        (y1 - y0).abs() <= 48.0,
        "reading anchor preserved: {anchor} was y={y0}, now y={y1}"
    );
}

// ---------------------------------------------------------------------------
// 7. Keyed reorder preserves identity + edit focus.
// ---------------------------------------------------------------------------

#[test]
fn keyed_reorder_preserves_identity_and_edit_focus() {
    const N: u64 = 24;
    let mut app = host();
    type SigPair = (
        aqua_reactive::ReadSignal<String>,
        aqua_reactive::WriteSignal<String>,
    );
    let mut sigs: Vec<SigPair> = Vec::new();
    for i in 0..N {
        sigs.push(app.scope().signal(format!("item-{i}")));
    }
    let sigs = Rc::new(sigs);
    let items: Vec<Item> = (0..N)
        .map(|i| Item {
            key: i,
            label: format!("item-{i}"),
            sig: i as usize,
        })
        .collect();
    let build = |order: &[Item], sigs: Rc<Vec<SigPair>>| {
        Aqua::List()
            .items(order.iter().cloned())
            .key(|it: &Item| it.key)
            .row(move |it: &Item| {
                let (r, w) = &sigs[it.sig];
                Aqua::Text("").bind((r.clone(), w.clone())).build()
            })
            .build()
    };
    app.mount(|_| build(&items, sigs.clone())).unwrap();
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 600.0, 800.0);

    // Focus the editable row for item-5, move the caret to the end through
    // the real key path, then edit through the real input path.
    let target = editable_node(&app, "item-5").expect("editable row mounted");
    assert!(app.tree_mut().set_focusable(target, true));
    assert!(app.tree_mut().set_focused(Some(target)));
    app.handle_input(&InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::End),
        modifiers: Modifiers::NONE,
        repeat: false,
    }));
    assert!(app.handle_input(&InputEvent::TextInput {
        text: "!".to_string()
    }));
    frame(&mut app, &mut scene, 600.0, 800.0);
    assert_eq!(node_text(&app, target).as_deref(), Some("item-5!"));

    // Reorder (reverse): identity and edit focus must follow the key.
    let mut reversed = items.clone();
    reversed.reverse();
    app.refresh(vec![build(&reversed, sigs.clone())]).unwrap();
    frame(&mut app, &mut scene, 600.0, 800.0);
    assert_eq!(
        app.tree().focused(),
        Some(target),
        "focus follows the key through reorder"
    );
    assert_eq!(
        node_text(&app, target).as_deref(),
        Some("item-5!"),
        "edit content survives reorder on the same node"
    );
}

// ---------------------------------------------------------------------------
// 8. A focused far row is pinned alone — never the entire gap.
// ---------------------------------------------------------------------------

#[test]
fn focused_far_row_pinned_alone_not_entire_gap() {
    let calls = Rc::new(Cell::new(0));
    let items: Vec<Item> = (0..20_000u64).map(Item::plain).collect();
    let mut app = host();
    app.mount(|_| list_element(items, calls.clone(), true))
        .unwrap();
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 600.0, 600.0);
    frame(&mut app, &mut scene, 600.0, 600.0);
    let scroll = find_scroll(&app).expect("virtualized list lowers to a Scroll viewport");

    assert!(app.tree_mut().scroll_to(scroll, Point::new(0.0, 200_000.0)));
    frame(&mut app, &mut scene, 600.0, 600.0);
    frame(&mut app, &mut scene, 600.0, 600.0);
    let mut visible: Vec<(f32, String)> = row_texts(&app, "row-")
        .into_iter()
        .filter_map(|t| {
            let id = text_node(&app, &t)?;
            Some((app.tree().rect(id)?.y, t))
        })
        .collect();
    visible.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let pinned_label = visible[visible.len() / 2].1.clone();
    let pinned = text_node(&app, &pinned_label).expect("row mounted");
    assert!(app.tree_mut().set_focusable(pinned, true));
    assert!(app.tree_mut().set_focused(Some(pinned)));

    // Scroll far away: the focused row stays mounted in isolation while the
    // gap between window and pin is NOT mounted.
    assert!(app.tree_mut().scroll_to(scroll, Point::new(0.0, 600_000.0)));
    frame(&mut app, &mut scene, 600.0, 600.0);
    frame(&mut app, &mut scene, 600.0, 600.0);
    assert!(
        app.tree().get(pinned).is_some(),
        "focused row {pinned_label} stays mounted (pinned)"
    );
    assert_eq!(app.tree().focused(), Some(pinned), "focus retained");
    let mounted = row_texts(&app, "row-").len();
    assert!(
        mounted < 400,
        "only window + isolated pin mounted, not the gap: {mounted} rows"
    );
}

// ---------------------------------------------------------------------------
// 9. Scroll churn stays bounded (no scope/node/run leaks per frame).
// ---------------------------------------------------------------------------

#[test]
fn scroll_churn_stays_bounded() {
    let calls = Rc::new(Cell::new(0));
    let items: Vec<Item> = (0..100_000u64).map(Item::plain).collect();
    let mut app = host();
    app.mount(|_| list_element(items, calls.clone(), true))
        .unwrap();
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 600.0, 600.0);
    frame(&mut app, &mut scene, 600.0, 600.0);
    let scroll = find_scroll(&app).expect("virtualized list lowers to a Scroll viewport");

    let mut max_nodes = 0usize;
    let mut max_runs = 0usize;
    for y in [50_000.0, 200_000.0, 1_000_000.0, 3_000_000.0, 0.0] {
        let before = calls.get();
        assert!(app.tree_mut().scroll_to(scroll, Point::new(0.0, y)));
        frame(&mut app, &mut scene, 600.0, 600.0);
        frame(&mut app, &mut scene, 600.0, 600.0);
        let delta = calls.get() - before;
        assert!(
            delta < 500,
            "one scroll realizes O(window), not O(n): {delta} factory calls"
        );
        max_nodes = max_nodes.max(app.tree().len());
        max_runs = max_runs.max(app.text_run_count());
    }
    assert!(
        max_nodes < 3000,
        "retained nodes bounded across churn: {max_nodes}"
    );
    assert!(
        max_runs < 600,
        "text runs GC'd with unmounted rows: {max_runs}"
    );
    assert!(
        row_texts(&app, "row-").iter().any(|t| t == "row-0"),
        "scroll back to top restores the head rows"
    );
}

// ---------------------------------------------------------------------------
// 10. Table: one shared width plan, horizontal scroll, real style merges.
// ---------------------------------------------------------------------------

#[test]
fn table_shares_one_width_plan_and_styles_merge() {
    const CELL: Oklch = Oklch {
        l: 0.72,
        c: 0.14,
        h: 250.0,
        alpha: 1.0,
    };
    const HEADER: Oklch = Oklch {
        l: 0.35,
        c: 0.05,
        h: 250.0,
        alpha: 1.0,
    };
    // Later styles win per field; unset fields never clobber (normal merges).
    let composed = Style::compose(&[
        Style::new().width(px(100.0)).gap(4.0),
        Style::new().gap(8.0),
    ]);
    assert_eq!(
        composed.resolve_layout().width,
        aqua_ui::style::Length::Px(100.0)
    );
    assert_eq!(composed.resolve_layout().gap, 8.0);

    let items: Vec<NameValue> = (0..200u64)
        .map(|i| NameValue {
            key: i,
            name: format!("name-{i}"),
            value: i,
        })
        .collect();
    let mut app = host();
    app.mount(|_| {
        Aqua::Table()
            .items(items.iter().cloned())
            .key(|it: &NameValue| it.key)
            .columns([
                Aqua::Column("name")
                    .header(|| Aqua::Text("Name").build())
                    .cell(|it: &NameValue| {
                        Aqua::Box()
                            .child(Aqua::Text(it.name.clone()).build())
                            .build()
                    })
                    .width(px(400.0))
                    .build(),
                Aqua::Column("value")
                    .header(|| Aqua::Text("Value").build())
                    .cell(|it: &NameValue| Aqua::Text(format!("{}", it.value)).build())
                    .width(px(400.0))
                    .build(),
            ])
            .header_styles([Style::new().background(HEADER).gap(6.0)])
            .row_styles([Style::new().gap(6.0)])
            .cell_styles([Style::new().background(CELL)])
            .styles([Style::new().width(px(700.0)).height(px(600.0))])
            .virtualize()
            .build()
    })
    .unwrap();
    // Viewport narrower than the 800px plan forces horizontal scroll.
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 700.0, 600.0);
    frame(&mut app, &mut scene, 700.0, 600.0);

    // Header and body share one width plan: same column x positions.
    // Visual x: layout rects ignore scroll by design; joint scroll must be
    // asserted in screen space (this is also the coordinate space hit-
    // testing and scene emission use).
    let x_of = |app: &UiApp, label: &str| {
        let id = text_node(app, label).expect("cell mounted");
        visual_rect(app, id).expect("cell rect").x
    };
    let name_hx = x_of(&app, "Name");
    let name_bx = x_of(&app, "name-0");
    let value_hx = x_of(&app, "Value");
    assert!(
        (name_hx - name_bx).abs() <= 2.0,
        "header/body share the width plan: Name@{name_hx} vs body@{name_bx}"
    );
    assert_header_visible(&app, "Name", "name-0");

    // cell_styles are normal merges on the shared outer cell box: the painted
    // background must appear in the real scene (arbitrary inner elements stay
    // styleable — body text above proves it).
    let painted = scene
        .ordered()
        .filter_map(|(_, item)| match item {
            aqua_core::DisplayItem::Rect { fill, .. } if *fill == CELL => Some(()),
            _ => None,
        })
        .count();
    assert!(
        painted > 0,
        "cell_styles background paints real scene rects"
    );

    // Horizontal scroll on the body viewport must move header and body
    // together by the same delta (shared plan + shared offset). The body
    // scroll is the tallest scroll viewport (header clips are short).
    let body = app
        .tree()
        .traverse()
        .into_iter()
        .filter(|n| {
            matches!(
                app.tree().get(*n).map(|r| &r.kind),
                Some(NodeKind::Scroll(_))
            )
        })
        .max_by(|a, b| {
            let ha = app.tree().content_size(*a).map(|s| s.h).unwrap_or(0.0);
            let hb = app.tree().content_size(*b).map(|s| s.h).unwrap_or(0.0);
            ha.partial_cmp(&hb).unwrap()
        })
        .expect("body scroll viewport");
    assert!(app.tree_mut().scroll_to(body, Point::new(80.0, 0.0)));
    frame(&mut app, &mut scene, 700.0, 600.0);
    frame(&mut app, &mut scene, 700.0, 600.0);
    let name_hx2 = x_of(&app, "Name");
    let name_bx2 = x_of(&app, "name-0");
    let value_hx2 = x_of(&app, "Value");
    // Single assertion so one failure reports all three deltas (isolates
    // dead-h-scroll from header-sync-only lag for the driver).
    let deltas = [
        (name_hx, name_hx2, "Name header"),
        (name_bx, name_bx2, "name body"),
        (value_hx, value_hx2, "Value header"),
    ];
    assert!(
        deltas
            .iter()
            .all(|(before, after, _)| (before - after - 80.0).abs() <= 3.0),
        "header+body scroll together by 80: {deltas:?}"
    );
    assert!(
        (name_hx2 - name_bx2).abs() <= 2.0,
        "shared plan holds while scrolled"
    );
}

// ---------------------------------------------------------------------------
// 11. Invalid keys / columns / tracks / heights surface loudly, never silent.
// ---------------------------------------------------------------------------

/// Either mount or the first frame must surface the problem (or record it on
/// `build_error`); clean mounts with full content are the failure mode.
fn expect_surfaced(mount_result: Result<(), aqua::UiAppError>, app: &mut UiApp, what: &str) {
    if mount_result.is_err() {
        return;
    }
    let mut scene = Scene::new();
    // sync and frame return different error types; evaluate both so a sync
    // failure cannot mask a missing frame failure (or vice versa).
    let sync_err = app.sync().is_err();
    let frame_err = app
        .build_scene_into(&mut scene, &observed(600.0, 600.0))
        .is_err();
    assert!(
        sync_err || frame_err || app.build_error().is_some(),
        "{what} must fail at mount/layout, never mount silently"
    );
}

#[test]
fn duplicate_row_keys_surface() {
    let items = vec![
        Item::plain(1),
        Item {
            key: 1,
            label: "row-dup".to_string(),
            sig: 0,
        },
    ];
    let mut app = host();
    let res = app.mount(|_| list_element(items, Rc::new(Cell::new(0)), false));
    expect_surfaced(res, &mut app, "duplicate row keys");
}

#[test]
fn duplicate_column_ids_surface() {
    let items: Vec<NameValue> = (0..4u64)
        .map(|i| NameValue {
            key: i,
            name: format!("name-{i}"),
            value: i,
        })
        .collect();
    let mut app = host();
    let res = app.mount(|_| {
        Aqua::Table()
            .items(items.iter().cloned())
            .key(|it: &NameValue| it.key)
            .columns([
                Aqua::Column("same")
                    .header(|| Aqua::Text("A").build())
                    .cell(|it: &NameValue| Aqua::Text(it.name.clone()).build())
                    .width(px(100.0))
                    .build(),
                Aqua::Column("same")
                    .header(|| Aqua::Text("B").build())
                    .cell(|it: &NameValue| Aqua::Text(format!("{}", it.value)).build())
                    .width(px(100.0))
                    .build(),
            ])
            .build()
    });
    expect_surfaced(res, &mut app, "duplicate column ids");
}

#[test]
fn invalid_tracks_and_heights_surface() {
    // Negative fr share.
    let mut app = host();
    let res = app.mount(|_| {
        Aqua::Table()
            .items([NameValue {
                key: 0,
                name: "name-0".to_string(),
                value: 0,
            }])
            .key(|it: &NameValue| it.key)
            .columns([Aqua::Column("c")
                .header(|| Aqua::Text("C").build())
                .cell(|it: &NameValue| Aqua::Text(it.name.clone()).build())
                .width(fr(-1.0))
                .build()])
            .build()
    });
    expect_surfaced(res, &mut app, "negative fr track");

    // Non-positive estimated row height.
    let mut app = host();
    let res = app.mount(|_| {
        Aqua::List()
            .items([Item::plain(0)])
            .key(|it: &Item| it.key)
            .row(|it: &Item| Aqua::Text(it.label.clone()).build())
            .estimated_row_height(px(-5.0))
            .build()
    });
    expect_surfaced(res, &mut app, "non-positive estimated row height");

    // min_width > max_width.
    let mut app = host();
    let res = app.mount(|_| {
        Aqua::Table()
            .items([NameValue {
                key: 0,
                name: "name-0".to_string(),
                value: 0,
            }])
            .key(|it: &NameValue| it.key)
            .columns([Aqua::Column("c")
                .header(|| Aqua::Text("C").build())
                .cell(|it: &NameValue| Aqua::Text(it.name.clone()).build())
                .width(px(100.0))
                .min_width(200)
                .max_width(100)
                .build()])
            .build()
    });
    expect_surfaced(res, &mut app, "min_width > max_width");
}

#[test]
fn missing_factories_surface() {
    // No .row(..).
    let mut app = host();
    let res = app.mount(|_| {
        Aqua::List()
            .items([Item::plain(0)])
            .key(|it: &Item| it.key)
            .build()
    });
    expect_surfaced(res, &mut app, "missing row factory");

    // No .key(..).
    let mut app = host();
    let res = app.mount(|_| {
        Aqua::List()
            .items([Item::plain(0)])
            .row(|it: &Item| Aqua::Text(it.label.clone()).build())
            .build()
    });
    expect_surfaced(res, &mut app, "missing key factory");

    // Table with no columns.
    let mut app = host();
    let res = app.mount(|_| {
        Aqua::Table()
            .items([NameValue {
                key: 0,
                name: "name-0".to_string(),
                value: 0,
            }])
            .key(|it: &NameValue| it.key)
            .build()
    });
    expect_surfaced(res, &mut app, "table with no columns");
}

// ---------------------------------------------------------------------------
// 12. Mounted rows/cells carry correct roles; totals + logical indices flow.
// ---------------------------------------------------------------------------

#[test]
fn semantics_carry_roles_totals_and_logical_indices() {
    // List: ListItem roles, one-based positions, set_size totals.
    let mut app = host();
    app.mount(|_| {
        list_element(
            (0..500u64).map(Item::plain).collect(),
            Rc::new(Cell::new(0)),
            true,
        )
    })
    .unwrap();
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 600.0, 400.0);
    frame(&mut app, &mut scene, 600.0, 400.0);

    let sem = app.tree().emit_semantics();
    let list_rows: Vec<_> = sem.iter().filter(|n| n.role == Role::ListItem).collect();
    assert!(
        !list_rows.is_empty(),
        "mounted list rows carry ListItem roles"
    );
    for n in &list_rows {
        let cs = n
            .collection
            .as_ref()
            .expect("row carries collection coordinates");
        assert_eq!(cs.set_size, Some(500), "total rows reported on every row");
        // ListItem wrappers carry the role, not the text: derive the logical
        // index from descendant text.
        let idx: usize = descendant_texts(&app, n.node)
            .iter()
            .find_map(|l| l.strip_prefix("row-")?.parse().ok())
            .expect("row descendant text parses");
        assert_eq!(
            cs.position_in_set,
            Some(idx + 1),
            "list positions are one-based logical indices"
        );
    }
    assert!(
        sem.iter()
            .any(|n| n.collection.is_some_and(|cs| cs.set_size == Some(500))),
        "total count visible through core metadata"
    );

    // Table: Row / ColumnHeader / Cell roles, zero-based indices, counts.
    let items: Vec<NameValue> = (0..200u64)
        .map(|i| NameValue {
            key: i,
            name: format!("name-{i}"),
            value: i,
        })
        .collect();
    let mut app = host();
    app.mount(|_| {
        Aqua::Table()
            .items(items.iter().cloned())
            .key(|it: &NameValue| it.key)
            .columns([
                Aqua::Column("name")
                    .header(|| Aqua::Text("Name").build())
                    .cell(|it: &NameValue| Aqua::Text(it.name.clone()).build())
                    .width(fr(1.0))
                    .build(),
                Aqua::Column("value")
                    .header(|| Aqua::Text("Value").build())
                    .cell(|it: &NameValue| Aqua::Text(format!("{}", it.value)).build())
                    .width(px(90.0))
                    .build(),
            ])
            .styles([Style::new().width(px(700.0)).height(px(600.0))])
            .virtualize()
            .build()
    })
    .unwrap();
    frame(&mut app, &mut scene, 700.0, 600.0);
    frame(&mut app, &mut scene, 700.0, 600.0);
    let sem = app.tree().emit_semantics();
    let body_rows: Vec<_> = sem.iter().filter(|n| n.role == Role::Row).collect();
    assert!(!body_rows.is_empty(), "mounted table rows carry Row roles");
    let headers: Vec<_> = sem
        .iter()
        .filter(|n| n.role == Role::ColumnHeader)
        .collect();
    assert_eq!(headers.len(), 2, "one ColumnHeader per column");
    let cells: Vec<_> = sem.iter().filter(|n| n.role == Role::Cell).collect();
    assert!(!cells.is_empty(), "body cells carry Cell roles");
    for n in &cells {
        let cs = n.collection.as_ref().expect("cell carries coordinates");
        assert!(
            cs.column_index == Some(0) || cs.column_index == Some(1),
            "zero-based column index, got {cs:?}"
        );
    }
    assert!(
        sem.iter().any(|n| n
            .collection
            .is_some_and(|cs| cs.row_count == Some(200) && cs.column_count == Some(2))),
        "table totals (rows + columns) visible through core metadata"
    );
}

// ---------------------------------------------------------------------------
// 13. Strict offscreen GPU pixels for List + Table (only AdapterNotFound skips).
// ---------------------------------------------------------------------------

fn headless_or_skip(w: u32, h: u32) -> Option<aqua_render::Renderer> {
    match pollster::block_on(aqua_render::Renderer::headless(w, h)) {
        Ok(r) => Some(r),
        Err(e) => {
            let strict = std::env::var("AQUA_REQUIRE_GPU").is_ok_and(|v| v == "1");
            if matches!(e, aqua_render::RenderError::AdapterNotFound(_)) && !strict {
                eprintln!("SKIP-NO-ADAPTER collections artifact (no adapter): {e}");
                return None;
            }
            panic!("collections artifact GPU init failed (must not skip): {e:?}");
        }
    }
}

#[test]
fn collections_paint_real_pixels_offscreen() {
    use aqua::ui::Element;
    let Some(mut r) = headless_or_skip(640, 480) else {
        return;
    };
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let backend = aqua_render::TextBackend::new(fonts.clone(), runs.clone());
    let fmt = r.format();
    r.set_text_provider(Box::new(aqua_render::GlyphonProvider::new(
        r.device(),
        r.queue(),
        fmt,
        backend,
    )));

    let items: Vec<Item> = (0..12u64).map(Item::plain).collect();
    let rows: Vec<NameValue> = (0..8u64)
        .map(|i| NameValue {
            key: i,
            name: format!("name-{i}"),
            value: i,
        })
        .collect();
    let el: Element = Aqua::Box()
        .child(
            Aqua::List()
                .items(items)
                .key(|it: &Item| it.key)
                .row(|it: &Item| Aqua::Text(it.label.clone()).build())
                .build(),
        )
        .child(
            Aqua::Table()
                .items(rows.iter().cloned())
                .key(|it: &NameValue| it.key)
                .columns([
                    Aqua::Column("name")
                        .header(|| Aqua::Text("Name").build())
                        .cell(|it: &NameValue| Aqua::Text(it.name.clone()).build())
                        .width(fr(1.0))
                        .build(),
                    Aqua::Column("value")
                        .header(|| Aqua::Text("Value").build())
                        .cell(|it: &NameValue| Aqua::Text(format!("{}", it.value)).build())
                        .width(px(80.0))
                        .build(),
                ])
                .styles([Style::new().width(px(640.0)).height(px(300.0))])
                .build(),
        )
        .build();
    let mut app = UiApp::new(fonts, runs);
    app.mount(|_| el).unwrap();
    let mut scene = Scene::new();
    app.build_scene_into(&mut scene, &observed(640.0, 480.0))
        .unwrap();
    assert_eq!(app.build_error(), None);
    assert!(!scene.is_empty(), "list + table emit scene ops");

    let plan = r.prepare(&mut scene);
    assert!(!plan.is_clean(), "real collection content produces work");
    r.draw(&plan).expect("draw");
    let px = r.readback().expect("readback");
    assert_eq!(px.len(), 640 * 480 * 4);
    let glyph_px = px
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|c| c[3] > 100 && c[0] > 150 && c[1] > 150 && c[2] > 150)
        .count();
    assert!(
        glyph_px > 500,
        "list + table text must paint hundreds of glyph pixels, got {glyph_px}"
    );
    if let Ok(path) = std::env::var("AQUA_PNG_OUT") {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(parent).expect("artifact dir");
        }
        let img = image::RgbaImage::from_raw(640, 480, px).expect("raw buffer");
        img.save(&path).expect("png save");
        eprintln!("artifact saved to {path}");
    }
}

// ---------------------------------------------------------------------------
// 14. Live signal subscriptions survive window updates; eviction disposes.
// ---------------------------------------------------------------------------

/// Row items holding live `ReadSignal`s (writers stay app-scope owned).
/// Catches a specific likely lifecycle bug: if re-realization drops the
/// cached `Element` bindings without re-subscribing (or disposes the View
/// effect while the row is still resident), signal writes stop updating
/// mounted text; if eviction forgets disposal, dead rows keep effects alive
/// and writes enqueue `SetText` for dead nodes.
#[derive(Clone)]
struct SigItem {
    key: u64,
    read: aqua_reactive::ReadSignal<String>,
}

#[test]
fn live_signal_subscriptions_survive_window_updates_and_dispose_on_eviction() {
    const N: u64 = 500;
    let mut app = host();
    let baseline = app.runtime().stats();
    let mut writers = Vec::new();
    let mut items = Vec::new();
    for i in 0..N {
        let (read, write) = app.scope().signal(format!("sig-{i}"));
        writers.push(write);
        items.push(SigItem { key: i, read });
    }
    app.mount(|_| {
        Aqua::List()
            .items(items.iter().cloned())
            .key(|it: &SigItem| it.key)
            .row(|it: &SigItem| Aqua::Text(it.read.clone()).build())
            .virtualize()
            .build()
    })
    .unwrap();
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 600.0, 400.0);
    frame(&mut app, &mut scene, 600.0, 400.0);
    let stats0 = app.runtime().stats();
    assert!(
        stats0.effects > 0,
        "resident rows hold live subscription effects"
    );
    assert!(
        text_node(&app, "sig-0").is_some(),
        "first key mounted in the initial window"
    );

    // Small scroll retaining the first key: subscription must stay live, and
    // the structural window change must not reset the current label from a
    // cached Element initial snapshot.
    let scroll = find_scroll(&app).expect("virtualized list lowers to a Scroll viewport");
    assert!(app.tree_mut().scroll_to(scroll, Point::new(0.0, 32.0)));
    frame(&mut app, &mut scene, 600.0, 400.0);
    frame(&mut app, &mut scene, 600.0, 400.0);
    // Small scroll retaining the first key: subscription must stay live, and
    // the structural window change must not reset the current label from a
    // cached Element initial snapshot.
    let resident = text_node(&app, "sig-0").expect("small scroll retains the first key");
    assert_eq!(
        node_text(&app, resident).as_deref(),
        Some("sig-0"),
        "current label intact after scroll, before any write"
    );
    writers[0].set("sig-0-live".to_string());
    frame(&mut app, &mut scene, 600.0, 400.0);
    let live = text_node(&app, "sig-0-live").expect(
        "signal write updates mounted text: cached bindings must re-subscribe, not dispose",
    );
    assert_eq!(node_text(&app, live).as_deref(), Some("sig-0-live"));

    // Resize retaining the first key: still live, and the updated text must
    // remain on the same node before the next write (no reset to the cached
    // initial snapshot, no silent remount).
    frame(&mut app, &mut scene, 600.0, 300.0);
    frame(&mut app, &mut scene, 600.0, 300.0);
    assert_eq!(
        node_text(&app, resident).as_deref(),
        Some("sig-0-live"),
        "updated label persists on the resident node across resize, before next write"
    );
    writers[0].set("sig-0-live2".to_string());
    frame(&mut app, &mut scene, 600.0, 300.0);
    assert!(
        text_node(&app, "sig-0-live2").is_some(),
        "subscription survives resize"
    );

    // Same-viewport effect count: the per-window subscription budget.
    frame(&mut app, &mut scene, 600.0, 400.0);
    frame(&mut app, &mut scene, 600.0, 400.0);
    let stats_same = app.runtime().stats();

    // Scroll far: first row evicted, its scope/effect disposed.
    assert!(app
        .tree_mut()
        .scroll_to(scroll, Point::new(0.0, 10_000_000.0)));
    frame(&mut app, &mut scene, 600.0, 400.0);
    frame(&mut app, &mut scene, 600.0, 400.0);
    assert!(
        row_texts(&app, "sig-0").is_empty(),
        "first key evicted from the far window"
    );
    let stats_far = app.runtime().stats();
    assert!(
        stats_far.effects <= stats_same.effects,
        "evicted rows dispose effects ({} > {} resident budget)",
        stats_far.effects,
        stats_same.effects
    );
    assert!(
        stats_far.live_scopes <= stats_same.live_scopes,
        "eviction churn leaks no child scopes ({} > {} same-viewport count)",
        stats_far.live_scopes,
        stats_same.live_scopes
    );

    // Writing the evicted signal must not enqueue SetText for a dead node:
    // no pending effects, no remount, no error, no effect growth.
    writers[0].set("sig-0-ghost".to_string());
    app.sync().expect("sync drains");
    assert_eq!(
        app.runtime().stats().queued_effects,
        0,
        "no effect queued for the evicted row"
    );
    frame(&mut app, &mut scene, 600.0, 400.0);
    assert!(
        row_texts(&app, "sig-0").is_empty(),
        "evicted row stays unmounted after its signal fires"
    );
    assert_eq!(app.build_error(), None);
    assert!(
        app.runtime().stats().effects <= stats_same.effects,
        "ghost write creates no zombie effects"
    );

    // Dispose returns the graph to zero: no leaked scopes/effects/sources.
    // Why zero, not baseline: baseline.live_scopes is 1 — the root app scope
    // already exists before mount — and app.dispose() explicitly disposes it
    // together with the view scope. Effects/sources sit at baseline 0
    // throughout (no app-level signals outside the collected rows).
    app.dispose();
    let end = app.runtime().stats();
    assert_eq!(end.effects, baseline.effects, "effects return to baseline");
    assert_eq!(end.sources, baseline.sources, "sources return to baseline");
    assert_eq!(end.live_scopes, 0, "all scopes disposed, none leaked");
}

// ---------------------------------------------------------------------------
// 15. Nested rows inherit parent typography; header Button activates via a
// real press. Catches the inheritance-default-approximation bug (realized rows
// rebuilt with default instead of inherited typography) and proves header
// actions run application code through the real input path.
// ---------------------------------------------------------------------------

#[test]
fn nested_rows_inherit_typography_and_header_button_activates_on_press() {
    use aqua_core::{PointerButton, PointerEvent, PointerKind};

    const INK: Oklch = Oklch {
        l: 0.62,
        c: 0.12,
        h: 140.0,
        alpha: 1.0,
    };

    // Part A: typography inheritance through the collection boundary.
    let mut app = host();
    app.mount(|_| {
        Aqua::Box()
            .style(Style::new().text_color(INK).font_size(20.0))
            .child(
                Aqua::List()
                    .items((0..8u64).map(Item::plain).collect::<Vec<_>>())
                    .key(|it: &Item| it.key)
                    .row(|it: &Item| Aqua::Text(it.label.clone()).build())
                    .build(),
            )
            .build()
    })
    .unwrap();
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 600.0, 400.0);

    // Tree level: every row carries the inherited size and color.
    for i in 0..8u64 {
        let id = text_node(&app, &format!("row-{i}")).expect("row mounted");
        match app.tree().get(id).map(|r| &r.kind) {
            Some(NodeKind::Text(t)) => {
                assert_eq!(t.font_size, 20.0, "row-{i} inherits font size");
                assert_eq!(t.color, INK, "row-{i} inherits text color");
            }
            other => panic!("row-{i} is not text: {other:?}"),
        }
    }
    // Scene level: painted glyph color matches the inherited color.
    let colors: Vec<Oklch> = scene
        .ordered()
        .filter_map(|(_, item)| match item {
            aqua_core::DisplayItem::Text { color, .. } => Some(*color),
            _ => None,
        })
        .collect();
    assert!(!colors.is_empty(), "rows emit text scene ops");
    assert!(
        colors.iter().all(|c| *c == INK),
        "all row glyphs paint with the inherited color: {colors:?}"
    );

    // Part B: header Button runs application code through a real press.
    let mut app = host();
    let (read, write) = app.scope().signal(0u32);
    let w = write.clone();
    let r = read.clone();
    let items: Vec<NameValue> = (0..4u64)
        .map(|i| NameValue {
            key: i,
            name: format!("name-{i}"),
            value: i,
        })
        .collect();
    app.mount(|_| {
        Aqua::Table()
            .items(items.iter().cloned())
            .key(|it: &NameValue| it.key)
            .columns([
                Aqua::Column("name")
                    .header(move || {
                        let wc = w.clone();
                        let rc = r.clone();
                        Aqua::Button()
                            .on_activate(move || {
                                wc.set(rc.try_get().unwrap_or(0) + 1);
                            })
                            .child(Aqua::Text("Sort").build())
                            .build()
                    })
                    .cell(|it: &NameValue| Aqua::Text(it.name.clone()).build())
                    .width(fr(1.0))
                    .build(),
                Aqua::Column("value")
                    .header(|| Aqua::Text("Value").build())
                    .cell(|it: &NameValue| Aqua::Text(format!("{}", it.value)).build())
                    .width(px(90.0))
                    .build(),
            ])
            .styles([Style::new().width(px(700.0)).height(px(400.0))])
            .build()
    })
    .unwrap();
    frame(&mut app, &mut scene, 700.0, 400.0);

    // Press in screen space (hit-testing consumes visual coords). Guard the
    // press point itself: it must sit on the header, above the body and
    // inside the viewport — a passing activation at overlap/wrong coords
    // would otherwise prove nothing about header visibility.
    let label = text_node(&app, "Sort").expect("header button mounted");
    let rect = visual_rect(&app, label).expect("header button rect");
    let at = Point::new(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    let body0 = visual_rect(&app, text_node(&app, "name-0").expect("body row")).expect("body rect");
    assert!(
        at.y + 1.0 <= body0.y,
        "press lands on the header above the body: press {at:?} vs body {body0:?}"
    );
    assert!(
        at.x >= 0.0 && at.y >= 0.0 && at.y <= 400.0,
        "press inside the viewport: {at:?}"
    );
    let press = InputEvent::PointerDown(
        PointerEvent {
            contact: 7,
            kind: PointerKind::Mouse,
            position: at,
            buttons: 1,
        },
        PointerButton::Primary,
    );
    let release = InputEvent::PointerUp(
        PointerEvent {
            contact: 7,
            kind: PointerKind::Mouse,
            position: at,
            buttons: 0,
        },
        PointerButton::Primary,
    );
    assert!(app.handle_input(&press), "press hits the header button");
    assert!(app.handle_input(&release), "release dispatches activation");
    frame(&mut app, &mut scene, 700.0, 400.0);
    assert_eq!(
        read.try_get(),
        Some(1),
        "header Button ran application code through the real press path"
    );
}

// ---------------------------------------------------------------------------
// 16. Column plan is correct on the FIRST frame at actual width (!= 800px
// default) and stable clean frames reinvoke nothing. Catches the fresh-plan
// gate bug (first realization using the 800px seed plan) and rebuild churn.
// ---------------------------------------------------------------------------

#[test]
fn table_plan_correct_on_first_frame_then_stable_without_refactory() {
    let header_calls = Rc::new(Cell::new(0));
    let cell_calls = Rc::new(Cell::new(0));
    let items: Vec<NameValue> = (0..200u64)
        .map(|i| NameValue {
            key: i,
            name: format!("name-{i}"),
            value: i,
        })
        .collect();
    let hc = header_calls.clone();
    let cc = cell_calls.clone();
    let mut app = host();
    app.mount(|_| {
        Aqua::Table()
            .items(items.iter().cloned())
            .key(|it: &NameValue| it.key)
            .columns([
                Aqua::Column("name")
                    .header(move || {
                        hc.set(hc.get() + 1);
                        Aqua::Text("Name").build()
                    })
                    .cell(move |it: &NameValue| {
                        cc.set(cc.get() + 1);
                        Aqua::Text(it.name.clone()).build()
                    })
                    .width(fr(1.0))
                    .build(),
                Aqua::Column("value")
                    .header(|| Aqua::Text("Value").build())
                    .cell(|it: &NameValue| Aqua::Text(format!("{}", it.value)).build())
                    .width(px(90.0))
                    .build(),
            ])
            .styles([Style::new().width(px(700.0)).height(px(600.0))])
            .virtualize()
            .build()
    })
    .unwrap();

    // ONE frame at the actual 700px width: the plan must already be the
    // 700px plan (col2 x = 700 - 90 = 610), not the 800px seed (710).
    // Screen space: at scroll 0 visual == layout, helper keeps it honest.
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 700.0, 600.0);
    assert!(header_calls.get() >= 1, "header realized on frame one");
    assert!(cell_calls.get() > 0, "body rows realized on frame one");
    let value_hx = text_node(&app, "Value")
        .and_then(|id| visual_rect(&app, id))
        .expect("header mounted")
        .x;
    let value_bx = text_node(&app, "0")
        .and_then(|id| visual_rect(&app, id))
        .expect("first body row mounted")
        .x;
    for (what, x) in [("header", value_hx), ("body", value_bx)] {
        assert!(
            (x - 610.0).abs() <= 16.0,
            "{what} column x uses the actual 700px plan on frame one, got {x} (800px seed would give 710)"
        );
    }
    assert!(
        (value_hx - value_bx).abs() <= 1.5,
        "header/body share one plan on frame one: {value_hx} vs {value_bx}"
    );

    // Stable clean frames reinvoke nothing.
    let (h0, c0) = (header_calls.get(), cell_calls.get());
    frame(&mut app, &mut scene, 700.0, 600.0);
    frame(&mut app, &mut scene, 700.0, 600.0);
    assert_eq!(
        header_calls.get(),
        h0,
        "steady frames do not rebuild the header"
    );
    assert_eq!(
        cell_calls.get(),
        c0,
        "steady frames do not rebuild mounted rows"
    );
}

// ---------------------------------------------------------------------------
// 17. Virtualized table body stacks vertically; header stays fixed on scroll.
// Regression for the stretched-body visual artifact (body host defaulted to
// AxisRow): rows must stack at line height, and vertical body scroll must
// move rows while the header stays pinned.
// ---------------------------------------------------------------------------

#[test]
fn virtualized_table_body_stacks_and_header_stays_fixed_on_vscroll() {
    let items: Vec<NameValue> = (0..500u64)
        .map(|i| NameValue {
            key: i,
            name: format!("name-{i}"),
            value: i,
        })
        .collect();
    let mut app = host();
    app.mount(|_| {
        Aqua::Table()
            .items(items.iter().cloned())
            .key(|it: &NameValue| it.key)
            .columns([
                Aqua::Column("name")
                    .header(|| Aqua::Text("Name").build())
                    .cell(|it: &NameValue| Aqua::Text(it.name.clone()).build())
                    .width(fr(1.0))
                    .build(),
                Aqua::Column("value")
                    .header(|| Aqua::Text("Value").build())
                    .cell(|it: &NameValue| Aqua::Text(format!("{}", it.value)).build())
                    .width(px(90.0))
                    .build(),
            ])
            .styles([Style::new().width(px(700.0)).height(px(600.0))])
            .virtualize()
            .build()
    })
    .unwrap();
    let mut scene = Scene::new();
    frame(&mut app, &mut scene, 700.0, 600.0);
    frame(&mut app, &mut scene, 700.0, 600.0);

    // Stacking in the initial window: first 6 body rows strictly increasing
    // y with line-height steps (not side-by-side, not stretched).
    let ys: Vec<f32> = (0..6u64)
        .map(|i| {
            text_node(&app, &format!("name-{i}"))
                .and_then(|id| visual_rect(&app, id))
                .expect("body cell mounted")
                .y
        })
        .collect();
    assert!(
        ys.windows(2).all(|w| w[1] > w[0]),
        "virtualized body rows stack vertically, got {ys:?}"
    );
    for w in ys.windows(2) {
        assert!(
            (1.0..=120.0).contains(&(w[1] - w[0])),
            "line-height step, got {:?} in {ys:?}",
            w[1] - w[0]
        );
    }
    assert_header_visible(&app, "Name", "name-0");

    // The body scroll is the tallest scroll viewport (header clips are
    // short); vertical scroll must move rows, never the header.
    let body = app
        .tree()
        .traverse()
        .into_iter()
        .filter(|n| {
            matches!(
                app.tree().get(*n).map(|r| &r.kind),
                Some(NodeKind::Scroll(_))
            )
        })
        .max_by(|a, b| {
            let ha = app.tree().content_size(*a).map(|s| s.h).unwrap_or(0.0);
            let hb = app.tree().content_size(*b).map(|s| s.h).unwrap_or(0.0);
            ha.partial_cmp(&hb).unwrap()
        })
        .expect("body scroll viewport");
    let header_y0 = visual_rect(&app, text_node(&app, "Name").expect("header"))
        .expect("header rect")
        .y;
    assert!(app.tree_mut().scroll_to(body, Point::new(0.0, 300.0)));
    frame(&mut app, &mut scene, 700.0, 600.0);
    frame(&mut app, &mut scene, 700.0, 600.0);

    let header_y1 = visual_rect(&app, text_node(&app, "Name").expect("header"))
        .expect("header rect")
        .y;
    assert!(
        (header_y1 - header_y0).abs() <= 1.0,
        "header stays fixed while the body scrolls: {header_y0} -> {header_y1}"
    );
    let mut rows: Vec<(usize, f32)> = row_texts(&app, "name-")
        .into_iter()
        .filter_map(|t| {
            let idx: usize = t.strip_prefix("name-")?.parse().ok()?;
            let id = text_node(&app, &t)?;
            Some((idx, visual_rect(&app, id)?.y))
        })
        .collect();
    rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    assert!(
        rows.len() >= 4,
        "multiple body rows visible after scroll: {}",
        rows.len()
    );
    assert!(
        rows.windows(2).all(|w| w[1].1 > w[0].1),
        "scrolled body still stacks vertically: {rows:?}"
    );
    assert!(
        rows[0].0 > 0,
        "body content actually moved under the fixed header: top row is {:?}",
        rows[0]
    );
}
