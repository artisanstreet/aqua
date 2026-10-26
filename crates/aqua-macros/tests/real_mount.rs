//! Real consumer tests: `ui!` → real `aqua-ui` builders → real `UiTree`
//! mount with live reactive updates. Always active (no facade needed):
//! the `crate = crate::real;` preamble points macro output at this file's
//! `real::ui` module, which re-exports the actual `aqua_ui::element::ui`
//! builders. Switching to the facade only changes the preamble to
//! `crate = ::aqua;` (see `tests/ui_facade.rs`, gated on the alias).

use aqua_macros::ui;

/// Local facade path: mirrors what `aqua::ui` must publish
/// (`pub use aqua_ui::element::ui;`).
pub mod real {
    pub use aqua_ui::element::ui;
}

use aqua_core::{NodeId, Size};
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::layout::NaiveMeasurer;
use aqua_ui::tree::{NodeKind, UiTree};

fn layout(tree: &mut UiTree) {
    tree.compute_layout(Size::new(400.0, 400.0), &NaiveMeasurer)
        .unwrap();
}

fn text_of(tree: &UiTree, id: NodeId) -> String {
    match &tree.get(id).unwrap().kind {
        NodeKind::Text(t) => t.text().to_string(),
        other => panic!("expected text node, got {other:?}"),
    }
}

#[test]
fn real_mount_counter_updates() {
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
        crate = crate::real;
        Box(layout: column, gap: 16) {
            Text(value: label)
            Button(on_activate: || {}) {
                Text("Increment")
            }
        }
    };
    let ids = view.set_content(&mut tree, vec![el]).unwrap();
    assert_eq!(ids.len(), 1);
    layout(&mut tree);
    let text_id = tree.get(ids[0]).unwrap().children[0];
    assert_eq!(text_of(&tree, text_id), "Count: 0");
    set_count.set(2);
    layout(&mut tree);
    assert_eq!(text_of(&tree, text_id), "Count: 2");
}

#[test]
fn real_keyed_list_reorders_with_identity() {
    struct Todo {
        id: u64,
        label: &'static str,
    }
    let rt = Runtime::new();
    let mut tree = UiTree::new();
    let sink = tree.command_sink();
    let mut view = View::new(rt.create_scope(), sink);
    let column = "column";
    let todos = || {
        vec![
            Todo {
                id: 1,
                label: "one",
            },
            Todo {
                id: 2,
                label: "two",
            },
        ]
    };
    let render = |items: &[Todo]| {
        let mut root = aqua_ui::element::ui::Box::builder().layout(column);
        for t in items {
            root = root.child(
                aqua_ui::element::ui::Text::builder()
                    .arg(t.label)
                    .build()
                    .with_key(t.id),
            );
        }
        vec![root.build()]
    };
    let ids = view.set_content(&mut tree, render(&todos())).unwrap();
    layout(&mut tree);
    let before: Vec<NodeId> = tree.get(ids[0]).unwrap().children.clone();
    assert_eq!(before.len(), 2);
    let mut rev = todos();
    rev.reverse();
    let ids2 = view.set_content(&mut tree, render(&rev)).unwrap();
    layout(&mut tree);
    let after: Vec<NodeId> = tree.get(ids2[0]).unwrap().children.clone();
    assert_eq!(after, vec![before[1], before[0]]);
    assert_eq!(text_of(&tree, after[0]), "two");
    assert_eq!(text_of(&tree, after[1]), "one");
}

#[test]
fn bare_axis_constants_match_dsl() {
    use aqua_ui::element::ui::{column, row};
    use aqua_ui::style::Axis;

    let el: aqua_ui::element::Element = ui! {
        crate = crate::real;
        Box(layout: column) {
            Box(layout: row) { Text("x") }
        }
    };
    let rt = Runtime::new();
    let mut tree = UiTree::new();
    let sink = tree.command_sink();
    let mut view = View::new(rt.create_scope(), sink);
    let ids = view.set_content(&mut tree, vec![el]).unwrap();
    layout(&mut tree);
    assert_eq!(tree.get(ids[0]).unwrap().style.axis, Axis::Column);
    let kid = tree.get(ids[0]).unwrap().children[0];
    assert_eq!(tree.get(kid).unwrap().style.axis, Axis::Row);
}

#[test]
fn real_mount_errors_are_explicit() {
    let rt = Runtime::new();
    let mut tree = UiTree::new();
    let sink = tree.command_sink();
    let mut view = View::new(rt.create_scope(), sink);
    assert!(view.set_content(&mut tree, vec![]).is_err());
}
