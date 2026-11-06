use accesskit::{Action, ActionData, NodeId as AkNodeId, TextPosition, TextSelection, TreeId};
use aqua_a11y_uia::{pack_node_id, A11yBridge, A11yTree, BridgeActivationHandler, Connection};
use aqua_core::{NodeId, RawId, Rect, Role, SemanticNode};
use aqua_protocol::{A11yActionData, A11yNode, A11yText, A11yTextRun, AccessTreeUpdate};

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
fn bridge_stays_dormant_until_a_client_activates_then_wakes_for_every_inbound_event() {
    use accesskit::ActivationHandler;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let wakes = Arc::new(AtomicUsize::new(0));
    let count = wakes.clone();
    let bridge = A11yBridge::new(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));
    let update = AccessTreeUpdate {
        root: owner(0),
        focus: owner(0),
        nodes: vec![text_node(0)],
    };
    assert_eq!(bridge.connection(), Connection::Dormant);
    assert!(bridge.snapshot(&update).is_none());
    assert_eq!(wakes.load(Ordering::SeqCst), 0);

    let mut activation = BridgeActivationHandler::new(bridge.clone());
    assert!(activation.request_initial_tree().is_none());
    assert_eq!(bridge.connection(), Connection::Connected);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);

    let snapshot = bridge.snapshot(&update).unwrap();
    bridge.native_action(&selection_request(
        owner(0),
        snapshot.nodes[0].1.children()[0],
    ));
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
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
    assert!(bridge.drain_actions().is_empty());
}
