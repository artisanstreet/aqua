use aqua_core::geometry::Transform2D;
use aqua_core::Oklch;
use aqua_scene::scene::{DisplayItem, NodeId, Rect, Scene, TextRun};
use aqua_scene::{DamageSet, DamageTracker, Footprints};

fn quad(x: f32, y: f32) -> DisplayItem {
    DisplayItem::Rect {
        bounds: Rect {
            x,
            y,
            w: 10.0,
            h: 10.0,
        },
        radius: 0.0,
        fill: Oklch::from_hex("#ff0000").unwrap(),
        border_width: 0.0,
        border_color: Oklch::TRANSPARENT,
        shadow: None,
    }
}

fn no_ink(_: &TextRun) -> Option<Rect> {
    None
}

fn damage(before: &Footprints, after: &Footprints) -> DamageSet {
    let mut tracker = DamageTracker::new(1.0);
    tracker.set_frame_size(400.0, 300.0);
    tracker.record_scene_diff(before, after);
    tracker.take()
}

fn covers(set: &DamageSet, rect: Rect) -> bool {
    set.full
        || set.damage.iter().any(|d| {
            d.x <= rect.x
                && d.y <= rect.y
                && d.right() >= rect.right()
                && d.bottom() >= rect.bottom()
        })
}

struct Rig {
    scene: Scene,
    push: NodeId,
    inner: Vec<NodeId>,
}

fn rig(count: usize) -> Rig {
    let mut scene = Scene::new();
    let push = scene.insert(DisplayItem::PushTransform(Transform2D::translate(0.0, 0.0)));
    let inner = (0..count)
        .map(|i| scene.insert(quad(i as f32 * 20.0, 10.0)))
        .collect();
    scene.insert(DisplayItem::PopTransform);
    Rig { scene, push, inner }
}

#[test]
fn translating_a_group_damages_old_and_new_extent_once() {
    let mut rig = rig(30);
    let before = Footprints::of_scene(&rig.scene, &no_ink);
    rig.scene.update(
        rig.push,
        DisplayItem::PushTransform(Transform2D::translate(50.0, 20.0)),
    );
    let after = Footprints::of_scene(&rig.scene, &no_ink);
    let set = damage(&before, &after);
    assert!(!set.full);
    assert_eq!(set.damage.len(), 2);
    let old = before.group(rig.push).unwrap();
    let new = after.group(rig.push).unwrap();
    assert_eq!(new.x, old.x + 50.0);
    assert!(covers(&set, old));
    assert!(covers(&set, new));
}

#[test]
fn items_inside_a_translated_group_are_placed_in_group_space() {
    let mut rig = rig(2);
    rig.scene.update(
        rig.push,
        DisplayItem::PushTransform(Transform2D::translate(100.0, 40.0)),
    );
    let footprints = Footprints::of_scene(&rig.scene, &no_ink);
    let placed = footprints.item(rig.inner[1]).unwrap();
    assert_eq!((placed.x, placed.y), (120.0, 50.0));
}

#[test]
fn changing_one_item_in_a_group_damages_only_its_translated_rect() {
    let mut rig = rig(3);
    rig.scene.update(
        rig.push,
        DisplayItem::PushTransform(Transform2D::translate(100.0, 40.0)),
    );
    let before = Footprints::of_scene(&rig.scene, &no_ink);
    rig.scene.update(rig.inner[1], quad(25.0, 10.0));
    let after = Footprints::of_scene(&rig.scene, &no_ink);
    let set = damage(&before, &after);
    assert!(!set.full);
    assert!(covers(&set, before.item(rig.inner[1]).unwrap()));
    assert!(covers(&set, after.item(rig.inner[1]).unwrap()));
    let bounds = set.bounding_box().unwrap();
    assert!(bounds.w < 40.0, "item-level, not group-level: {bounds:?}");
}

#[test]
fn nested_group_motion_propagates_to_the_outer_extent() {
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushTransform(Transform2D::translate(
        10.0, 0.0,
    )));
    let inner = scene.insert(DisplayItem::PushTransform(Transform2D::translate(0.0, 0.0)));
    scene.insert(quad(0.0, 0.0));
    scene.insert(DisplayItem::PopTransform);
    scene.insert(DisplayItem::PopTransform);
    let before = Footprints::of_scene(&scene, &no_ink);
    scene.update(
        inner,
        DisplayItem::PushTransform(Transform2D::translate(0.0, 80.0)),
    );
    let after = Footprints::of_scene(&scene, &no_ink);
    let set = damage(&before, &after);
    assert!(!set.full);
    assert!(covers(&set, before.group(inner).unwrap()));
    assert!(covers(&set, after.group(inner).unwrap()));
    assert_eq!(after.group(inner).unwrap().y, 80.0);
}

#[test]
fn removing_a_group_damages_its_old_extent() {
    let rig = rig(2);
    let before = Footprints::of_scene(&rig.scene, &no_ink);
    let after = Footprints::of_scene(&Scene::new(), &no_ink);
    let set = damage(&before, &after);
    assert!(!set.full);
    assert!(covers(&set, before.group(rig.push).unwrap()));
}

#[test]
fn unchanged_scene_is_clean() {
    let rig = rig(4);
    let a = Footprints::of_scene(&rig.scene, &no_ink);
    let b = Footprints::of_scene(&rig.scene, &no_ink);
    assert!(damage(&a, &b).is_clean());
}

#[test]
fn unmeasured_text_change_falls_back_to_full() {
    let mut scene = Scene::new();
    let text = scene.insert(DisplayItem::Text {
        at: [0.0, 0.0],
        run: TextRun { id: 1, revision: 1 },
        color: Oklch::from_hex("#ff0000").unwrap(),
        opacity: 1.0,
    });
    let before = Footprints::of_scene(&scene, &no_ink);
    scene.update(
        text,
        DisplayItem::Text {
            at: [4.0, 0.0],
            run: TextRun { id: 1, revision: 2 },
            color: Oklch::from_hex("#ff0000").unwrap(),
            opacity: 1.0,
        },
    );
    let after = Footprints::of_scene(&scene, &no_ink);
    assert!(damage(&before, &after).full);
}
