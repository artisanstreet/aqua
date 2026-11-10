use aqua_core::Oklch;
use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::ui::Text;
use aqua_ui::program;
use aqua_ui::style::Style;
use aqua_ui::tree::interaction::{InteractionCommand, InteractionEvent};
use aqua_ui::tree::{NodeKind, UiTree};

#[test]
fn eager_binding_installation_keeps_the_latest_normal_style_after_hover() {
    let graph = Runtime::new();
    let scope = graph.create_scope();
    let (color, set_color) = scope.signal(Oklch::WHITE);
    let mut tree = UiTree::new();
    let mut view = View::new(scope, tree.command_sink());
    view.set_content(
        &mut tree,
        vec![Text::new("interactive")
            .build()
            .bind(program::TextColor, color)
            .hover_style(Style::new().text_color(Oklch::BLACK))],
    )
    .unwrap();
    tree.drain_commands();
    let node = tree.root().unwrap();
    tree.apply_interaction_command(InteractionCommand::Event {
        node,
        event: InteractionEvent::Hover(true),
    });
    let next = Oklch::new(0.6, 0.1, 140.0);
    set_color.set(next);
    tree.drain_commands();
    let NodeKind::Text(text) = &tree.get(node).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.color, Oklch::BLACK);
    tree.apply_interaction_command(InteractionCommand::Event {
        node,
        event: InteractionEvent::Hover(false),
    });
    let NodeKind::Text(text) = &tree.get(node).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.color, next);
    view.dispose();
}

#[test]
fn mounting_many_bindings_delivers_fresh_values_without_spending_the_effect_budget() {
    let graph = Runtime::new();
    graph.set_auto_flush(false);
    graph.set_flush_limit(1);
    let scope = graph.create_scope();
    let (text, set_text) = scope.signal("before construction".to_string());
    let element = aqua_ui::element::ui::Box::new()
        .children((0..3000).map(|_| Text::new(text.clone()).build()))
        .build();
    set_text.set("before mount".into());
    let mut tree = UiTree::new();
    let mut view = View::new(scope, tree.command_sink());
    view.set_content(&mut tree, vec![element]).unwrap();
    assert_eq!(graph.pending(), 0);
    tree.drain_commands();
    let nodes = tree.traverse();
    let last = *nodes.last().unwrap();
    let NodeKind::Text(value) = &tree.get(last).unwrap().kind else {
        panic!()
    };
    assert_eq!(value.text(), "before mount");
    set_text.set("after mount".into());
    assert!(graph.flush().truncated);
    graph.set_flush_limit(4000);
    assert!(!graph.flush().truncated);
    tree.drain_commands();
    let NodeKind::Text(value) = &tree.get(last).unwrap().kind else {
        panic!()
    };
    assert_eq!(value.text(), "after mount");
    view.dispose();
    assert_eq!(graph.pending(), 0);
}
