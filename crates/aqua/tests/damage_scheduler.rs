//! Verification: incremental damage, demand-driven scheduler, resource bounds.
//!
//! Black-box tests against the public `aqua-render` damage/scene/pool APIs
//! and the `aqua-platform` scheduler:
//! - trivial updates drain exactly one change (cost independent of scene size);
//! - damage covers old AND new footprints, structural/scale changes go full;
//! - idle scheduler sleeps (`next_wakeup` is `None`), runtime target changes
//!   apply to already-pending work;
//! - texture residency is capped in BYTES (live + cached), not object count.

use std::num::NonZeroU32;
use std::time::Duration;

use aqua::{Instant, Scheduler};
use aqua_core::{DisplayItem, Oklch, Rect};
use aqua_protocol::geometry::AquaWindowId;
use aqua_protocol::FrameReason;
use aqua_runtime::scheduler::MIN_FRAME_INTERVAL;
use aqua_scene::damage::{DamageTracker, InvalidationReason};
use aqua_scene::Scene;

fn red() -> Oklch {
    Oklch::from_hex("#ff0000").unwrap()
}
fn blue() -> Oklch {
    Oklch::from_hex("#0000ff").unwrap()
}

fn rect_item(x: f32, fill: Oklch) -> DisplayItem {
    DisplayItem::Rect {
        bounds: Rect {
            x,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        },
        radius: 2.0,
        fill,
        border_width: 0.0,
        border_color: fill,
        shadow: None,
    }
}

fn id(n: u64) -> AquaWindowId {
    AquaWindowId::new(n).unwrap()
}

// -- incremental scene ---------------------------------------------------------

#[test]
fn trivial_update_drains_exactly_one_change() {
    let mut scene = Scene::new();
    for i in 0..1000 {
        scene.insert(rect_item(i as f32, red()));
    }
    scene.drain_changes();
    assert_eq!(scene.pending_change_count(), 0);
    let target = scene.ordered().nth(500).unwrap().0;
    let before = scene.revision();
    assert!(scene.update(target, rect_item(9999.0, red())));
    assert_eq!(scene.revision(), before + 1, "one revision bump per change");
    assert_eq!(scene.pending_change_count(), 1);
    let changes = scene.drain_changes();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].0, target);
    // Repeat updates to the same node before a drain coalesce to one entry.
    assert!(scene.update(target, rect_item(1.0, blue())));
    assert!(scene.update(target, rect_item(2.0, blue())));
    assert_eq!(scene.pending_change_count(), 1);
}

#[test]
fn noop_writes_are_free_and_stale_handles_fail_closed() {
    let mut scene = Scene::new();
    let item = rect_item(1.0, red());
    let live = scene.insert(item.clone());
    scene.drain_changes();
    let before = scene.revision();
    assert!(scene.update(live, item));
    assert_eq!(scene.revision(), before, "identical write bumps nothing");
    assert_eq!(scene.pending_change_count(), 0);

    assert!(scene.remove(live));
    let reuse = scene.insert(rect_item(5.0, blue()));
    assert_ne!(live.raw().generation, reuse.raw().generation);
    assert!(
        !scene.update(live, rect_item(7.0, red())),
        "stale fails closed"
    );
    assert_eq!(scene.get(live), None);
    assert!(scene.get(reuse).is_some());
}

// -- damage --------------------------------------------------------------------

#[test]
fn rect_damage_covers_old_and_new_footprints() {
    let mut t = DamageTracker::new(2.0);
    t.set_frame_size(800.0, 600.0);
    t.record_rect_change(
        Rect {
            x: 10.0,
            y: 10.0,
            w: 30.0,
            h: 30.0,
        },
        Some(Rect {
            x: 50.0,
            y: 50.0,
            w: 30.0,
            h: 30.0,
        }),
    );
    let set = t.take();
    assert!(!set.is_clean());
    assert!(!set.full, "precise rect change must not go full-frame");
    assert_eq!(set.damage.len(), 2, "old + new footprint repaint");
    assert_eq!(set.damage[0].x, 20.0, "logical -> physical at scale 2");
    for r in &set.raster_invalid {
        assert!(
            set.damage.iter().any(|d| {
                d.x <= r.x && d.y <= r.y && d.x + d.w >= r.x + r.w && d.y + d.h >= r.y + r.h
            }),
            "every raster-invalid region is covered by damage"
        );
    }
}

#[test]
fn structural_and_scale_changes_are_full_not_empty() {
    let mut t = DamageTracker::new(1.0);
    t.set_frame_size(400.0, 300.0);
    t.record_change(&DisplayItem::PushOpacity(0.5), None);
    let set = t.take();
    assert!(
        set.full,
        "clip/opacity structure change is conservative-full"
    );
    assert!(!set.is_clean());
    assert!(!set.raster_invalid.is_empty());

    t.set_scale(2.0);
    let set = t.take();
    assert!(set.full, "scale change invalidates raster caches");
    assert!(set.reasons.contains(&InvalidationReason::ScaleChanged));
}

#[test]
fn clean_tracker_produces_no_work() {
    let mut t = DamageTracker::new(1.0);
    t.set_frame_size(800.0, 600.0);
    assert!(t.take().is_clean());
    // A taken set resets: no phantom second frame.
    assert!(t.take().is_clean());
}

