use aqua_gpu_d3d12::policy::{
    ticks_to_duration, HeapDecision, HeapGate, GROWTH_SLACK_PERCENT, SHRINK_AFTER_FRAMES,
};
use std::time::Duration;

const ALIGN: u64 = 65536;

#[test]
fn growth_adds_slack_and_aligns() {
    let mut gate = HeapGate::new(None, ALIGN);
    let needed = 10 * ALIGN;
    let expected = (needed + needed * GROWTH_SLACK_PERCENT / 100).div_ceil(ALIGN) * ALIGN;
    assert_eq!(
        gate.decide(0, needed),
        HeapDecision::Resize { bytes: expected }
    );
}

#[test]
fn a_fitting_heap_is_kept() {
    let mut gate = HeapGate::new(None, ALIGN);
    assert_eq!(gate.decide(10 * ALIGN, 7 * ALIGN), HeapDecision::Keep);
    assert_eq!(gate.decide(10 * ALIGN, 10 * ALIGN), HeapDecision::Keep);
}

#[test]
fn the_ceiling_rejects_and_caps_slack() {
    let mut gate = HeapGate::new(Some(10 * ALIGN), ALIGN);
    assert_eq!(
        gate.decide(0, 11 * ALIGN),
        HeapDecision::Reject {
            needed: 11 * ALIGN,
            ceiling: 10 * ALIGN
        }
    );
    assert_eq!(
        gate.decide(0, 9 * ALIGN),
        HeapDecision::Resize { bytes: 10 * ALIGN }
    );
}

#[test]
fn a_mostly_empty_heap_shrinks_only_after_sustained_idleness() {
    let mut gate = HeapGate::new(None, ALIGN);
    for _ in 0..SHRINK_AFTER_FRAMES - 1 {
        assert_eq!(gate.decide(100 * ALIGN, 4 * ALIGN), HeapDecision::Keep);
    }
    assert_eq!(
        gate.decide(100 * ALIGN, 4 * ALIGN),
        HeapDecision::Resize { bytes: 4 * ALIGN }
    );
}

#[test]
fn a_busy_frame_restarts_the_shrink_clock() {
    let mut gate = HeapGate::new(None, ALIGN);
    for _ in 0..SHRINK_AFTER_FRAMES - 1 {
        gate.decide(100 * ALIGN, 4 * ALIGN);
    }
    assert_eq!(gate.decide(100 * ALIGN, 80 * ALIGN), HeapDecision::Keep);
    for _ in 0..SHRINK_AFTER_FRAMES - 1 {
        assert_eq!(gate.decide(100 * ALIGN, 4 * ALIGN), HeapDecision::Keep);
    }
}

#[test]
fn an_unused_heap_drains_to_nothing() {
    let mut gate = HeapGate::new(None, ALIGN);
    for _ in 0..SHRINK_AFTER_FRAMES - 1 {
        gate.decide(8 * ALIGN, 0);
    }
    assert_eq!(gate.decide(8 * ALIGN, 0), HeapDecision::Resize { bytes: 0 });
}

#[test]
fn ticks_convert_through_the_queue_frequency() {
    assert_eq!(
        ticks_to_duration(1_000, 26_000_000 + 1_000, 25_000_000),
        Some(Duration::from_millis(1040))
    );
    assert_eq!(ticks_to_duration(5, 4, 1000), None);
    assert_eq!(ticks_to_duration(0, 10, 0), None);
}
