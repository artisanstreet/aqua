use aqua_core::Oklch;
use aqua_ui::style::{LayoutStyle, Paint, Style};
use aqua_ui::tree::interaction::{
    InputModality, InteractionEvent, InteractionState, InteractionStyleKind, InteractionStyles,
};

#[test]
fn modality_changes_retained_focus_and_hover_without_changing_focus() {
    let mut state = InteractionState::default();
    assert!(state.transition(InteractionEvent::Focus(true)));
    assert!(state.transition(InteractionEvent::Hover(true)));
    assert!(state.hover_visible());
    assert!(!state.focus_visible());
    assert!(state.transition(InteractionEvent::Modality(InputModality::Keyboard)));
    assert!(state.focused);
    assert!(state.focus_visible());
    assert!(!state.hover_visible());
    state.transition(InteractionEvent::Capture(true));
    assert!(state.hover_visible());
    state.transition(InteractionEvent::Capture(false));
    assert!(!state.transition(InteractionEvent::Modality(InputModality::Keyboard)));
    state.transition(InteractionEvent::Modality(InputModality::Touch));
    assert!(!state.focus_visible());
    assert!(!state.hover_visible());
    state.transition(InteractionEvent::Modality(InputModality::Pointer));
    assert!(state.hover_visible());
}

#[test]
fn refinements_follow_gpui_precedence_and_restore_base() {
    let mut styles = InteractionStyles::default();
    styles.set(
        InteractionStyleKind::FocusVisible,
        Style::new().background(Oklch::WHITE),
    );
    styles.set(
        InteractionStyleKind::Hover,
        Style::new().background(Oklch::BLACK),
    );
    styles.set(
        InteractionStyleKind::Pressed,
        Style::new().background(Oklch::TRANSPARENT),
    );
    let mut state = InteractionState::default();
    let base_layout = LayoutStyle::default();
    let base_paint = Paint {
        fill: Some(Oklch::WHITE),
        ..Paint::default()
    };
    state.transition(InteractionEvent::Hover(true));
    assert_eq!(
        styles.resolve(state, &base_layout, &base_paint).1.fill,
        Some(Oklch::BLACK)
    );
    state.transition(InteractionEvent::Press(true));
    assert_eq!(
        styles.resolve(state, &base_layout, &base_paint).1.fill,
        Some(Oklch::TRANSPARENT)
    );
    state.transition(InteractionEvent::Disabled(true));
    assert!(!state.pressed);
    assert_eq!(
        styles.resolve(state, &base_layout, &base_paint).1,
        base_paint
    );
}

#[test]
fn queued_events_update_retained_paint_and_semantics_at_drain() {
    use aqua_ui::tree::interaction::{InteractionCommand, InteractionSemantic};
    use aqua_ui::tree::{BoxContent, NodeKind, NodeSpec, UiTree};

    let mut tree = UiTree::new();
    let node = tree.mount_spec(NodeSpec::new(
        NodeKind::Box(BoxContent::default()),
        LayoutStyle::default(),
    ));
    let sink = tree.command_sink();
    tree.queue_interaction_binding(node, true);
    sink.interaction(InteractionCommand::Style {
        node,
        kind: InteractionStyleKind::Hover,
        style: Box::new(Style::new().background(Oklch::BLACK)),
    });
    sink.interaction(InteractionCommand::Semantics {
        node,
        value: InteractionSemantic::Expanded(Some(true)),
    });
    sink.interaction(InteractionCommand::Semantics {
        node,
        value: InteractionSemantic::Checked(Some(true)),
    });
    tree.queue_interaction_event(node, InteractionEvent::Hover(true));
    assert_eq!(tree.paint(node).unwrap().fill, None);
    assert_eq!(tree.interaction_semantics(node).expanded, None);
    let version = tree.content_version();
    tree.drain_commands();
    assert_eq!(tree.paint(node).unwrap().fill, Some(Oklch::BLACK));
    assert_eq!(tree.interaction_semantics(node).expanded, Some(true));
    assert_eq!(tree.interaction_semantics(node).checked, Some(true));
    assert!(tree.content_version() > version);
    sink.interaction(InteractionCommand::Modality(InputModality::Keyboard));
    tree.drain_commands();
    assert_eq!(tree.paint(node).unwrap().fill, None);
    sink.interaction(InteractionCommand::Modality(InputModality::Pointer));
    tree.drain_commands();
    assert_eq!(tree.paint(node).unwrap().fill, Some(Oklch::BLACK));
}

