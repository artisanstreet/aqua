use aqua_core::{DisplayItem, NodeId};
use aqua_scene::Scene;

fn filled(color: aqua_core::Oklch) -> DisplayItem {
    DisplayItem::Rect {
        bounds: aqua_core::Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        },
        radius: 0.0,
        fill: color,
        border_width: 0.0,
        border_color: aqua_core::Oklch::TRANSPARENT,
        shadow: None,
    }
}

#[test]
fn fill_changes_coalesce_without_losing_pending_geometry() {
    use aqua_core::Oklch;
    let mut scene = Scene::new();
    let node = scene.insert(filled(Oklch::BLACK));
    scene.update_fill(node, Oklch::WHITE);
    let (upserts, fills) = scene.drain_paint_changes();
    assert_eq!(upserts, vec![(node, filled(Oklch::WHITE))]);
    assert!(fills.is_empty());
    let geometry = scene.geometry_revision();
    scene.update_fill(node, Oklch::BLACK);
    scene.update_fill(node, Oklch::WHITE);
    let (upserts, fills) = scene.drain_paint_changes();
    assert!(upserts.is_empty());
    assert_eq!(
        fills,
        vec![aqua_core::FillPatch {
            node,
            fill: Oklch::WHITE
        }]
    );
    assert_eq!(scene.geometry_revision(), geometry);
    scene.update_fill(node, Oklch::BLACK);
    let mut changed = filled(Oklch::BLACK);
    if let DisplayItem::Rect { bounds, .. } = &mut changed {
        bounds.w = 20.0;
    }
    scene.update(node, changed.clone());
    scene.update_fill(node, Oklch::WHITE);
    let (upserts, fills) = scene.drain_paint_changes();
    assert!(fills.is_empty());
    if let DisplayItem::Rect { fill, .. } = &mut changed {
        *fill = Oklch::WHITE;
    }
    assert_eq!(upserts, vec![(node, changed)]);
}

#[test]
fn retained_fill_reads_require_a_complete_acknowledged_baseline() {
    use aqua_core::Oklch;
    let mut scene = Scene::new();
    let node = scene.insert(filled(Oklch::BLACK));
    assert!(scene.drain_fill_changes(scene.revision()).is_none());
    scene.acknowledge_changes();
    let baseline = scene.revision();
    scene.update_fill(node, Oklch::WHITE);
    assert_eq!(
        scene.drain_fill_changes(baseline).unwrap(),
        vec![aqua_core::FillPatch {
            node,
            fill: Oklch::WHITE
        }]
    );
    assert!(scene.drain_fill_changes(baseline).is_none());
    let baseline = scene.revision();
    scene.update(node, filled(Oklch::BLACK));
    assert!(scene.drain_fill_changes(baseline).is_none());
    assert_eq!(scene.drain_changes(), vec![(node, filled(Oklch::BLACK))]);
}

#[test]
fn removed_and_reused_fill_slots_never_accept_stale_generations() {
    use aqua_core::Oklch;
    let mut scene = Scene::new();
    let old = scene.insert(filled(Oklch::BLACK));
    scene.drain_paint_changes();
    scene.update_fill(old, Oklch::WHITE);
    scene.remove(old);
    let new = scene.insert(filled(Oklch::BLACK));
    assert_eq!(old.raw().index, new.raw().index);
    assert!(!scene.update_fill(old, Oklch::WHITE));
    let (upserts, fills) = scene.drain_paint_changes();
    assert!(fills.is_empty());
    assert_eq!(upserts, vec![(new, filled(Oklch::BLACK))]);
}

fn quad(x: f32, _y: f32) -> DisplayItem {
    DisplayItem::PushOpacity(x / 1000.0)
}

#[test]
fn reordering_retains_ids_and_changes_only_structure() {
    let mut scene = Scene::new();
    let first = scene.insert(DisplayItem::PushOpacity(0.25));
    let second = scene.insert(DisplayItem::PopOpacity);
    scene.drain_changes();
    scene.take_structure_changed();
    let revision = scene.revision();
    assert!(scene.set_order(&[second, first]));
    assert!(scene.revision() > revision);
    assert!(scene.take_structure_changed());
    assert!(scene.drain_changes().is_empty());
    assert_eq!(
        scene.ordered().map(|(id, _)| id).collect::<Vec<_>>(),
        vec![second, first]
    );
    assert_eq!(scene.get(first), Some(&DisplayItem::PushOpacity(0.25)));
    assert!(scene.set_order(&[second, first]));
    assert!(!scene.take_structure_changed());
}

