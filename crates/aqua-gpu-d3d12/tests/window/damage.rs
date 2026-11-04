use aqua_core::geometry::Rect;
use aqua_gpu_d3d12::window::damage::{Damage, DamageTracker, PixelRect};
use aqua_graph::Extent;

const EXTENT: Extent = Extent {
    width: 100,
    height: 80,
};

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

fn pixels(left: i32, top: i32, right: i32, bottom: i32) -> PixelRect {
    PixelRect {
        left,
        top,
        right,
        bottom,
    }
}

#[test]
fn rects_snap_outward_and_clamp_to_the_surface() {
    let damage = Damage::from_rects(&[rect(10.4, 5.6, 20.2, 10.0), rect(-5.0, 70.0, 20.0, 30.0)], EXTENT);
    assert_eq!(
        damage,
        Damage::Rects(vec![pixels(10, 5, 31, 16), pixels(0, 70, 15, 80)])
    );
}

#[test]
fn empty_input_and_full_coverage_mean_full_repaint() {
    assert_eq!(Damage::from_rects(&[], EXTENT), Damage::Full);
    assert_eq!(
        Damage::from_rects(&[rect(-4.0, -4.0, 200.0, 200.0)], EXTENT),
        Damage::Full
    );
}

#[test]
fn rects_outside_the_surface_are_dropped() {
    let damage = Damage::from_rects(&[rect(150.0, 10.0, 5.0, 5.0)], EXTENT);
    assert!(damage.is_none());
}

#[test]
fn too_many_rects_collapse_into_their_bounds() {
    let many: Vec<Rect> = (0..40).map(|i| rect(i as f32 * 2.0, 0.0, 1.0, 1.0)).collect();
    assert_eq!(
        Damage::from_rects(&many, EXTENT),
        Damage::Rects(vec![pixels(0, 0, 79, 1)])
    );
}

#[test]
fn unknown_buffers_repaint_everything() {
    let tracker = DamageTracker::new(2);
    assert_eq!(tracker.age(0), 0);
    assert_eq!(tracker.repaint(0, &Damage::none()), Damage::Full);
}

#[test]
fn a_buffer_repaints_what_the_other_buffers_changed_since() {
    let mut tracker = DamageTracker::new(2);
    let a = Damage::Rects(vec![pixels(0, 0, 10, 10)]);
    let b = Damage::Rects(vec![pixels(50, 50, 60, 60)]);
    let c = Damage::Rects(vec![pixels(20, 20, 30, 30)]);
    tracker.commit(0, Damage::Full);
    tracker.commit(1, Damage::Full);
    tracker.commit(0, a.clone());
    tracker.commit(1, b.clone());
    assert_eq!(tracker.age(0), 2);
    assert_eq!(tracker.age(1), 1);
    assert_eq!(
        tracker.repaint(0, &c),
        Damage::Rects(vec![pixels(20, 20, 30, 30), pixels(50, 50, 60, 60)])
    );
    assert_eq!(tracker.repaint(1, &c), c);
}

#[test]
fn reset_forgets_every_buffer() {
    let mut tracker = DamageTracker::new(2);
    tracker.commit(0, Damage::none());
    tracker.commit(1, Damage::none());
    tracker.reset();
    assert_eq!(tracker.repaint(0, &Damage::none()), Damage::Full);
    assert_eq!(tracker.age(1), 0);
}

#[test]
fn history_that_is_too_old_falls_back_to_full() {
    let mut tracker = DamageTracker::new(2);
    tracker.commit(0, Damage::none());
    for _ in 0..20 {
        tracker.commit(1, Damage::none());
    }
    assert_eq!(tracker.repaint(0, &Damage::none()), Damage::Full);
}

#[test]
fn full_damage_anywhere_in_the_window_forces_full() {
    let mut tracker = DamageTracker::new(2);
    tracker.commit(0, Damage::none());
    tracker.commit(1, Damage::Full);
    assert_eq!(tracker.repaint(0, &Damage::none()), Damage::Full);
}
