use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::ui;
use aqua_ui::tree::{NodeKind, UiTree};

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
