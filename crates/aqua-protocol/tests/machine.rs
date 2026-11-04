use aqua_protocol::{
    Feedback, GpuEvent, Instant, Machine, Recorder, RuntimeOutput, first_divergence, replay,
};

struct Accumulator {
    total: u64,
}

impl Machine for Accumulator {
    type Input = u64;
    type Output = u64;

    fn step(&mut self, input: u64) -> u64 {
        self.total = self.total.wrapping_mul(31).wrapping_add(input);
        self.total
    }
}

#[test]
fn a_recorded_log_replays_to_identical_outputs() {
    let mut recorder = Recorder::new(Accumulator { total: 7 });
    let inputs = [3, 9, 27, 81];
    let live = replay(&mut recorder, inputs);
    let log = recorder.into_log();
    let mut fresh = Accumulator { total: 7 };
    let replayed = replay(&mut fresh, log.iter().map(|entry| entry.input));
    assert_eq!(replayed, live);
    assert_eq!(first_divergence(&log, &replayed), None);
}

#[test]
fn replay_against_a_different_machine_reports_the_first_divergent_step() {
    let mut recorder = Recorder::new(Accumulator { total: 7 });
    replay(&mut recorder, [3, 9, 27]);
    let log = recorder.into_log();
    let mut drifted = Accumulator { total: 7 };
    let mut replayed = vec![drifted.step(3), drifted.step(10), drifted.step(27)];
    assert_eq!(first_divergence(&log, &replayed), Some(1));
    replayed.truncate(1);
    assert_eq!(first_divergence(&log, &replayed), Some(1));
}

#[test]
fn merged_outputs_keep_the_earliest_wake() {
    let mut first = RuntimeOutput {
        wake_at: Some(Instant::from_secs_f64(2.0)),
        ..Default::default()
    };
    let second = RuntimeOutput {
        wake_at: Some(Instant::from_secs_f64(1.0)),
        ..Default::default()
    };
    first.merge(second);
    assert_eq!(first.wake_at, Some(Instant::from_secs_f64(1.0)));
    assert!(first.is_idle());
}

#[test]
fn only_presentation_and_device_events_reach_the_render_side() {
    assert_eq!(
        GpuEvent::Presented(4).feedback(),
        Some(Feedback::Presented(4))
    );
    assert_eq!(GpuEvent::DeviceLost.feedback(), Some(Feedback::DeviceLost));
    assert_eq!(
        GpuEvent::DeviceRecovered.feedback(),
        Some(Feedback::DeviceRecovered)
    );
}
