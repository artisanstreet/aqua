use aqua_protocol::Instant;
use aqua_shell_win32::occlusion::{Level, QUIET_BEFORE_UNHOOK, Watch};
use std::time::Duration;

fn at(ms: u64) -> Instant {
    Instant::from_nanos(ms * 1_000_000)
}

#[test]
fn hooks_go_full_on_activity_and_off_after_a_quiet_period_when_visible() {
    let mut watch = Watch::default();
    assert_eq!(watch.step(at(0), true, false), Some(Level::Full));
    assert_eq!(watch.step(at(10), true, false), None);
    assert_eq!(watch.step(at(20), false, false), None);
    assert_eq!(watch.next_check(), Some(at(20) + QUIET_BEFORE_UNHOOK));
    let almost = at(20) + QUIET_BEFORE_UNHOOK - Duration::from_millis(1);
    assert_eq!(watch.step(almost, false, false), None);
    assert_eq!(
        watch.step(at(20) + QUIET_BEFORE_UNHOOK, false, false),
        Some(Level::Off)
    );
    assert_eq!(watch.next_check(), None);
    assert_eq!(watch.step(at(5000), false, false), None);
}

#[test]
fn activity_during_the_quiet_period_restarts_it() {
    let mut watch = Watch::default();
    watch.step(at(0), true, false);
    watch.step(at(100), false, false);
    assert_eq!(watch.step(at(1000), true, false), None);
    assert_eq!(watch.step(at(1100), false, false), None);
    assert_eq!(watch.next_check(), Some(at(1100) + QUIET_BEFORE_UNHOOK));
}

#[test]
fn a_covered_window_settles_to_the_narrow_set_and_wakes_for_uncovering() {
    let mut watch = Watch::default();
    assert_eq!(watch.step(at(0), true, true), Some(Level::Full));
    assert_eq!(watch.step(at(1), false, true), None);
    assert_eq!(
        watch.step(at(1) + QUIET_BEFORE_UNHOOK, false, true),
        Some(Level::Narrow)
    );
    assert_eq!(watch.next_check(), None);
    assert_eq!(watch.step(at(60_000), false, true), None);
    assert_eq!(watch.step(at(60_001), true, false), Some(Level::Full));
    assert_eq!(watch.step(at(60_002), false, false), None);
    assert_eq!(watch.next_check(), Some(at(60_002) + QUIET_BEFORE_UNHOOK));
    assert_eq!(
        watch.step(at(60_002) + QUIET_BEFORE_UNHOOK, false, false),
        Some(Level::Off)
    );
}

#[test]
fn coverage_is_refreshed_at_most_every_quarter_second_while_hooks_are_full() {
    let mut watch = Watch::default();
    assert!(!watch.refresh_due(at(0)));
    watch.step(at(0), true, false);
    assert!(watch.refresh_due(at(0)));
    assert!(!watch.refresh_due(at(100)));
    assert!(watch.refresh_due(at(250)));
    watch.step(at(2000), false, true);
    watch.step(at(2000) + QUIET_BEFORE_UNHOOK, false, true);
    assert!(!watch.refresh_due(at(9000)));
}
