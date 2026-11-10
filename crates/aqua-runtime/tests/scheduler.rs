use aqua_protocol::{FrameReason, Instant, WindowId};
use aqua_runtime::Scheduler;
use std::num::NonZeroU32;
use std::time::Duration;

#[test]
fn pulse_cadence_preserves_uncapped_and_monitor_rates_in_every_activity_state() {
    use aqua_runtime::scheduler::gpui_pulse_interval;
    let now = Instant::from_secs_f64(10.0);
    for interval in [
        Duration::ZERO,
        Duration::from_secs_f64(1.0 / 240.0),
        Duration::from_secs_f64(1.0 / 60.0),
    ] {
        for active in [false, true] {
            for last_input in [None, Some(now), Some(Instant::ZERO)] {
                assert_eq!(
                    gpui_pulse_interval(interval, active, last_input, now),
                    interval
                );
            }
        }
    }
}

#[test]
fn pulse_deadlines_do_not_consume_future_application_animation() {
    let window = WindowId::new(1).unwrap();
    let now = Instant::ZERO;
    let mut scheduler = Scheduler::new();
    scheduler.request_animation_deadline(window, now + Duration::from_millis(40));
    scheduler.request_gpui_pulse_deadline(window, now + Duration::from_millis(16));
    assert_eq!(
        scheduler.next_wakeup(now),
        Some(now + Duration::from_millis(16))
    );
    let first = now + Duration::from_millis(16);
    assert_eq!(
        scheduler.ready_windows(first),
        [(window, FrameReason::GPUI_PULSE)]
    );
    scheduler.mark_produced(&[window], first);
    scheduler.mark_presented(window);
    assert_eq!(
        scheduler.next_wakeup(first),
        Some(now + Duration::from_millis(40))
    );
    scheduler.request_gpui_pulse_deadline(window, now + Duration::from_millis(40));
    assert_eq!(
        scheduler.ready_windows(now + Duration::from_millis(40)),
        [(window, FrameReason::GPUI_PULSE | FrameReason::ANIMATION)]
    );
}

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

#[test]
fn resize_frames_wait_for_the_gate_while_other_reasons_do_not() {
    let window = WindowId::new(70).unwrap();
    let now = Instant::from_secs_f64(5.0);
    let gate = now + Duration::from_millis(30);
    let mut scheduler = Scheduler::new();
    scheduler.defer_resize_frames(window, gate);

    scheduler.request_frame(window, FrameReason::RESIZE | FrameReason::INPUT);
    assert!(scheduler.ready_windows(now).is_empty());
    assert_eq!(scheduler.next_wakeup(now), Some(gate));
    assert_eq!(scheduler.ready_windows(gate).len(), 1);

    scheduler.mark_produced(&[window], gate);
    scheduler.mark_presented(window);
    scheduler.request_frame(window, FrameReason::INPUT);
    assert_eq!(scheduler.ready_windows(now).len(), 1);
}

#[test]
fn removing_a_window_clears_its_resize_gate() {
    let window = WindowId::new(71).unwrap();
    let now = Instant::from_secs_f64(5.0);
    let mut scheduler = Scheduler::new();
    scheduler.defer_resize_frames(window, now + Duration::from_secs(1));
    scheduler.remove_window(window);
    scheduler.request_frame(window, FrameReason::RESIZE);
    assert_eq!(scheduler.ready_windows(now).len(), 1);
}
