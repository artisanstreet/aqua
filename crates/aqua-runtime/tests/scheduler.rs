use aqua_protocol::{FrameReason, Instant, WindowId};
use aqua_runtime::Scheduler;
use std::num::NonZeroU32;
use std::time::Duration;

#[test]
fn live_window_rate_changes_release_only_their_own_pacing_gate() {
    let first = WindowId::new(1).unwrap();
    let second = WindowId::new(2).unwrap();
    let mut scheduler = Scheduler::new();
    scheduler.set_fps_target(NonZeroU32::new(120));
    scheduler.set_window_fps_target(first, NonZeroU32::new(60));
    let now = Instant::from_nanos(0);
    for id in [first, second] {
        scheduler.request_frame(id, FrameReason::INPUT);
    }
    scheduler.mark_produced(&[first, second], now);
    for id in [first, second] {
        scheduler.mark_presented(id);
        scheduler.request_frame(id, FrameReason::INPUT);
    }
    let after_10ms = now + Duration::from_millis(10);
    assert_eq!(
        scheduler.ready_windows(after_10ms),
        [(second, FrameReason::INPUT)]
    );
    scheduler.set_window_fps_target(first, None);
    assert_eq!(
        scheduler.ready_windows(after_10ms),
        [(first, FrameReason::INPUT), (second, FrameReason::INPUT)]
    );
    scheduler.mark_produced(&[first, second], after_10ms);
    for id in [first, second] {
        scheduler.mark_presented(id);
        scheduler.request_frame(id, FrameReason::INPUT);
    }
    assert_eq!(
        scheduler.ready_windows(after_10ms),
        [(first, FrameReason::INPUT)]
    );
    scheduler.remove_window(first);
    scheduler.request_frame(first, FrameReason::INPUT);
    scheduler.mark_produced(&[first], after_10ms);
    scheduler.mark_presented(first);
    scheduler.request_frame(first, FrameReason::INPUT);
    assert!(scheduler.ready_windows(after_10ms).is_empty());
}