#[test]
fn semantic_updates_are_ordered_and_removed_nodes_are_ignored() {
    use aqua_ui::tree::interaction::{InteractionCommand, InteractionSemantic};
    use aqua_ui::tree::{BoxContent, NodeKind, NodeSpec, UiTree};

    let mut tree = UiTree::new();
    let node = tree.mount_spec(NodeSpec::new(
        NodeKind::Box(BoxContent::default()),
        LayoutStyle::default(),
    ));
    let sink = tree.command_sink();
    tree.queue_interaction_binding(node, true);
    sink.interaction(InteractionCommand::Semantics {
        node,
        value: InteractionSemantic::Selected(true),
    });
    sink.interaction(InteractionCommand::Semantics {
        node,
        value: InteractionSemantic::Selected(false),
    });
    sink.interaction(InteractionCommand::Semantics {
        node,
        value: InteractionSemantic::Value(Some("two".into())),
    });
    tree.drain_commands();
    let semantics = &tree.get(node).unwrap().semantics;
    assert!(!semantics.selected);
    assert_eq!(semantics.value_text.as_deref(), Some("two"));
    tree.remove(node);
    sink.interaction(InteractionCommand::Semantics {
        node,
        value: InteractionSemantic::Checked(Some(true)),
    });
    tree.drain_commands();
    assert_eq!(tree.interaction_semantics(node).checked, None);
}

#[test]
fn controlled_semantics_follow_signals_after_keyed_refresh() {
    use aqua_reactive::Runtime;
    use aqua_ui::binding::View;
    use aqua_ui::element::Box;
    use aqua_ui::tree::UiTree;

    let runtime = Runtime::new();
    let scope = runtime.create_scope();
    let (checked, set_checked) = scope.signal(false);
    let (expanded, set_expanded) = scope.signal(false);
    let (value, set_value) = scope.signal(String::from("closed"));
    let mut tree = UiTree::new();
    let mut view = View::new(scope, tree.command_sink());
    let content = || {
        vec![Box::new()
            .build()
            .with_key("control")
            .aria_checked_signal(checked.clone())
            .aria_expanded_signal(expanded.clone())
            .aria_value_signal(value.clone())]
    };
    let node = view.set_content(&mut tree, content()).unwrap()[0];
    tree.drain_commands();
    assert_eq!(tree.interaction_semantics(node).checked, Some(false));
    assert_eq!(tree.interaction_semantics(node).expanded, Some(false));
    assert_eq!(
        tree.get(node).unwrap().semantics.value_text.as_deref(),
        Some("closed")
    );
    set_checked.set(true);
    set_expanded.set(true);
    set_value.set(String::from("open"));
    runtime.flush();
    tree.drain_commands();
    assert_eq!(tree.interaction_semantics(node).checked, Some(true));
    assert_eq!(tree.interaction_semantics(node).expanded, Some(true));
    assert_eq!(
        tree.get(node).unwrap().semantics.value_text.as_deref(),
        Some("open")
    );
    assert_eq!(view.set_content(&mut tree, content()).unwrap(), vec![node]);
    tree.drain_commands();
    assert_eq!(tree.interaction_semantics(node).checked, Some(true));
    assert_eq!(tree.interaction_semantics(node).expanded, Some(true));
}