#[test]
fn invalid_orders_leave_scene_and_revision_unchanged() {
    let mut scene = Scene::new();
    let first = scene.insert(DisplayItem::PushOpacity(0.25));
    let stale = scene.insert(DisplayItem::PopOpacity);
    scene.remove(stale);
    let second = scene.insert(DisplayItem::PushOpacity(0.5));
    let revision = scene.revision();
    assert!(!scene.set_order(&[first]));
    assert!(!scene.set_order(&[first, first]));
    assert!(!scene.set_order(&[first, stale]));
    assert_eq!(scene.revision(), revision);
    assert_eq!(
        scene.ordered().map(|(id, _)| id).collect::<Vec<_>>(),
        vec![first, second]
    );
}

fn changed_positions(scene: &Scene, since: u64) -> Option<Vec<usize>> {
    let mut positions = Vec::new();
    scene
        .changed_since(since, |position| positions.push(position))
        .then_some(positions)
}

#[test]
fn change_log_reports_positions_written_after_a_revision() {
    let mut scene = Scene::new();
    let ids: Vec<NodeId> = (0..6).map(|i| scene.insert(quad(i as f32, 0.0))).collect();
    let stamp = scene.revision();
    assert_eq!(changed_positions(&scene, stamp), Some(vec![]));
    scene.update(ids[4], quad(40.0, 1.0));
    scene.update(ids[1], quad(10.0, 1.0));
    scene.update(ids[4], quad(41.0, 1.0));
    let mut found = changed_positions(&scene, stamp).unwrap();
    found.sort_unstable();
    found.dedup();
    assert_eq!(found, vec![1, 4]);
    assert_eq!(scene.position(ids[4]), Some(4));
}

#[test]
fn change_log_forgets_old_history_and_says_so() {
    let mut scene = Scene::new();
    let id = scene.insert(quad(0.0, 0.0));
    let stamp = scene.revision();
    for step in 0..10_000 {
        scene.update(id, quad(step as f32 + 1.0, 0.0));
    }
    assert_eq!(changed_positions(&scene, stamp), None);
    let recent = scene.revision() - 3;
    assert_eq!(changed_positions(&scene, recent), Some(vec![0, 0, 0]));
}

#[test]
fn positions_follow_removals_and_reordering() {
    let mut scene = Scene::new();
    let ids: Vec<NodeId> = (0..4).map(|i| scene.insert(quad(i as f32, 0.0))).collect();
    let before = scene.struct_version();
    scene.remove(ids[1]);
    assert_ne!(scene.struct_version(), before);
    assert_eq!(scene.position(ids[1]), None);
    assert_eq!(scene.position(ids[3]), Some(2));
    scene.set_order(&[ids[3], ids[0], ids[2]]);
    assert_eq!(scene.position(ids[3]), Some(0));
    assert_eq!(scene.item_at(2).map(|(id, _)| id), Some(ids[2]));
}

#[test]
fn recycled_fill_batches_coalesce_writes_without_reusing_previous_indices() {
    use aqua_core::Oklch;
    let mut scene = Scene::new();
    let first = scene.insert(filled(Oklch::BLACK));
    let second = scene.insert(filled(Oklch::BLACK));
    scene.drain_paint_changes();
    scene.update_fill(first, Oklch::WHITE);
    scene.update_fill(second, Oklch::WHITE);
    scene.update_fill(first, Oklch::BLACK);
    let (upserts, fills) = scene.drain_paint_changes();
    assert!(upserts.is_empty());
    assert_eq!(
        fills,
        vec![
            aqua_core::FillPatch {
                node: first,
                fill: Oklch::BLACK
            },
            aqua_core::FillPatch {
                node: second,
                fill: Oklch::WHITE
            },
        ]
    );
    scene.recycle_fills(fills);
    scene.update_fill(second, Oklch::BLACK);
    scene.update_fill(first, Oklch::WHITE);
    let (upserts, fills) = scene.drain_paint_changes();
    assert!(upserts.is_empty());
    assert_eq!(
        fills,
        vec![
            aqua_core::FillPatch {
                node: second,
                fill: Oklch::BLACK
            },
            aqua_core::FillPatch {
                node: first,
                fill: Oklch::WHITE
            },
        ]
    );
    scene.recycle_fills(fills);
    scene.update_fill(first, Oklch::BLACK);
    assert_eq!(scene.drain_changes(), vec![(first, filled(Oklch::BLACK))]);
    scene.update_fill(first, Oklch::WHITE);
    let (_, fills) = scene.drain_paint_changes();
    assert_eq!(
        fills,
        vec![aqua_core::FillPatch {
            node: first,
            fill: Oklch::WHITE
        }]
    );
}

