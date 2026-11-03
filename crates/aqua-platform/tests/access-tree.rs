use accesskit::{Action, ActionData, NodeId as AkNodeId, TextPosition, TextSelection, TreeId};
use aqua_core::{NodeId, RawId, Rect, Role, SemanticNode};
use aqua_platform::a11y::{pack_node_id, A11yActionData, A11yBridge, A11yNode, A11yTree};
use aqua_protocol::{A11yText, A11yTextRun};

fn owner(generation: u32) -> NodeId {
    NodeId::from_raw(RawId {
        index: 7,
        generation,
    })
}

fn text_node(generation: u32) -> A11yNode {
    let owner = owner(generation);
    A11yNode {
        semantic: SemanticNode::new(owner, Role::TextField),
        bounds: Some(Rect {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 20.0,
        }),
        children: vec![],
        text: Some(A11yText {
            selection_enabled: true,
            selection: Some((1, 14)),
            runs: vec![A11yTextRun {
                text: "a👩‍💻é".into(),
                byte_start: 0,
                bounds: Rect {
                    x: 10.0,
                    y: 20.0,
                    w: 60.0,
                    h: 20.0,
                },
                word_starts: vec![0],
                character_lengths: vec![1, 11, 2],
                character_positions: vec![0.0, 10.0, 40.0],
                character_widths: vec![10.0, 30.0, 20.0],
                rtl: false,
            }],
        }),
    }
}

fn selection_request(owner: NodeId, run: AkNodeId) -> accesskit::ActionRequest {
    accesskit::ActionRequest {
        action: Action::SetTextSelection,
        target_tree: TreeId::ROOT,
        target_node: pack_node_id(owner),
        data: Some(ActionData::SetTextSelection(TextSelection {
            anchor: TextPosition {
                node: run,
                character_index: 1,
            },
            focus: TextPosition {
                node: run,
                character_index: 3,
            },
        })),
    }
}

#[test]
fn text_run_ids_are_stable_reserved_and_stale_generations_reject_actions() {
    let mut tree = A11yTree::default();
    let original = text_node(0);
    let first = tree.build(owner(0), owner(0), std::slice::from_ref(&original));
    let run = first.nodes[0].1.children()[0];
    assert_eq!(run.0 >> 32, u64::from(u32::MAX));
    assert_ne!(run, pack_node_id(owner(u32::MAX)));
    assert_eq!(first.nodes[1].1.value(), Some("a👩‍💻é"));
    let next = tree.build(owner(0), owner(0), &[original]);
    assert_eq!(next.nodes[0].1.children(), &[run]);
    let translated = tree.translate(&selection_request(owner(0), run)).unwrap();
    assert_eq!(
        translated.data,
        A11yActionData::TextByteSelection {
            anchor_node: owner(0),
            anchor_byte: 1,
            focus_node: owner(0),
            focus_byte: 14
        }
    );
    let replacement = tree.build(owner(1), owner(1), &[text_node(1)]);
    assert_ne!(replacement.nodes[0].1.children()[0], run);
    assert!(tree.translate(&selection_request(owner(0), run)).is_none());
}

#[test]
fn bridge_remembers_initial_tree_without_waking_and_maps_native_text_selection() {
    let wakes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = wakes.clone();
    let bridge = A11yBridge::new(std::sync::Arc::new(move || {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    let update = bridge
        .build_snapshot(owner(0), owner(0), &[text_node(0)])
        .unwrap();
    assert!(bridge.take_staged().is_some());
    assert_eq!(wakes.load(std::sync::atomic::Ordering::SeqCst), 0);
    bridge.push_native_action(&selection_request(
        owner(0),
        update.nodes[0].1.children()[0],
    ));
    assert_eq!(wakes.load(std::sync::atomic::Ordering::SeqCst), 1);
    let actions = bridge.drain_actions();
    assert_eq!(actions.len(), 1);
    assert!(matches!(
        actions[0].data,
        A11yActionData::TextByteSelection {
            anchor_byte: 1,
            focus_byte: 14,
            ..
        }
    ));
}

#[test]
fn inactive_snapshots_are_staged_once_and_activation_requests_refreshes() {
    use accesskit::{ActivationHandler, DeactivationHandler};
    use aqua_platform::a11y::{BridgeActivationHandler, BridgeDeactivationHandler};
    let bridge = std::sync::Arc::new(A11yBridge::without_waker());
    assert!(bridge.needs_snapshot());
    bridge
        .build_snapshot(owner(0), owner(0), &[text_node(0)])
        .unwrap();
    assert!(!bridge.needs_snapshot());
    let mut activation = BridgeActivationHandler::new(bridge.clone());
    assert!(activation.request_initial_tree().is_some());
    assert!(bridge.needs_snapshot());
    let mut deactivation = BridgeDeactivationHandler::new(bridge.clone());
    deactivation.deactivate_accessibility();
    assert!(!bridge.needs_snapshot());
}
