use aqua_core::{DisplayItem, NodeId};
use aqua_scene::Scene;

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