#[test]
fn native_batches_survive_mixed_full_updates_and_stale_generations() {
    use aqua_core::{Srgba8, SrgbaFillBatch, SrgbaFillPatch};
    let mut scene = Scene::new();
    let red = Srgba8::new(255, 0, 0, 255);
    let green = Srgba8::new(0, 255, 0, 255);
    let original = DisplayItem::SrgbaRect {
        bounds: aqua_core::Rect {
            x: 1.25,
            y: 2.5,
            w: 3.75,
            h: 4.0,
        },
        fill: red,
    };
    let first = scene.insert(original.clone());
    let second = scene.insert(original.clone());
    scene.acknowledge_changes();
    let revision = scene.revision();
    let mut batch = SrgbaFillBatch::default();
    batch.extend([
        SrgbaFillPatch {
            node: first,
            fill: green,
        },
        SrgbaFillPatch {
            node: second,
            fill: green,
        },
    ]);
    assert!(scene.apply_srgba_batch(batch));
    let (oklch, batches) = scene.drain_color_changes(revision).unwrap();
    assert!(oklch.is_empty());
    assert_eq!(batches.iter().map(|batch| batch.len()).sum::<usize>(), 2);
    assert!(
        matches!(scene.get(first), Some(DisplayItem::SrgbaRect { fill, .. }) if *fill == green)
    );
    let mut next = batches[0].sibling();
    next.push(SrgbaFillPatch {
        node: first,
        fill: red,
    });
    assert!(scene.apply_srgba_batch(next));
    let mut moved = original.clone();
    if let DisplayItem::SrgbaRect { bounds, .. } = &mut moved {
        bounds.x += 1.0;
    }
    scene.update(first, moved);
    scene.remove(second);
    let replacement = scene.insert(original);
    let mut stale = SrgbaFillBatch::default();
    stale.push(SrgbaFillPatch {
        node: second,
        fill: green,
    });
    assert!(!scene.apply_srgba_batch(stale));
    let (upserts, _, batches) = scene.drain_updates();
    assert!(batches.is_empty());
    assert!(upserts.iter().any(|(node, _)| *node == replacement));
    assert!(
        matches!(scene.get(replacement), Some(DisplayItem::SrgbaRect { fill, .. }) if *fill == red)
    );
}

#[test]
fn large_native_batches_report_lost_history_and_preserve_latest_contents() {
    use aqua_core::{Srgba8, SrgbaFillBatch, SrgbaFillPatch};
    let mut scene = Scene::new();
    let black = Srgba8::new(0, 0, 0, 255);
    let white = Srgba8::new(255, 255, 255, 255);
    let bounds = aqua_core::Rect {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };
    let nodes = (0..5000)
        .map(|_| {
            scene.insert(DisplayItem::SrgbaRect {
                bounds,
                fill: black,
            })
        })
        .collect::<Vec<_>>();
    scene.acknowledge_changes();
    let before = scene.revision();
    let mut batch = SrgbaFillBatch::default();
    batch.extend(
        nodes
            .iter()
            .copied()
            .map(|node| SrgbaFillPatch { node, fill: white }),
    );
    assert!(scene.apply_srgba_batch(batch));
    assert_eq!(changed_positions(&scene, before), None);
    let revision = scene.revision();
    assert_eq!(changed_positions(&scene, revision), Some(Vec::new()));
    assert!(nodes.iter().all(|node| matches!(scene.get(*node), Some(DisplayItem::SrgbaRect { fill, .. }) if *fill == white)));
    let (_, batches) = scene.drain_color_changes(before).unwrap();
    assert_eq!(
        batches.iter().map(|batch| batch.len()).sum::<usize>(),
        nodes.len()
    );
    let mut small = batches[0].sibling();
    small.push(SrgbaFillPatch {
        node: nodes[7],
        fill: black,
    });
    assert!(scene.apply_srgba_batch(small));
    assert_eq!(changed_positions(&scene, revision), Some(vec![7]));
}
