use accesskit::{Action, ActionData, NodeId as AkNodeId, TextPosition, TextSelection, TreeId};
use aqua_a11y::{pack_node_id, A11yBridge, A11yTree, BridgeActivationHandler, Connection};
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
        scroll: aqua_core::Point::ZERO,
        children: vec![],
        text: Some(A11yText {
            selection_enabled: true,
            links: vec![],
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

#[test]
fn identical_snapshots_are_skipped_and_reactivation_sends_a_full_tree() {
    let bridge = A11yBridge::new(std::sync::Arc::new(|| {}));
    bridge.connect();
    let mut update = AccessTreeUpdate {
        root: owner(0),
        focus: owner(0),
        nodes: vec![text_node(0)],
    };
    assert!(bridge.snapshot(&update).is_some());
    assert!(bridge.snapshot(&update).is_none());
    update.nodes[0].text.as_mut().unwrap().selection = Some((1, 2));
    assert!(bridge.snapshot(&update).is_some());
    assert!(bridge.snapshot(&update).is_none());
    bridge.connect();
    assert!(bridge.snapshot(&update).is_some());
}

#[test]
fn a_departed_assistive_client_stops_snapshots_and_a_returning_one_gets_the_full_tree() {
    let bridge = A11yBridge::new(std::sync::Arc::new(|| {}));
    bridge.connect();
    let update = AccessTreeUpdate {
        root: owner(0),
        focus: owner(0),
        nodes: vec![text_node(0)],
    };
    let full = bridge.snapshot(&update).unwrap().nodes.len();
    assert!(bridge.snapshot(&update).is_none());
    bridge.deactivate();
    assert_eq!(bridge.connection(), Connection::Dormant);
    assert!(bridge.snapshot(&update).is_none());
    bridge.connect();
    assert_eq!(bridge.snapshot(&update).unwrap().nodes.len(), full);
}

#[test]
fn a_leaving_and_returning_client_wakes_once_each_after_the_lock_and_advances_the_epoch() {
    use accesskit::{ActivationHandler, DeactivationHandler};
    use aqua_a11y::BridgeDeactivationHandler;
    use std::sync::{Arc, Mutex, OnceLock, Weak};

    let observed = Arc::new(Mutex::new(Vec::new()));
    let target: Arc<OnceLock<Weak<A11yBridge>>> = Arc::new(OnceLock::new());
    let (seen, reader) = (observed.clone(), target.clone());
    let bridge = A11yBridge::new(Arc::new(move || {
        let bridge = reader.get().and_then(Weak::upgrade).unwrap();
        seen.lock()
            .unwrap()
            .push((bridge.connection(), bridge.epoch()));
    }));
    target.set(Arc::downgrade(&bridge)).unwrap();
    let mut activation = BridgeActivationHandler::new(bridge.clone());
    let mut deactivation = BridgeDeactivationHandler::new(bridge.clone());
    assert_eq!(bridge.epoch(), 0);

    activation.request_initial_tree();
    deactivation.deactivate_accessibility();
    activation.request_initial_tree();

    assert_eq!(
        *observed.lock().unwrap(),
        vec![
            (Connection::Connected, 1),
            (Connection::Dormant, 1),
            (Connection::Connected, 2),
        ]
    );
}

#[test]
fn changed_snapshots_only_publish_changed_nodes_and_keep_text_actions_live() {
    let bridge = A11yBridge::new(std::sync::Arc::new(|| {}));
    bridge.connect();
    let mut update = AccessTreeUpdate {
        root: owner(0),
        focus: owner(0),
        nodes: vec![text_node(0)],
    };
    let initial = bridge.snapshot(&update).unwrap();
    assert!(initial.nodes.len() > 1);
    let run = initial.nodes[0].1.children()[0];
    update.nodes[0].semantic.label = "Updated label".to_owned();
    let changed = bridge.snapshot(&update).unwrap();
    assert_eq!(changed.nodes.len(), 1);
    assert_eq!(changed.nodes[0].0, pack_node_id(owner(0)));
    bridge.native_action(&selection_request(owner(0), run));
    assert_eq!(bridge.drain_actions().len(), 1);
    bridge.connect();
    assert_eq!(
        bridge.snapshot(&update).unwrap().nodes.len(),
        initial.nodes.len()
    );
}

fn region() -> NodeId {
    NodeId::from_raw(RawId {
        index: 3,
        generation: 0,
    })
}

fn scrolled_list(offset: f32, moved: f32) -> AccessTreeUpdate {
    let mut row = text_node(0);
    let mut shift = |rect: &mut Rect| rect.y -= moved;
    row.bounds.iter_mut().for_each(&mut shift);
    row.text
        .iter_mut()
        .flat_map(|text| text.runs.iter_mut())
        .for_each(|run| shift(&mut run.bounds));
    AccessTreeUpdate {
        root: region(),
        focus: region(),
        nodes: vec![
            A11yNode {
                semantic: SemanticNode::new(region(), Role::ScrollRegion),
                bounds: Some(Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 200.0,
                    h: 100.0,
                }),
                scroll: aqua_core::Point::new(0.0, offset),
                children: vec![owner(0)],
                text: None,
            },
            row,
        ],
    }
}

fn published(bridge: &A11yBridge, update: &AccessTreeUpdate) -> Vec<AkNodeId> {
    bridge
        .snapshot(update)
        .map(|snapshot| snapshot.nodes.iter().map(|(id, _)| *id).collect())
        .unwrap_or_default()
}

#[test]
fn scrolling_republishes_only_the_scroll_region_and_keeps_window_bounds() {
    let bridge = A11yBridge::new(std::sync::Arc::new(|| {}));
    bridge.connect();
    assert!(bridge.snapshot(&scrolled_list(0.0, 0.0)).is_some());
    let scrolled = scrolled_list(30.0, 30.0);
    assert_eq!(published(&bridge, &scrolled), vec![pack_node_id(region())]);
    let root = aqua_a11y::inspect(&scrolled).unwrap();
    assert_eq!(root.bounds, Some([0.0, 0.0, 200.0, 100.0]));
    let row = &root.children[0];
    assert_eq!(row.bounds, Some([10.0, -10.0, 110.0, 10.0]));
    assert_eq!(row.children[0].bounds, Some([10.0, -10.0, 70.0, 10.0]));
}

#[test]
fn reanchoring_a_list_republishes_its_rows_but_not_their_contents() {
    let bridge = A11yBridge::new(std::sync::Arc::new(|| {}));
    bridge.connect();
    let initial = bridge.snapshot(&scrolled_list(30.0, 30.0)).unwrap();
    let run = initial.nodes[1].1.children()[0];
    let reanchored = scrolled_list(430.0, 30.0);
    let changed = published(&bridge, &reanchored);
    assert_eq!(
        changed,
        vec![pack_node_id(region()), pack_node_id(owner(0))]
    );
    assert!(!changed.contains(&run));
    let row = &aqua_a11y::inspect(&reanchored).unwrap().children[0];
    assert_eq!(row.bounds, Some([10.0, -10.0, 110.0, 10.0]));
}

fn plain(index: u32, role: Role) -> A11yNode {
    let id = NodeId::from_raw(RawId {
        index,
        generation: 1,
    });
    A11yNode {
        semantic: SemanticNode {
            bounds: Some(Rect {
                x: 0.0,
                y: 0.0,
                w: 50.0,
                h: 20.0,
            }),
            ..SemanticNode::new(id, role)
        },
        bounds: Some(Rect {
            x: 0.0,
            y: 0.0,
            w: 50.0,
            h: 20.0,
        }),
        scroll: aqua_core::Point::ZERO,
        children: vec![],
        text: None,
    }
}

#[test]
fn reply_regions_ranges_scrolling_and_active_options_reach_uia() {
    use accesskit::{Live, Role as AkRole};
    use aqua_core::{LiveRegion, NumericRangeSemantics, ScrollSemantics};
    let mut root = plain(1, Role::ScrollRegion);
    let mut reply = plain(2, Role::Text);
    let mut chart = plain(3, Role::Group);
    let mut progress = plain(4, Role::ProgressIndicator);
    let mut trigger = plain(5, Role::ComboBox);
    let option = plain(6, Role::Option);
    let link = plain(7, Role::Link);
    let mut dialog = plain(8, Role::Dialog);
    root.semantic.scroll = Some(ScrollSemantics {
        x: 0.0,
        y: 40.0,
        x_max: 0.0,
        y_max: 400.0,
    });
    reply.semantic.label = "Partial answer".into();
    reply.semantic.live = Some(LiveRegion::Polite);
    reply.semantic.busy = true;
    reply.semantic.language = Some("nb-NO".into());
    chart.semantic.focusable = true;
    progress.semantic.numeric_range = Some(NumericRangeSemantics {
        min: 0.0,
        max: 100.0,
        value: 30.0,
        step: 1.0,
    });
    trigger.semantic.focused = true;
    trigger.semantic.active_descendant = Some(option.semantic.node);
    dialog.semantic.modal = true;
    let items = [root, reply, chart, progress, trigger, option, link, dialog];
    let ids: Vec<NodeId> = items.iter().map(|item| item.semantic.node).collect();
    let mut items = items.to_vec();
    items[0].children = ids[1..].to_vec();
    let update = A11yTree::default().build(ids[0], ids[4], &items);
    let node = |id: NodeId| {
        &update
            .nodes
            .iter()
            .find(|(packed, _)| *packed == pack_node_id(id))
            .unwrap()
            .1
    };
    let region = node(ids[0]);
    assert_eq!(region.scroll_y(), Some(40.0));
    assert_eq!(region.scroll_y_max(), Some(400.0));
    assert!(region.supports_action(Action::ScrollDown));
    assert!(region.supports_action(Action::SetScrollOffset));
    let reply = node(ids[1]);
    assert_eq!(reply.live(), Some(Live::Polite));
    assert!(reply.is_busy());
    assert_eq!(reply.language(), Some("nb-NO"));
    assert!(reply.supports_action(Action::ScrollIntoView));
    assert!(node(ids[2]).supports_action(Action::Focus));
    assert_eq!(node(ids[3]).numeric_value(), Some(30.0));
    assert_eq!(node(ids[3]).max_numeric_value(), Some(100.0));
    assert!(!node(ids[3]).supports_action(Action::SetValue));
    assert_eq!(node(ids[4]).active_descendant(), Some(pack_node_id(ids[5])));
    assert_eq!(node(ids[6]).role(), AkRole::Link);
    assert!(node(ids[6]).supports_action(Action::Click));
    assert!(node(ids[7]).is_modal());
}

#[test]
fn inline_links_become_named_uia_links_that_press_their_owner() {
    use accesskit::Role as AkRole;
    use aqua_protocol::A11yLink;
    let mut item = text_node(1);
    let owner = item.semantic.node;
    item.text.as_mut().unwrap().links = vec![
        A11yLink {
            label: "docs".into(),
            bounds: Rect {
                x: 10.0,
                y: 20.0,
                w: 30.0,
                h: 20.0,
            },
            focused: false,
        },
        A11yLink {
            label: "guide".into(),
            bounds: Rect {
                x: 50.0,
                y: 20.0,
                w: 30.0,
                h: 20.0,
            },
            focused: true,
        },
    ];
    let mut tree = A11yTree::default();
    let update = tree.build(owner, owner, &[item]);
    let links: Vec<_> = update
        .nodes
        .iter()
        .filter(|(_, node)| node.role() == AkRole::Link)
        .collect();
    assert_eq!(
        links
            .iter()
            .map(|(_, node)| node.label())
            .collect::<Vec<_>>(),
        vec![Some("docs"), Some("guide")]
    );
    assert_eq!(update.focus, links[1].0);
    let pressed = tree
        .translate(&accesskit::ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: links[1].0,
            data: None,
        })
        .unwrap();
    assert_eq!(pressed.node, owner);
    assert_eq!(pressed.data, A11yActionData::Custom(1));
}
