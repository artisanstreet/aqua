use aqua_protocol::Instant;
use aqua_shell_win32::timing::*;
use std::time::Duration;

fn at(ms: u64) -> Instant {
    Instant::from_nanos(ms * 1_000_000)
}

#[test]
fn timer_due_time_is_negative_relative_and_never_zero() {
    assert_eq!(relative_due_time_100ns(at(10), at(11)), -10_000);
    assert_eq!(relative_due_time_100ns(at(10), at(10)), -1);
    assert_eq!(relative_due_time_100ns(at(10), at(5)), -1);
}

#[test]
fn timeouts_round_up_to_whole_milliseconds() {
    assert_eq!(timeout_ms(at(0), None), u32::MAX);
    assert_eq!(
        timeout_ms(at(0), Some(at(0) + Duration::from_micros(1500))),
        2
    );
    assert_eq!(timeout_ms(at(5), Some(at(3))), 0);
    assert!(due(at(5), Some(at(5))));
    assert!(!due(at(4), Some(at(5))));
    assert!(!due(at(4), None));
}

#[test]
fn near_deadlines_stay_precise_and_far_ones_get_bounded_leeway() {
    assert_eq!(plan(at(0), at(16)), TimerPlan::Precise);
    assert_eq!(plan(at(0), at(100)), TimerPlan::Precise);
    assert_eq!(plan(at(0), at(500)), TimerPlan::Coarse { leeway_ms: 50 });
    assert_eq!(
        plan(at(0), at(60_000)),
        TimerPlan::Coarse { leeway_ms: 250 }
    );
    assert_eq!(plan(at(9), at(3)), TimerPlan::Precise);
}