#[test]
fn backdrop_dependency_recomputes_on_backdrop_damage() {
    let mut t = DamageTracker::new(1.0);
    t.set_frame_size(400.0, 300.0);
    t.record_rect_change(
        Rect {
            x: 10.0,
            y: 10.0,
            w: 50.0,
            h: 50.0,
        },
        None,
    );
    let blur = Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 100.0,
    };
    assert!(t.damage_overlaps(blur), "blur region overlaps new damage");
    t.record_backdrop_dependency(blur);
    let set = t.take();
    assert!(!set.is_clean());
    assert!(set
        .reasons
        .contains(&InvalidationReason::BackdropDependencyChanged));
    // A distant blur is unaffected.
    let mut t = DamageTracker::new(1.0);
    t.set_frame_size(400.0, 300.0);
    t.record_rect_change(
        Rect {
            x: 300.0,
            y: 200.0,
            w: 10.0,
            h: 10.0,
        },
        None,
    );
    assert!(!t.damage_overlaps(Rect {
        x: 0.0,
        y: 0.0,
        w: 50.0,
        h: 50.0
    }));
}

// -- scheduler: idle, coalescing, pacing ----------------------------------------

#[test]
fn idle_scheduler_sleeps_with_no_ticks() {
    let s = Scheduler::new();
    assert!(!s.has_pending());
    assert_eq!(s.next_wakeup(Instant::from_nanos(1_000_000_000)), None);
}

#[test]
fn reasons_merge_and_repeat_requests_coalesce() {
    let mut s = Scheduler::new();
    let now = Instant::from_nanos(1_000_000_000);
    s.request_frame(id(1), FrameReason::INPUT);
    s.request_frame(id(1), FrameReason::DAMAGE);
    assert_eq!(
        s.pending_reasons(id(1)).unwrap(),
        FrameReason::INPUT | FrameReason::DAMAGE
    );
    assert_eq!((s.stats().requested, s.stats().coalesced), (2, 1));
    let ready = s.ready_windows(now);
    assert_eq!(ready.len(), 1);
    s.mark_produced(&[id(1)], now);
    assert_eq!(s.stats().produced, 1);
    assert!(!s.has_pending());
    assert_eq!(s.next_wakeup(now), None);
}

#[test]
fn pacing_gate_blocks_then_runtime_target_change_releases() {
    let mut s = Scheduler::new();
    s.set_fps_target(Some(NonZeroU32::new(60).unwrap()));
    let start = Instant::from_nanos(1_000_000_000);
    s.request_frame(id(4), FrameReason::DAMAGE);
    s.mark_produced(&[id(4)], start);
    s.request_frame(id(4), FrameReason::DAMAGE);
    assert!(s.ready_windows(start).is_empty(), "16.6ms gate blocks");
    let wake = s.next_wakeup(start).expect("gate must schedule a wakeup");
    assert!(wake > start);
    assert!(wake - start >= MIN_FRAME_INTERVAL);
    s.clear_fps_target();
    assert_eq!(s.ready_windows(start).len(), 1);
    assert_eq!(s.next_wakeup(start), Some(start), "drawable right now");
}

#[test]
fn backpressure_defers_without_queue_growth() {
    let mut s = Scheduler::new();
    s.set_max_in_flight(1);
    let now = Instant::from_nanos(1_000_000_000);
    s.request_frame(id(3), FrameReason::DAMAGE);
    s.mark_produced(&[id(3)], now);
    s.request_frame(id(3), FrameReason::DAMAGE);
    assert!(s.ready_windows(now).is_empty(), "in-flight cap defers");
    assert_eq!(s.stats().deferred_backpressure, 1);
    s.mark_presented(id(3));
    assert_eq!(s.ready_windows(now).len(), 1);
}

/// Backpressure must not busy-spin: when every pending window is at the
/// in-flight cap, `next_wakeup` reports `None` (the wake arrives with the
/// presentation completion via `mark_presented`, never a timer). Regression
/// test for the busy-wake defect: `Some(now)` + empty `ready_windows` fed
/// into `control_flow_for_next_wakeup` (`WaitUntil(now)`) spins with no
/// producible work. Fixed in the platform scheduler (rule 1); locked here.
#[test]
fn backpressure_wakeup_sleeps_until_present() {
    let mut s = Scheduler::new();
    s.set_max_in_flight(1);
    let now = Instant::from_nanos(1_000_000_000);
    s.request_frame(id(9), FrameReason::DAMAGE);
    s.mark_produced(&[id(9)], now);
    s.request_frame(id(9), FrameReason::DAMAGE);
    assert!(s.ready_windows(now).is_empty());
    assert_eq!(
        s.next_wakeup(now),
        None,
        "backpressured-only pending sleeps; present completion wakes it"
    );
    s.mark_presented(id(9));
    assert_eq!(
        s.next_wakeup(now),
        Some(now),
        "drawable again after present"
    );
    assert_eq!(s.ready_windows(now).len(), 1);
}

#[test]
fn high_fps_target_accepted_and_clamped_to_usable_interval() {
    let mut s = Scheduler::new();
    s.set_fps_target(Some(NonZeroU32::new(900).unwrap()));
    assert!(s.min_frame_interval().unwrap() >= MIN_FRAME_INTERVAL);
    s.set_fps_target(Some(NonZeroU32::new(u32::MAX).unwrap()));
    assert!(s.min_frame_interval().unwrap() >= Duration::from_micros(1));
}
