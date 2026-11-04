use aqua_reactive::Runtime;
use aqua_ui::binding::View;
use aqua_ui::element::Box;
use aqua_ui::tree::UiTree;

#[test]
fn mounted_bindings_invalidate_only_their_live_tree_on_every_signal_write() {
    let runtime = Runtime::new();
    runtime.set_auto_flush(false);
    let signals = runtime.create_scope();
    let (first, set_first) = signals.signal(false);
    let (second, set_second) = signals.signal(false);
    let (unrelated, set_unrelated) = signals.signal(false);
    signals.subscribe(&unrelated, |_| {});
    let mut first_tree = UiTree::new();
    let mut second_tree = UiTree::new();
    let mut first_view = View::new(runtime.create_scope(), first_tree.command_sink());
    let mut second_view = View::new(runtime.create_scope(), second_tree.command_sink());
    first_view
        .set_content(
            &mut first_tree,
            vec![Box::new().build().aria_checked_signal(first)],
        )
        .unwrap();
    second_view
        .set_content(
            &mut second_tree,
            vec![Box::new().build().aria_checked_signal(second)],
        )
        .unwrap();
    runtime.flush();
    first_tree.drain_commands();
    second_tree.drain_commands();
    let first_before = first_tree.invalidation_epoch();
    let second_before = second_tree.invalidation_epoch();
    set_unrelated.set(true);
    assert_eq!(first_tree.invalidation_epoch(), first_before);
    assert_eq!(second_tree.invalidation_epoch(), second_before);
    set_first.set(true);
    let first_queued = first_tree.invalidation_epoch();
    assert_ne!(first_queued, first_before);
    assert_eq!(second_tree.invalidation_epoch(), second_before);
    set_first.set(false);
    assert_ne!(first_tree.invalidation_epoch(), first_queued);
    set_second.set(true);
    assert_ne!(second_tree.invalidation_epoch(), second_before);
    first_view.dispose();
    let disposed_epoch = first_tree.invalidation_epoch();
    set_first.set(true);
    assert_eq!(first_tree.invalidation_epoch(), disposed_epoch);
}
