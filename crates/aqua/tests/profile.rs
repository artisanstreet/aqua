use std::time::Duration;

use aqua::profile::recorder::{Recorder, Role};
use aqua::profile::report::{spike_line, Config};
use aqua_protocol::probe::{Event, Metric, Phase};

fn busy(millis: u64) {
    std::thread::sleep(Duration::from_millis(millis));
}

#[test]
fn nested_phases_are_charged_exclusively() {
    let mut recorder = Recorder::new();
    recorder.begin();
    recorder.event(Event::Enter(Phase::Build));
    busy(4);
    recorder.event(Event::Enter(Phase::Layout));
    busy(12);
    recorder.event(Event::Exit(Phase::Layout));
    busy(4);
    recorder.event(Event::Exit(Phase::Build));
    let frame = recorder.finish(Role::App, 7);
    let build = frame.nanos[Phase::Build.index()];
    let layout = frame.nanos[Phase::Layout.index()];
    assert!(layout >= 12_000_000, "layout {layout}");
    assert!(
        build >= 8_000_000 && build < layout + 8_000_000,
        "build {build}"
    );
    assert!(frame.work_nanos() >= build + layout);
}

#[test]
fn wait_is_not_work_and_counters_reset_per_frame() {
    let mut recorder = Recorder::new();
    recorder.event(Event::Enter(Phase::Wait));
    busy(6);
    recorder.event(Event::Exit(Phase::Wait));
    recorder.event(Event::Count(Metric::UploadBytes, 4096));
    recorder.event(Event::Count(Metric::UploadBytes, 4096));
    let first = recorder.finish(Role::Render, 1);
    assert_eq!(first.work_nanos(), 0);
    assert!(first.nanos[Phase::Wait.index()] >= 6_000_000);
    assert_eq!(first.metrics[Metric::UploadBytes.index()], 8192);
    let second = recorder.finish(Role::Render, 1);
    assert_eq!(second.metrics[Metric::UploadBytes.index()], 0);
    assert_eq!(second.nanos[Phase::Wait.index()], 0);
}

#[test]
fn spike_line_names_the_dominant_phase_and_the_counts() {
    let mut recorder = Recorder::new();
    recorder.begin();
    recorder.event(Event::Enter(Phase::Text));
    busy(10);
    recorder.event(Event::Exit(Phase::Text));
    recorder.event(Event::Enter(Phase::Plan));
    busy(1);
    recorder.event(Event::Exit(Phase::Plan));
    recorder.event(Event::Count(Metric::TextRunsShaped, 42));
    recorder.note("rebuild_reason=INPUT".to_string());
    let frame = recorder.finish(Role::App, 3);
    let config = Config::from_environment();
    let line = spike_line(&config, &frame, 9);
    assert!(line.contains("dominant=text"), "{line}");
    assert!(line.contains("text_runs_shaped:42"), "{line}");
    assert!(line.contains("rebuild_reason=INPUT"), "{line}");
    assert!(line.contains("role=app label=3 seq=9"), "{line}");
}

#[cfg(feature = "profile-alloc")]
#[test]
fn heap_counters_are_charged_to_the_phase_that_allocated() {
    let mut recorder = Recorder::new();
    recorder.begin();
    recorder.event(Event::Enter(Phase::Lower));
    let kept: Vec<Vec<u8>> = (0..50).map(|_| vec![0u8; 1024]).collect();
    std::hint::black_box(&kept);
    recorder.event(Event::Exit(Phase::Lower));
    recorder.event(Event::Enter(Phase::Plan));
    recorder.event(Event::Exit(Phase::Plan));
    let frame = recorder.finish(Role::Render, 1);
    let lower = frame.heap[Phase::Lower.index()];
    assert!(lower.allocs >= 50, "{lower:?}");
    assert!(lower.bytes >= 50 * 1024, "{lower:?}");
    assert!(frame.heap[Phase::Plan.index()].allocs < 5);
}
