use aqua::debug::history::{History, CAPACITY};

#[test]
fn fps_counts_a_full_second_beyond_the_graph_capacity() {
    let mut history = History::default();
    for frame in 0..2000 {
        history.present(frame * 500_000);
    }
    assert_eq!(
        history.summary(1_000_000_000, None).presents_per_second,
        2000
    );
    assert_eq!(history.intervals().len(), CAPACITY - 1);
    history.present(2_000_000_000);
    assert_eq!(history.summary(2_000_000_000, None).presents_per_second, 1);
    assert_eq!(history.summary(3_000_000_001, None).presents_per_second, 0);
}

#[test]
fn ui_update_rate_is_separate_from_repeated_gpu_submissions() {
    use aqua::debug::history::{Sample, Thread};
    let mut history = History::default();
    for frame in 0..2000 {
        let at = frame * 500_000;
        history.present(at);
        if frame % 4 == 0 {
            history.record(Sample {
                thread: Thread::App,
                window: 1,
                at,
                wall: 1,
                phases: Default::default(),
                metrics: Default::default(),
                allocs: 0,
                bytes: 0,
                notes: Vec::new(),
            });
        }
    }
    let summary = history.summary(1_000_000_000, None);
    assert_eq!(summary.app_updates_per_second, 500);
    assert_eq!(summary.presents_per_second, 2000);
    assert_eq!(
        history.summary(2_000_000_000, None).app_updates_per_second,
        0
    );
}
