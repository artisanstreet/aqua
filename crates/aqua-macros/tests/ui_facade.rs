//! Consumer tests against the REAL facade (`::aqua`) and `aqua-ui` element
//! builders.
//!
//! Dormant: the whole file is gated on `--features aqua-integration` AND
//! requires dev-dependencies on the real crates (add to
//! `crates/aqua-macros/Cargo.toml` when the tree is green):
//! `aqua = { path = "../aqua" }`, `aqua-ui = { path = "../aqua-ui" }`,
//! `aqua-reactive = { path = "../aqua-reactive" }`,
//! `aqua-core = { path = "../aqua-core" }`. Run:
//! `cargo test --features aqua-integration`.
//!
//! Method names below follow the frozen builder contract in
//! `docs/UI_MACRO.md`. If names change in `aqua_ui::element`, update the
//! call sites here — the `ui!` invocations themselves must NOT change (that
//! is the point: the macro is stable, builders adapt).

#![cfg(feature = "aqua-integration")]

use aqua_macros::ui;

// Facade alias under test: `aqua::ui` must re-export `aqua_ui::element::ui`
// (i.e. `pub use aqua_ui::element::ui;` in the facade). These imports fail
// until the facade publishes the alias.
use ::aqua::ui::{Box as UiBox, Element, Text as UiText};

#[test]
fn facade_todo_smoke() {
    let el: Element = ui! {
        Box(layout: "column", gap: 16) {
            Text("Todo App")
        }
    };
    let _ = el;
    let _ = (UiBox::builder(), UiText::builder());
}

#[test]
fn facade_keyed_list() {
    let todos = [("a", 1u64), ("b", 2u64)];
    let el: Element = ui! {
        Box {
            for (label, id) in todos.iter().copied() {
                key(id) {
                    Text(label)
                }
            }
        }
    };
    let _ = el;
}

/// End-to-end: `ui!` expands against the REAL `aqua_ui::element::ui`
/// builders and mounts into a real `UiTree` with live reactive updates.
/// `crate = ::aqua_ui::element` exercises the configurable crate path;
/// switching to the facade only changes the preamble to `crate = ::aqua`.
#[test]
fn real_mount_counter() {
    use aqua_reactive::Runtime;
    use aqua_ui::binding::View;
    use aqua_ui::layout::NaiveMeasurer;
    use aqua_ui::tree::{NodeKind, UiTree};

    let rt = Runtime::new();
    let mut tree = UiTree::new();
    let sink = tree.command_sink();
    let mut view = View::new(rt.create_scope(), sink);
    let column = "column";
    let (count, set_count) = {
        let s = view.scope();
        s.signal(0i32)
    };
    let label = {
        let s = view.scope();
        s.derive1(&count, |c| format!("Count: {c}"))
    };
    let el: aqua_ui::element::Element = ui! {
        crate = ::aqua_ui::element;
        Box(layout: column, gap: 16) {
            Text(value: label)
            Button(on_activate: || {}) {
                Text("Increment")
            }
        }
    };
    let ids = view.set_content(&mut tree, vec![el]).unwrap();
    assert_eq!(ids.len(), 1);
    tree.compute_layout(aqua_core::Size::new(400.0, 400.0), &NaiveMeasurer)
        .unwrap();
    let text_id = tree.get(ids[0]).unwrap().children[0];
    let text_of = |tree: &UiTree, id: aqua_core::NodeId| match &tree.get(id).unwrap().kind {
        NodeKind::Text(t) => t.text().to_string(),
        other => panic!("expected text, got {other:?}"),
    };
    assert_eq!(text_of(&tree, text_id), "Count: 0");
    set_count.set(2);
    tree.compute_layout(aqua_core::Size::new(400.0, 400.0), &NaiveMeasurer)
        .unwrap();
    assert_eq!(text_of(&tree, text_id), "Count: 2");
}
