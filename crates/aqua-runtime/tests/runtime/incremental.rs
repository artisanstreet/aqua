use std::cell::Cell;
use std::rc::Rc;

use aqua_core::{DisplayItem, Oklch};
use aqua_reactive::collection::Collection;
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Text};
use aqua_ui::live::LiveList;
use aqua_ui::program;
use aqua_ui::style::{px, Axis, Style};
use aqua_ui::tree::NodeKind;

fn runtime() -> Runtime {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(800.0, 600.0, 1.0);
    runtime
}

#[test]
fn compiled_properties_patch_only_their_retained_fragment() {
    let mut runtime = runtime();
    runtime.tree().update_log().set_enabled(true);
    let scope = runtime.scope().clone();
    let (label, set_label) = scope.signal("first".to_string());
    let (color, set_color) = scope.signal(Oklch::WHITE);
    runtime
        .mount(|_| {
            UiBox::new()
                .layout(Axis::Column)
                .child(
                    Text::new(label)
                        .width(300)
                        .height(24)
                        .build()
                        .bind(program::TextColor, color),
                )
                .children((0..3000).map(|index| Text::new(index.to_string()).height(24).build()))
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let before: Vec<_> = runtime.scene().ordered().map(|(id, _)| id).collect();
    runtime.tree().update_log().drain();
    set_label.set("streaming update".into());
    runtime.build_frame().unwrap();
    assert_eq!(runtime.frame_work().text_nodes_checked, 1);
    assert_eq!(runtime.frame_work().text_runs_shaped, 1);
    assert_eq!(runtime.frame_work().tree_scene_ops_emitted, 1);
    assert_eq!(runtime.frame_work().scene_items_reconciled, 1);
    assert_eq!(
        runtime
            .scene()
            .ordered()
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        before
    );
    let trace = runtime.tree().update_log().drain();
    assert_eq!(trace.len(), 1);
    assert_eq!(trace[0].cause.source, set_label.node_id());
    assert!(trace[0].changed);
    let work_trace = runtime.tree().update_log().drain_work();
    assert!(work_trace
        .iter()
        .any(|trace| trace.stage == program::WorkStage::TextShaped
            && trace
                .sources
                .iter()
                .any(|cause| cause.source == set_label.node_id())));
    assert!(work_trace
        .iter()
        .any(|trace| trace.stage == program::WorkStage::ScenePatched
            && trace
                .sources
                .iter()
                .any(|cause| cause.source == set_label.node_id())));
    set_color.set(Oklch::BLACK);
    runtime.build_frame().unwrap();
    assert!(runtime.frame_stats().layout_skipped);
    assert_eq!(runtime.frame_work().text_runs_shaped, 0);
    assert_eq!(runtime.frame_work().scene_items_reconciled, 1);
}

#[test]
fn accessibility_properties_do_not_invalidate_paint_or_layout() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let (label, set_label) = scope.signal("first accessible name".to_string());
    runtime
        .mount(|_| {
            Text::new("visible text")
                .build()
                .bind(program::Label, label)
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let epoch = runtime.tree().invalidation_epoch();
    set_label.set("updated accessible name".to_string());
    runtime.build_frame().unwrap();
    assert!(runtime.tree().invalidation_epoch() > epoch);
    assert!(runtime.frame_stats().layout_skipped);
    assert!(runtime.frame_work().tree_scene_reused);
    assert_eq!(runtime.frame_work().text_nodes_checked, 0);
    assert_eq!(runtime.frame_work().tree_scene_ops_emitted, 0);
    assert_eq!(
        runtime.tree().emit_semantics()[0].label,
        "updated accessible name"
    );
}

#[test]
fn bound_opacity_updates_do_not_visit_the_groups_descendants() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let (opacity, set_opacity) = scope.signal(1.0f32);
    runtime
        .mount(|_| {
            UiBox::new()
                .layout(Axis::Column)
                .children((0..3000).map(|index| Text::new(index.to_string()).build()))
                .build()
                .bind(program::Opacity, opacity)
        })
        .unwrap();
    runtime.build_frame().unwrap();
    set_opacity.set(0.5);
    runtime.build_frame().unwrap();
    let work = runtime.frame_work();
    assert!(runtime.frame_stats().layout_skipped);
    assert_eq!(work.text_nodes_checked, 0);
    assert_eq!(work.tree_nodes_visited, 1);
    assert_eq!(work.tree_scene_ops_emitted, 2);
    assert_eq!(work.scene_items_reconciled, 2);
    assert!(runtime
        .scene()
        .ordered()
        .any(|(_, item)| *item == DisplayItem::PushOpacity(0.5)));
}

#[test]
fn bound_background_updates_patch_only_the_containers_paint_slot() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let (color, set_color) = scope.signal(Oklch::WHITE);
    runtime
        .mount(|_| {
            UiBox::new()
                .layout(Axis::Column)
                .children((0..3000).map(|index| Text::new(index.to_string()).build()))
                .build()
                .bind(program::Background, color)
        })
        .unwrap();
    runtime.build_frame().unwrap();
    set_color.set(Oklch::BLACK);
    runtime.build_frame().unwrap();
    let work = runtime.frame_work();
    assert!(runtime.frame_stats().layout_skipped);
    assert_eq!(work.text_nodes_checked, 0);
    assert_eq!(work.tree_nodes_visited, 1);
    assert_eq!(work.tree_scene_ops_emitted, 1);
    assert_eq!(work.scene_items_reconciled, 1);
    let expected: Vec<_> = runtime
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect();
    runtime.tree_mut().invalidate_layout_cache();
    runtime.build_frame().unwrap();
    assert_eq!(
        runtime
            .scene()
            .ordered()
            .map(|(_, item)| item.clone())
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn hover_exit_restores_the_latest_bound_color_and_appearance() {
    use aqua_ui::tree::interaction::{InteractionCommand, InteractionEvent};
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let (color, set_color) = scope.signal(Oklch::WHITE);
    let (opacity, set_opacity) = scope.signal(1.0f32);
    runtime
        .mount(|_| {
            Text::new("interactive")
                .build()
                .hover_style(Style::new().text_color(Oklch::BLACK).opacity(0.25))
                .bind(program::TextColor, color)
                .bind(program::Opacity, opacity)
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let node = runtime.tree().root().unwrap();
    runtime
        .tree_mut()
        .apply_interaction_command(InteractionCommand::Event {
            node,
            event: InteractionEvent::Hover(true),
        });
    runtime.build_frame().unwrap();
    let next = Oklch::new(0.6, 0.1, 140.0);
    set_color.set(next);
    set_opacity.set(0.75);
    runtime.build_frame().unwrap();
    let NodeKind::Text(text) = &runtime.tree().get(node).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.color, Oklch::BLACK);
    assert_eq!(runtime.tree().paint(node).unwrap().opacity, Some(0.25));
    runtime
        .tree_mut()
        .apply_interaction_command(InteractionCommand::Event {
            node,
            event: InteractionEvent::Hover(false),
        });
    runtime.build_frame().unwrap();
    let NodeKind::Text(text) = &runtime.tree().get(node).unwrap().kind else {
        panic!()
    };
    assert_eq!(text.color, next);
    assert_eq!(runtime.tree().paint(node).unwrap().opacity, Some(0.75));
}

#[test]
fn conditional_branches_mount_once_and_dispose_when_replaced() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let (enabled, set_enabled) = scope.signal(true);
    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    runtime
        .mount(|_| {
            aqua_ui::region::when(
                enabled,
                move |owner| {
                    observed.set(observed.get() + 1);
                    let (text, _) = owner.signal("yes".to_string());
                    Text::new(text).build()
                },
                |_| Text::new("no").build(),
            )
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let first = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Text(_)))
        .unwrap();
    runtime.build_frame().unwrap();
    set_enabled.set(true);
    runtime.build_frame().unwrap();
    assert_eq!(calls.get(), 1);
    assert!(runtime.tree().get(first).is_some());
    set_enabled.set(false);
    runtime.build_frame().unwrap();
    assert!(runtime.tree().get(first).is_none());
    assert_eq!(calls.get(), 1);
    set_enabled.set(true);
    runtime.build_frame().unwrap();
    assert_eq!(calls.get(), 2);
}

#[test]
fn contained_layout_changes_do_not_visit_unrelated_panels() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let (text, set_text) = scope.signal("small".to_string());
    runtime
        .mount(|_| {
            UiBox::new()
                .layout(Axis::Column)
                .child(
                    UiBox::new()
                        .layout(Axis::Column)
                        .child(Text::new(text).multiline(true).width(280).build())
                        .child(Text::new("following sibling").height(20).build())
                        .build()
                        .layout_boundary(aqua_core::Size::new(300.0, 100.0)),
                )
                .children((0..1000).map(|index| {
                    UiBox::new()
                        .child(Text::new(index.to_string()).build())
                        .build()
                        .layout_boundary(aqua_core::Size::new(300.0, 100.0))
                }))
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    set_text.set("A much longer message which wraps onto multiple lines inside its own panel without moving other panels.".into());
    runtime.build_frame().unwrap();
    assert!(
        runtime.frame_work().layout_nodes_visited < 20,
        "{:?}",
        runtime.frame_work()
    );
    let nodes = runtime.tree().traverse();
    let expected: Vec<_> = nodes
        .iter()
        .map(|node| runtime.tree().rect(*node))
        .collect();
    let scene: Vec<_> = runtime
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect();
    runtime.tree_mut().invalidate_layout_cache();
    runtime.build_frame().unwrap();
    assert_eq!(
        nodes
            .iter()
            .map(|node| runtime.tree().rect(*node))
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        runtime
            .scene()
            .ordered()
            .map(|(_, item)| item.clone())
            .collect::<Vec<_>>(),
        scene
    );
}

#[test]
fn contained_layout_preserves_rounding_at_fractional_parent_offsets() {
    for scale in [1.0, 1.25, 2.0] {
        let mut runtime = runtime();
        runtime.set_viewport(800.5, 600.7, scale);
        let scope = runtime.scope().clone();
        let (text, set_text) = scope.signal("small".to_string());
        runtime
            .mount(|_| {
                UiBox::new()
                    .width(800.5)
                    .height(600.7)
                    .padding(3.4)
                    .gap(5.9)
                    .child(UiBox::new().width(13.3).height(11.1).build())
                    .child(
                        UiBox::new()
                            .layout(Axis::Column)
                            .padding(2.7)
                            .gap(1.3)
                            .child(Text::new(text).multiline(true).width(240.2).build())
                            .child(Text::new("following").height(20.3).build())
                            .build()
                            .layout_boundary(aqua_core::Size::new(300.3, 120.7)),
                    )
                    .build()
            })
            .unwrap();
        runtime.build_frame().unwrap();
        set_text.set(
            "A longer message which wraps and changes internal layout beneath a fractional offset."
                .into(),
        );
        runtime.build_frame().unwrap();
        let nodes = runtime.tree().traverse();
        let incremental: Vec<_> = nodes
            .iter()
            .map(|node| runtime.tree().rect(*node))
            .collect();
        runtime.tree_mut().invalidate_layout_cache();
        runtime.build_frame().unwrap();
        assert_eq!(
            nodes
                .iter()
                .map(|node| runtime.tree().rect(*node))
                .collect::<Vec<_>>(),
            incremental,
            "scale {scale}"
        );
    }
}

#[test]
fn branch_topology_updates_emit_only_the_owned_region() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let (branch, set_branch) = scope.signal(false);
    let region = aqua_ui::region::when(
        branch,
        |_| {
            UiBox::new()
                .child(Text::new("first").build())
                .child(Text::new("second").build())
                .build()
        },
        |_| Text::new("old branch").build(),
    )
    .layout_boundary(aqua_core::Size::new(300.0, 80.0));
    runtime
        .mount(|_| {
            UiBox::new()
                .layout(Axis::Column)
                .child(region)
                .children((0..1000).map(|index| {
                    Text::new(index.to_string())
                        .build()
                        .layout_boundary(aqua_core::Size::new(300.0, 24.0))
                }))
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    set_branch.set(true);
    runtime.build_frame().unwrap();
    assert!(
        runtime.frame_work().tree_scene_ops_emitted < 10,
        "{:?}",
        runtime.frame_work()
    );
    assert!(
        runtime.frame_work().layout_nodes_visited < 20,
        "{:?}",
        runtime.frame_work()
    );
    let incremental: Vec<_> = runtime
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect();
    runtime.tree_mut().invalidate_layout_cache();
    runtime.build_frame().unwrap();
    assert_eq!(
        runtime
            .scene()
            .ordered()
            .map(|(_, item)| item.clone())
            .collect::<Vec<_>>(),
        incremental
    );
}

#[derive(Clone, PartialEq)]
struct Message {
    text: String,
    unread: bool,
}

#[test]
fn virtual_conversation_keeps_offscreen_records_unsubscribed() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let messages = Collection::new(
        &scope,
        (0..20_000u32).map(|key| {
            (
                key,
                Message {
                    text: format!("Message {key}"),
                    unread: false,
                },
            )
        }),
    )
    .unwrap();
    let unread = messages.count_where(&scope, |message| message.unread);
    let factories = Rc::new(Cell::new(0));
    let observed = factories.clone();
    let list = LiveList::new(messages.clone(), move |owner, _, message| {
        observed.set(observed.get() + 1);
        let text = owner.derive((message,), |(message,)| message.text.clone());
        Text::new(text).width(700).height(32).build()
    })
    .styles([Style::new().width(px(760.0)).height(px(400.0))])
    .build();
    runtime.mount(|_| list).unwrap();
    runtime.build_frame().unwrap();
    assert!(messages.observer_count() > 0);
    assert!(messages.observer_count() < 100);
    let mounted = factories.get();
    messages
        .update(
            &19_999,
            Message {
                text: "offscreen update".into(),
                unread: true,
            },
        )
        .unwrap();
    runtime.build_frame().unwrap();
    assert_eq!(unread.get(), 1);
    assert_eq!(factories.get(), mounted);
    assert_eq!(runtime.frame_work().text_runs_shaped, 0);
    messages
        .update(
            &0,
            Message {
                text: "streaming message".into(),
                unread: true,
            },
        )
        .unwrap();
    runtime.build_frame().unwrap();
    assert_eq!(runtime.frame_work().text_runs_shaped, 1);
    assert_eq!(factories.get(), mounted);
    messages
        .insert(
            20_000,
            Some(0),
            Message {
                text: "new first".into(),
                unread: false,
            },
        )
        .unwrap();
    runtime.build_frame().unwrap();
    assert!(runtime
        .scene()
        .ordered()
        .any(|(_, item)| matches!(item, DisplayItem::Text { .. })));
    assert!(messages.observer_count() < 100);
    runtime.dispose();
    assert_eq!(messages.observer_count(), 0);
}

#[test]
fn replacing_a_record_with_the_same_key_rebinds_the_mounted_row() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let rows = Collection::new(&scope, [(1u32, "old".to_string())]).unwrap();
    let list = LiveList::new(rows.clone(), |owner, _, record| {
        let text = owner.derive((record,), |(text,)| (*text).clone());
        Text::new(text).build()
    })
    .styles([Style::new().width(px(300.0)).height(px(100.0))])
    .build();
    runtime.mount(|_| list).unwrap();
    runtime.build_frame().unwrap();
    scope.batch(|| {
        rows.remove(&1).unwrap();
        rows.insert(1, None, "new".to_string()).unwrap();
    });
    runtime.build_frame().unwrap();
    let texts: Vec<_> = runtime
        .tree()
        .traverse()
        .into_iter()
        .filter_map(|node| match &runtime.tree().get(node)?.kind {
            NodeKind::Text(text) => Some(text.text().to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["new"]);
    assert_eq!(rows.observer_count(), 1);
}

#[test]
fn query_lists_retain_rows_across_filter_changes_and_evict_removed_scopes() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let messages = Collection::new(
        &scope,
        (0..2000u32).map(|key| {
            (
                key,
                Message {
                    text: format!("Message {key}"),
                    unread: key % 2 == 0,
                },
            )
        }),
    )
    .unwrap();
    let query = messages.query(&scope, |message| message.unread, |_| 0u32);
    let list = LiveList::query(query.clone(), |owner, _, record| {
        let text = owner.derive((record,), |(message,)| message.text.clone());
        Text::new(text).editable(true).width(700).height(32).build()
    })
    .styles([Style::new().width(px(760.0)).height(px(400.0))])
    .build();
    runtime.mount(|_| list).unwrap();
    runtime.build_frame().unwrap();
    let first = runtime.tree().traverse().into_iter().find(|node| {
        matches!(&runtime.tree().get(*node).unwrap().kind, NodeKind::Text(text) if text.text() == "Message 0")
    }).unwrap();
    runtime.tree_mut().set_focused(Some(first));
    runtime.build_frame().unwrap();
    query.configure(|_| true, |_| 0u32).unwrap();
    runtime.build_frame().unwrap();
    assert_eq!(runtime.tree().focused(), Some(first));
    assert!(messages.observer_count() < 100);
    messages.remove(&0).unwrap();
    runtime.build_frame().unwrap();
    assert!(runtime.tree().get(first).is_none());
    assert_ne!(runtime.tree().focused(), Some(first));
    assert!(messages.observer_count() < 100);
}

#[test]
fn nested_regions_realize_their_initial_branch_in_the_first_frame() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let (outer, _) = scope.signal(true);
    let (inner, _) = scope.signal(true);
    runtime
        .mount(|_| {
            aqua_ui::region::when(
                outer,
                move |_| {
                    aqua_ui::region::when(
                        inner.clone(),
                        |_| Text::new("nested").build(),
                        |_| Text::new("other").build(),
                    )
                },
                |_| Text::new("closed").build(),
            )
        })
        .unwrap();
    runtime.build_frame().unwrap();
    assert_eq!(
        runtime
            .scene()
            .ordered()
            .filter(|(_, item)| matches!(item, DisplayItem::Text { .. }))
            .count(),
        1
    );
    assert!(runtime.tree().traverse().into_iter().any(|node| matches!(&runtime.tree().get(node).unwrap().kind, NodeKind::Text(text) if text.text() == "nested")));
}

#[test]
fn dynamic_regions_preserve_inherited_typography() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let (open, set_open) = scope.signal(true);
    let messages = Collection::new(
        &scope,
        [(
            1u32,
            Message {
                text: "row".into(),
                unread: false,
            },
        )],
    )
    .unwrap();
    let source = messages.clone();
    let region = aqua_ui::region::when(
        open,
        move |_| {
            LiveList::new(source.clone(), |owner, _, record| {
                let text = owner.derive((record,), |(record,)| record.text.clone());
                Text::new(text).build()
            })
            .styles([Style::new().width(px(300.0)).height(px(100.0))])
            .build()
        },
        |_| Text::new("closed").build(),
    );
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().font_size(28.0))
                .child(region)
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let assert_size = |runtime: &Runtime| {
        let sizes: Vec<_> = runtime
            .tree()
            .traverse()
            .into_iter()
            .filter_map(|node| match &runtime.tree().get(node)?.kind {
                NodeKind::Text(text) => Some(text.font_size),
                _ => None,
            })
            .collect();
        assert!(!sizes.is_empty());
        assert!(sizes.iter().all(|size| *size == 28.0));
    };
    assert_size(&runtime);
    messages
        .insert(
            2,
            None,
            Message {
                text: "second".into(),
                unread: true,
            },
        )
        .unwrap();
    runtime.build_frame().unwrap();
    assert_size(&runtime);
    set_open.set(false);
    runtime.build_frame().unwrap();
    assert_size(&runtime);
}

#[test]
fn invalid_branch_does_not_destroy_the_previous_mounted_branch() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let (valid, set_valid) = scope.signal(true);
    runtime
        .mount(|_| {
            aqua_ui::region::when(
                valid,
                |_| Text::new("kept").build(),
                |_| aqua_ui::element::Element::fragment(Vec::new()),
            )
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let before: Vec<_> = runtime.tree().traverse();
    set_valid.set(false);
    assert!(runtime.build_frame().is_err());
    assert_eq!(runtime.tree().traverse(), before);
    set_valid.set(true);
    runtime.build_frame().unwrap();
    assert_eq!(runtime.tree().traverse(), before);
}

#[test]
fn unsupported_property_targets_fail_before_mounting_nodes() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let (text, _) = scope.signal("invalid target".to_string());
    let outcome = runtime.mount(|_| UiBox::new().build().bind(program::Text, text));
    assert!(outcome.is_err());
    assert!(runtime.tree().root().is_none());
}

#[test]
fn dense_paint_updates_preserve_scene_identity_and_resume_sparse_updates() {
    let mut runtime = runtime();
    let scope = runtime.scope().clone();
    let (label, set_label) = scope.signal("first".to_string());
    runtime
        .mount(|_| {
            UiBox::new()
                .layout(Axis::Column)
                .child(Text::new(label).width(300).height(24).build())
                .children(
                    (0..128).map(|_| UiBox::new().width(8).height(8).fill(Oklch::BLACK).build()),
                )
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let identities: Vec<_> = runtime.scene().ordered().map(|(id, _)| id).collect();
    let boxes: Vec<_> = runtime
        .tree()
        .traverse()
        .into_iter()
        .filter(|node| runtime.tree().paint(*node).unwrap().fill.is_some())
        .collect();
    for color in [Oklch::WHITE, Oklch::BLACK] {
        for node in &boxes {
            let mut paint = runtime.tree().paint(*node).unwrap();
            paint.fill = Some(color);
            runtime.tree_mut().set_paint(*node, paint);
        }
        runtime.build_frame().unwrap();
        assert_eq!(
            runtime
                .scene()
                .ordered()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            identities
        );
        let updated = runtime
            .scene()
            .ordered()
            .filter(|(_, item)| matches!(item, DisplayItem::Rect { fill, .. } if *fill == color))
            .count();
        assert_eq!(updated, boxes.len());
    }
    runtime.set_viewport(900.0, 600.0, 1.25);
    runtime.build_frame().unwrap();
    set_label.set("sparse after dense resize".into());
    runtime.build_frame().unwrap();
    assert_eq!(runtime.frame_work().text_nodes_checked, 1);
    assert_eq!(runtime.frame_work().scene_items_reconciled, 1);
    let incremental: Vec<_> = runtime
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect();
    runtime.tree_mut().invalidate_layout_cache();
    runtime.build_frame().unwrap();
    assert_eq!(
        runtime
            .scene()
            .ordered()
            .map(|(_, item)| item.clone())
            .collect::<Vec<_>>(),
        incremental
    );
}

#[test]
fn adding_paint_operations_after_a_dense_frame_matches_full_reconstruction() {
    let mut runtime = runtime();
    runtime
        .mount(|_| {
            UiBox::new()
                .children(
                    (0..64).map(|_| UiBox::new().width(8).height(8).fill(Oklch::BLACK).build()),
                )
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let root = runtime.tree().root().unwrap();
    for node in runtime.tree().get(root).unwrap().children.clone() {
        let mut paint = runtime.tree().paint(node).unwrap();
        paint.fill = Some(Oklch::WHITE);
        runtime.tree_mut().set_paint(node, paint);
    }
    runtime.build_frame().unwrap();
    let mut paint = runtime.tree().paint(root).unwrap();
    paint.opacity = Some(0.5);
    runtime.tree_mut().set_paint(root, paint);
    runtime.build_frame().unwrap();
    let incremental: Vec<_> = runtime
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect();
    assert!(incremental
        .iter()
        .any(|item| matches!(item, DisplayItem::PushOpacity(value) if *value == 0.5)));
    runtime.tree_mut().invalidate_layout_cache();
    runtime.build_frame().unwrap();
    assert_eq!(
        runtime
            .scene()
            .ordered()
            .map(|(_, item)| item.clone())
            .collect::<Vec<_>>(),
        incremental
    );
}

#[test]
fn repainting_a_culled_child_does_not_reuse_its_old_parent_context() {
    let mut runtime = runtime();
    runtime
        .mount(|_| {
            UiBox::new()
                .child(
                    UiBox::new()
                        .width(100)
                        .height(100)
                        .clip(true)
                        .child(UiBox::new().width(20).height(20).fill(Oklch::BLACK).build())
                        .build(),
                )
                .build()
        })
        .unwrap();
    runtime.build_frame().unwrap();
    let parent = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| runtime.tree().paint(*node).unwrap().clip)
        .unwrap();
    let child = runtime.tree().get(parent).unwrap().children[0];
    let mut paint = runtime.tree().paint(parent).unwrap();
    paint.transform = Some(aqua_core::Transform2D::translate(2000.0, 0.0));
    runtime.tree_mut().set_paint(parent, paint);
    runtime.build_frame().unwrap();
    let mut paint = runtime.tree().paint(child).unwrap();
    paint.fill = Some(Oklch::WHITE);
    runtime.tree_mut().set_paint(child, paint);
    runtime.build_frame().unwrap();
    let culled: Vec<_> = runtime
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect();
    runtime.tree_mut().invalidate_layout_cache();
    runtime.build_frame().unwrap();
    assert_eq!(
        runtime
            .scene()
            .ordered()
            .map(|(_, item)| item.clone())
            .collect::<Vec<_>>(),
        culled
    );
    let mut paint = runtime.tree().paint(parent).unwrap();
    paint.transform = None;
    runtime.tree_mut().set_paint(parent, paint);
    runtime.build_frame().unwrap();
    assert!(runtime
        .scene()
        .ordered()
        .any(|(_, item)| matches!(item, DisplayItem::Rect { fill, .. } if *fill == Oklch::WHITE)));
}
