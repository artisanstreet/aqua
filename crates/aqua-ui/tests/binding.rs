use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::ui;
use aqua_ui::tree::{NodeKind, UiTree};

#[test]
fn paint_refresh_reuses_empty_bindings_and_unbound_metadata() {
    use aqua_core::Oklch;
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let content = |fill| {
        vec![ui::Box::new()
            .children((0..100).map(|_| ui::Box::new().fill(fill).build()))
            .build()]
    };
    let ids = view.set_content(&mut tree, content(Oklch::BLACK)).unwrap();
    assert!(tree.drain_commands() > 0);
    view.set_content(&mut tree, content(Oklch::WHITE)).unwrap();
    assert_eq!(tree.drain_commands(), 0);
    assert_eq!(view.root(), Some(ids[0]));
    let child = tree.get(ids[0]).unwrap().children[0];
    tree.get_mut(child).unwrap().debug_selector = Some("temporary".into());
    view.set_content(&mut tree, content(Oklch::BLACK)).unwrap();
    assert!(tree.drain_commands() > 0);
    assert_eq!(tree.get(child).unwrap().debug_selector, None);
    view.set_content(&mut tree, content(Oklch::WHITE)).unwrap();
    assert_eq!(tree.drain_commands(), 0);
}

#[test]
fn plain_refresh_removes_out_of_band_bound_input() {
    use aqua_core::CursorIcon;
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let plain = || vec![ui::Box::new().build()];
    let node = view.set_content(&mut tree, plain()).unwrap()[0];
    tree.drain_commands();
    tree.set_cursor_bound(node, CursorIcon::Pointer);
    view.set_content(&mut tree, plain()).unwrap();
    assert!(tree.drain_commands() > 0);
    view.set_content(&mut tree, plain()).unwrap();
    assert_eq!(tree.drain_commands(), 0);
}

#[test]
fn multiple_refreshes_before_drain_keep_the_last_typography() {
    use aqua_core::Oklch;
    use aqua_ui::style::Style;
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let content = |color| {
        vec![ui::Box::new()
            .style(Style::new().text_color(color))
            .child(ui::Text::new("label").build())
            .build()]
    };
    let root = view.set_content(&mut tree, content(Oklch::BLACK)).unwrap()[0];
    tree.drain_commands();
    for color in [Oklch::WHITE, Oklch::BLACK] {
        view.set_content(&mut tree, content(color)).unwrap();
    }
    tree.drain_commands();
    let child = tree.get(root).unwrap().children[0];
    let NodeKind::Text(text) = &tree.get(child).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.color, Oklch::BLACK);
}

#[test]
fn plain_nodes_do_not_allocate_reactive_scopes_across_refreshes() {
    let runtime = Runtime::new();
    let mut tree = UiTree::new();
    let mut view = View::new(runtime.create_scope(), tree.command_sink());
    let baseline = runtime.stats();
    for count in [100, 200, 50, 0] {
        view.set_content(
            &mut tree,
            vec![ui::Box::new()
                .children(
                    (0..count).map(|key| ui::Text::new(format!("row {key}")).build().with_key(key)),
                )
                .build()],
        )
        .unwrap();
        tree.drain_commands();
        assert_eq!(runtime.stats(), baseline);
    }
    view.dispose();
    assert_eq!(runtime.stats().live_scopes, 0);
}

#[test]
fn switching_between_reactive_and_plain_bindings_disposes_stale_sources() {
    let runtime = Runtime::new();
    let scope = runtime.create_scope();
    let (source, set_source) = scope.signal(String::from("initial"));
    let mut tree = UiTree::new();
    let mut view = View::new(scope, tree.command_sink());
    let baseline = runtime.stats();
    let plain = || vec![ui::Text::new("plain").build().with_key("label")];
    let reactive = || {
        vec![ui::Text::builder()
            .value(source.clone())
            .build()
            .with_key("label")]
    };
    let node = view.set_content(&mut tree, plain()).unwrap()[0];
    tree.drain_commands();
    for index in 0..3 {
        assert_eq!(view.set_content(&mut tree, reactive()).unwrap(), vec![node]);
        tree.drain_commands();
        assert_eq!(runtime.stats().effects, baseline.effects + 1);
        assert_eq!(runtime.stats().live_scopes, baseline.live_scopes + 1);
        set_source.set(format!("live {index}"));
        runtime.flush();
        tree.drain_commands();
        let NodeKind::Text(text) = &tree.get(node).unwrap().kind else {
            panic!("expected retained text node");
        };
        assert_eq!(text.text(), format!("live {index}"));
        assert_eq!(view.set_content(&mut tree, plain()).unwrap(), vec![node]);
        tree.drain_commands();
        assert_eq!(runtime.stats().effects, baseline.effects);
        assert_eq!(runtime.stats().live_scopes, baseline.live_scopes);
        set_source.set(format!("stale {index}"));
        runtime.flush();
        assert_eq!(tree.pending_commands(), 0);
    }
}

