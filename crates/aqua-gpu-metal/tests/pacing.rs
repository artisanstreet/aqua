use std::time::{Duration, Instant};

use aqua_gpu_metal::window::pacing::{
    presentation, FrameSlot, FrameWait, Pacer, Presentation, Screen, Slot, SurfacePacer, Wait,
};
use aqua_protocol::PresentHint;

const INTERVAL: Duration = Duration::from_micros(16_667);

fn shown(at_micros: u64) -> Presentation {
    Presentation::Shown {
        at: Some(aqua_protocol::Instant::ZERO + Duration::from_micros(at_micros)),
        refresh: Some(INTERVAL),
    }
}

#[test]
fn a_fresh_surface_renders_without_waiting() {
    let slot = FrameSlot::default();
    assert_eq!(
        slot.plan(Instant::now(), PresentHint::Vsync, INTERVAL),
        Wait::None
    );
}

#[test]
fn after_a_present_vsync_waits_at_most_two_intervals_for_the_frame_callback() {
    let start = Instant::now();
    let mut slot = FrameSlot::default();
    slot.presented(start);
    assert_eq!(
        slot.plan(start, PresentHint::Vsync, INTERVAL),
        Wait::For(INTERVAL * 2)
    );
    assert_eq!(
        slot.plan(start + INTERVAL, PresentHint::Vsync, INTERVAL),
        Wait::For(INTERVAL)
    );
    assert_eq!(
        slot.plan(start + INTERVAL * 3, PresentHint::Vsync, INTERVAL),
        Wait::For(Duration::ZERO)
    );
}

#[test]
fn an_immediate_present_only_polls_for_the_frame_callback() {
    let start = Instant::now();
    let mut slot = FrameSlot::default();
    slot.presented(start);
    assert_eq!(
        slot.plan(start, PresentHint::Immediate, INTERVAL),
        Wait::Poll
    );
    slot.resolve(start, FrameWait::TimedOut, INTERVAL);
    assert!(matches!(slot.slot(), Slot::Pending { .. }));
}

#[test]
fn a_callback_that_misses_the_bound_starves_the_surface_until_one_arrives() {
    let start = Instant::now();
    let mut slot = FrameSlot::default();
    slot.presented(start);
    slot.resolve(start + INTERVAL, FrameWait::TimedOut, INTERVAL);
    assert!(
        matches!(slot.slot(), Slot::Pending { .. }),
        "an early timeout is not starvation"
    );
    slot.resolve(start + INTERVAL * 2, FrameWait::TimedOut, INTERVAL);
    assert_eq!(slot.slot(), Slot::Starved);
    for frame in 3..10 {
        let now = start + INTERVAL * frame;
        assert_eq!(
            slot.plan(now, PresentHint::Vsync, INTERVAL),
            Wait::Poll,
            "a starved surface never waits again"
        );
        slot.presented(now);
        slot.resolve(now, FrameWait::TimedOut, INTERVAL);
        assert_eq!(slot.slot(), Slot::Starved);
    }
    slot.resolve(start + INTERVAL * 11, FrameWait::Ready, INTERVAL);
    assert_eq!(slot.slot(), Slot::Free);
}

#[test]
fn presentation_feedback_sets_the_interval_and_counts_missed_vblanks() {
    let mut pacer = Pacer::default();
    assert_eq!(pacer.interval(None), Duration::from_nanos(16_666_667));
    assert_eq!(
        pacer.interval(Some(Duration::from_millis(8))),
        Duration::from_millis(8)
    );
    [0, 16_667, 33_334, 83_335]
        .into_iter()
        .for_each(|at| pacer.observe(shown(at)));
    pacer.observe(Presentation::Discarded);
    let report = pacer.report();
    assert_eq!(report.interval, INTERVAL);
    assert_eq!(report.frames, 4);
    assert_eq!(report.discarded, 1);
    assert_eq!(report.missed, 2, "the 50 ms gap skipped two vblanks");
    assert_eq!(report.worst, Duration::from_micros(50_001));
}

struct Clock;

impl SurfacePacer for Clock {
    fn refresh(&self) -> Option<Duration> {
        Some(INTERVAL)
    }

    fn covered(&self) -> bool {
        false
    }

    fn presented_at(&self, media_seconds: f64) -> aqua_protocol::Instant {
        aqua_protocol::Instant::from_nanos((media_seconds * 1e9).round() as u64)
    }
}

#[test]
fn a_drawable_with_no_presented_time_counts_as_discarded() {
    let mut pacer = Pacer::default();
    [1.0, 1.016_667, 0.0, 1.033_334]
        .into_iter()
        .for_each(|seconds| pacer.observe(presentation(seconds, &Clock)));
    let report = pacer.report();
    assert_eq!(report.frames, 3);
    assert_eq!(report.discarded, 1);
    assert_eq!(
        report.missed, 0,
        "a discarded drawable is not a missed vblank"
    );
    assert_eq!(
        report.last_shown,
        Some(aqua_protocol::Instant::from_nanos(1_033_334_000))
    );
}

#[test]
fn a_frame_dropped_behind_an_in_flight_present_is_due_when_its_drawable_returns() {
    let start = Instant::now();
    let mut slot = FrameSlot::default();
    slot.presented(start);
    let owed = |slot: &FrameSlot, outstanding, now| {
        slot.owed_frame_in(Screen::Behind, outstanding, false, now, INTERVAL)
    };
    assert_eq!(
        owed(&slot, 1, start),
        Some(INTERVAL * 2),
        "with the present still in flight the owed frame waits at most for starvation"
    );
    assert_eq!(owed(&slot, 1, start + INTERVAL), Some(INTERVAL));
    assert_eq!(
        owed(&slot, 0, start + INTERVAL),
        Some(Duration::ZERO),
        "the returned drawable makes it due at once"
    );
    slot.resolve(start + INTERVAL * 2, FrameWait::TimedOut, INTERVAL);
    assert_eq!(slot.slot(), Slot::Starved);
    assert_eq!(
        owed(&slot, 1, start + INTERVAL * 2),
        Some(Duration::ZERO),
        "a starved slot may draw the owed frame without the drawable"
    );
}

#[test]
fn a_screen_showing_the_latest_frame_or_covered_owes_no_redraw() {
    let start = Instant::now();
    let mut slot = FrameSlot::default();
    slot.presented(start);
    assert_eq!(
        slot.owed_frame_in(Screen::Current, 0, false, start, INTERVAL),
        None
    );
    assert_eq!(
        slot.owed_frame_in(Screen::Behind, 0, true, start, INTERVAL),
        None,
        "a covered window redraws when it is uncovered, not before"
    );
}
