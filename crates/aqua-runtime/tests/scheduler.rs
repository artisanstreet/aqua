use aqua_protocol::{FrameReason, Instant, WindowId};
use aqua_runtime::Scheduler;
use std::num::NonZeroU32;
use std::time::Duration;

#[test]
fn gpui_pulse_cadence_uses_active_state_and_invalidating_input_grace() {
    use aqua_runtime::scheduler::gpui_pulse_interval;
    let refresh = Duration::from_micros(16_667);
    let slow = Duration::from_micros(33_333);
    let start = Instant::from_secs_f64(10.0);
    assert_eq!(gpui_pulse_interval(refresh, true, None, start), refresh);
    assert_eq!(gpui_pulse_interval(refresh, false, None, start), slow);
    assert_eq!(
        gpui_pulse_interval(
            refresh,
            false,
            Some(start),
            start + Duration::from_millis(999)
        ),
        refresh
    );
    assert_eq!(
        gpui_pulse_interval(refresh, false, Some(start), start + Duration::from_secs(1)),
        slow
    );
    assert_eq!(
        gpui_pulse_interval(Duration::from_millis(40), false, None, start),
        Duration::from_millis(40)
    );
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