fn keyed_list(keys: &[u32]) -> Vec<aqua_ui::element::Element> {
    use aqua_core::Oklch;
    use aqua_ui::collection::List;
    vec![List::new()
        .items(keys.to_vec())
        .key(|key: &u32| *key)
        .revision(|key: &u32| Some(u64::from(*key)))
        .row(|key: &u32| {
            ui::Box::builder()
                .height(20.0)
                .fill(Oklch::BLACK)
                .child(ui::Text::new(format!("row-{key}")).build())
                .build()
        })
        .build()]
}

fn show_list(view: &mut View, tree: &mut UiTree, keys: &[u32]) -> aqua_core::NodeId {
    use aqua_core::Size;
    use aqua_ui::layout::NaiveMeasurer;
    let root = view.set_content(tree, keyed_list(keys)).unwrap()[0];
    view.compute_layout(tree, Size::new(400.0, 600.0), &NaiveMeasurer)
        .unwrap();
    tree.drain_commands();
    root
}

fn mounted_rows(tree: &UiTree, root: aqua_core::NodeId) -> Vec<String> {
    tree.traverse()
        .into_iter()
        .filter(|node| *node != root)
        .filter(|node| {
            std::iter::successors(Some(*node), |node| tree.get(*node).and_then(|r| r.parent))
                .any(|ancestor| ancestor == root)
        })
        .filter_map(|node| match &tree.get(node)?.kind {
            NodeKind::Text(text) => Some(text.text().to_string()),
            _ => None,
        })
        .collect()
}

fn painted_inside(tree: &mut UiTree, root: aqua_core::NodeId) -> usize {
    let descendants: std::collections::HashSet<_> = tree
        .traverse()
        .into_iter()
        .filter(|node| *node != root)
        .filter(|node| {
            std::iter::successors(Some(*node), |node| tree.get(*node).and_then(|r| r.parent))
                .any(|ancestor| ancestor == root)
        })
        .collect();
    tree.emit_scene()
        .iter()
        .filter(|(node, _)| descendants.contains(node))
        .count()
}

fn labels(keys: &[u32]) -> Vec<String> {
    keys.iter().map(|key| format!("row-{key}")).collect()
}

#[test]
fn refreshed_lists_mount_and_paint_exactly_the_new_rows() {
    for (before, after) in [
        (&[1, 2, 3, 4, 5][..], &[][..]),
        (&[1, 2, 3, 4, 5][..], &[1, 2][..]),
        (&[1, 2, 3, 4, 5][..], &[4, 2][..]),
        (&[1, 2, 3][..], &[3, 2, 1][..]),
        (&[][..], &[7, 8, 9][..]),
    ] {
        let runtime = Runtime::new();
        let mut tree = UiTree::new();
        let mut view = View::new(runtime.create_scope(), tree.command_sink());
        let root = show_list(&mut view, &mut tree, before);
        assert_eq!(mounted_rows(&tree, root), labels(before));
        assert_eq!(show_list(&mut view, &mut tree, after), root);
        assert_eq!(
            mounted_rows(&tree, root),
            labels(after),
            "{before:?} -> {after:?}"
        );
        if after.is_empty() {
            assert_eq!(painted_inside(&mut tree, root), 0, "{before:?} -> []");
        } else {
            assert!(painted_inside(&mut tree, root) > 0);
        }
    }
}
