use aqua::render::cadence::Cadence;
use std::time::{Duration, Instant};

fn feed(cadence: &mut Cadence, start: Instant, gaps_us: &[u64], halved: bool) -> Instant {
    gaps_us.iter().fold(start, |at, gap| {
        let next = at + Duration::from_micros(*gap);
        cadence.observe(next, halved);
        next
    })
}

#[test]
fn a_fast_display_halves_only_while_gliding() {
    let mut cadence = Cadence::new();
    let now = Instant::now();
    cadence.observe(now, false);
    feed(&mut cadence, now, &[6060; 8], false);
    assert!(cadence.half_rate(true));
    assert!(!cadence.half_rate(false));
}

#[test]
fn a_sixty_hertz_display_never_halves() {
    let mut cadence = Cadence::new();
    let now = Instant::now();
    cadence.observe(now, false);
    feed(&mut cadence, now, &[16_667; 10], false);
    assert!(!cadence.half_rate(true));
}

#[test]
fn nothing_halves_before_the_period_is_known() {
    let mut cadence = Cadence::new();
    let now = Instant::now();
    cadence.observe(now, false);
    feed(&mut cadence, now, &[6060; 4], false);
    assert!(!cadence.half_rate(true));
}

#[test]
fn halved_presents_do_not_pollute_the_period() {
    let mut cadence = Cadence::new();
    let now = Instant::now();
    cadence.observe(now, false);
    let at = feed(&mut cadence, now, &[6060; 8], false);
    cadence.observe(at + Duration::from_micros(6060), true);
    feed(
        &mut cadence,
        at + Duration::from_micros(6060),
        &[12_120; 20],
        true,
    );
    assert_eq!(cadence.period(), Some(Duration::from_micros(6060)));
    assert!(cadence.half_rate(true));
}

#[test]
fn bursts_and_idle_gaps_are_ignored() {
    let mut cadence = Cadence::new();
    let now = Instant::now();
    cadence.observe(now, false);
    feed(
        &mut cadence,
        now,
        &[300, 900, 400, 500_000, 1_000_000],
        false,
    );
    assert_eq!(cadence.period(), None);
}
