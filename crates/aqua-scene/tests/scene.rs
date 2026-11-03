use aqua_core::DisplayItem;
use aqua_scene::Scene;

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
    assert_eq!(scene.ordered().map(|(id, _)| id).collect::<Vec<_>>(), vec![second, first]);
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
    assert_eq!(scene.ordered().map(|(id, _)| id).collect::<Vec<_>>(), vec![first, second]);
}
