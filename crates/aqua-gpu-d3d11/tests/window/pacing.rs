use std::time::{Duration, Instant};

use aqua_gpu_d3d11::window::pacing::{
    interval_from_ticks, reconcile_interval, ticks_until_next_vblank, vblanks_spanned, Pacer,
    VblankGuard,
};

const FREQUENCY: u64 = 10_000_000;

fn ms(value: f64) -> Duration {
    Duration::from_secs_f64(value / 1000.0)
}

#[test]
fn sparse_fast_vblank_waits_do_not_disable_the_clock() {
    let mut guard = VblankGuard::default();
    let epoch = Instant::now();
    for index in 0..150 {
        let started = epoch + ms(index as f64 * 110.0);
        assert!(guard.observe(started, started + ms(0.01), ms(6.06)));
    }
}

#[test]
fn three_rapid_nonblocking_vblank_waits_disable_the_clock() {
    let mut guard = VblankGuard::default();
    let epoch = Instant::now();
    assert!(guard.observe(epoch, epoch + ms(0.01), ms(6.06)));
    assert!(guard.observe(epoch + ms(0.5), epoch + ms(0.51), ms(6.06)));
    assert!(!guard.observe(epoch + ms(1.0), epoch + ms(1.01), ms(6.06)));
}

#[test]
fn a_blocking_vblank_wait_resets_the_fast_wait_streak() {
    let mut guard = VblankGuard::default();
    let epoch = Instant::now();
    assert!(guard.observe(epoch, epoch + ms(0.01), ms(6.06)));
    assert!(guard.observe(epoch + ms(0.5), epoch + ms(0.51), ms(6.06)));
    assert!(guard.observe(epoch + ms(1.0), epoch + ms(4.0), ms(6.06)));
    assert!(guard.observe(epoch + ms(4.1), epoch + ms(4.11), ms(6.06)));
    assert!(guard.observe(epoch + ms(4.2), epoch + ms(4.21), ms(6.06)));
    assert!(!guard.observe(epoch + ms(4.3), epoch + ms(4.31), ms(6.06)));
}

#[test]
fn qpc_period_converts_to_the_real_refresh_interval() {
    let hz_59_94 = interval_from_ticks(166_833, FREQUENCY);
    assert!((hz_59_94.as_secs_f64() * 1000.0 - 16.6833).abs() < 0.001);
    let hz_144 = interval_from_ticks(69_444, FREQUENCY);
    assert!((hz_144.as_secs_f64() * 1000.0 - 6.9444).abs() < 0.001);
}

#[test]
fn unknown_periods_do_not_invent_a_refresh_rate() {
    assert_eq!(interval_from_ticks(0, FREQUENCY), Duration::ZERO);
    assert_eq!(interval_from_ticks(166_833, 0), Duration::ZERO);
    assert_eq!(interval_from_ticks(u64::MAX, 1), Duration::ZERO);
}

#[test]
fn high_refresh_displays_keep_their_actual_interval() {
    for hz in [144, 240, 360, 500, 1000] {
        let period = FREQUENCY / hz;
        let interval = interval_from_ticks(period, FREQUENCY);
        assert_eq!(interval, Duration::from_nanos(period * 100));
        assert_eq!(
            reconcile_interval(Duration::ZERO, hz as u32),
            Duration::from_secs_f64(1.0 / hz as f64)
        );
    }
}

#[test]
fn next_vblank_is_the_remainder_of_the_current_period() {
    assert_eq!(ticks_until_next_vblank(100, 150, 60), 50);
    assert_eq!(ticks_until_next_vblank(150, 150, 60), 0);
    assert_eq!(ticks_until_next_vblank(151, 150, 60), 59);
    assert_eq!(ticks_until_next_vblank(150 + 60 * 7 + 20, 150, 60), 40);
    assert_eq!(ticks_until_next_vblank(5, 150, 0), 0);
}

#[test]
fn spanned_vblanks_tolerate_scheduling_jitter() {
    let interval = ms(16.667);
    assert_eq!(vblanks_spanned(ms(16.7), interval), 1);
    assert_eq!(vblanks_spanned(ms(15.9), interval), 1);
    assert_eq!(vblanks_spanned(ms(33.4), interval), 2);
    assert_eq!(vblanks_spanned(ms(50.0), interval), 3);
    assert_eq!(vblanks_spanned(ms(2.0), interval), 0);
    assert_eq!(vblanks_spanned(ms(10.0), Duration::ZERO), 0);
}

#[test]
fn pacer_counts_only_missed_vblanks() {
    let mut pacer = Pacer::new(ms(16.667));
    for _ in 0..10 {
        pacer.observe(ms(16.7));
    }
    pacer.observe(ms(33.4));
    pacer.observe(ms(50.1));
    let report = pacer.report();
    assert_eq!(report.frames, 12);
    assert_eq!(report.missed, 3);
    assert_eq!(report.worst, ms(50.1));
    assert!(report.mean > ms(16.0) && report.mean < ms(22.0));
}

#[test]
fn changing_the_interval_restarts_the_window_but_keeps_totals() {
    let mut pacer = Pacer::new(ms(16.667));
    pacer.observe(ms(16.7));
    pacer.set_interval(ms(6.944));
    assert_eq!(pacer.report().mean, Duration::ZERO);
    pacer.observe(ms(7.0));
    pacer.observe(ms(14.0));
    let report = pacer.report();
    assert_eq!(report.frames, 3);
    assert_eq!(report.missed, 1);
    assert_eq!(report.interval, ms(6.944));
}

#[test]
fn the_composition_period_is_trusted_only_when_it_matches_the_window_monitor() {
    let dwm_240 = ms(4.1667);
    assert_eq!(reconcile_interval(dwm_240, 240), dwm_240);
    let dwm_5994 = ms(16.6833);
    assert_eq!(reconcile_interval(dwm_5994, 60), dwm_5994);
    let mismatched = reconcile_interval(dwm_240, 165);
    assert!((mismatched.as_secs_f64() * 1000.0 - 6.0606).abs() < 0.001);
    assert_eq!(reconcile_interval(dwm_240, 0), dwm_240);
}
