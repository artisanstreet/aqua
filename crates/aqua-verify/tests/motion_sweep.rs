use aqua_motion::{Easing, MotionKind, MotionValue, ReducedMotion, SpringParams, Track};
use aqua_verify::differential::scenes;

fn scalar(value: MotionValue) -> f32 {
    match value {
        MotionValue::Scalar(v) => v,
        other => panic!("not a scalar: {other:?}"),
    }
}

#[derive(Clone, Copy)]
struct Retarget {
    at: f64,
    to: f32,
    kind: MotionKind,
}

fn kinds() -> [MotionKind; 4] {
    [
        MotionKind::Spring(SpringParams::SMOOTH),
        MotionKind::Spring(SpringParams::SNAPPY),
        MotionKind::Spring(SpringParams::new(400.0, 10.0)),
        MotionKind::Tween {
            duration_secs: 0.2,
            easing: Easing::EaseInOut,
        },
    ]
}

fn schedule(seed: u64) -> Vec<Retarget> {
    let mut state = seed;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as f64 / (1u64 << 31) as f64
    };
    let kinds = kinds();
    let mut at = 0.0;
    (0..12)
        .map(|_| {
            at += 0.02 + next() * 0.4;
            Retarget {
                at,
                to: (next() as f32 - 0.5) * 400.0,
                kind: kinds[(next() * kinds.len() as f64) as usize % kinds.len()],
            }
        })
        .collect()
}

fn sampled(schedule: &[Retarget], times: &[f64]) -> Vec<f32> {
    let mut track = Track::new(
        MotionValue::Scalar(0.0),
        MotionKind::Instant,
        ReducedMotion::Full,
        0.0,
    );
    let mut pending = schedule.iter().peekable();
    times
        .iter()
        .map(|time| {
            while let Some(retarget) = pending.next_if(|retarget| retarget.at <= *time) {
                track
                    .retarget(MotionValue::Scalar(retarget.to), retarget.kind, retarget.at)
                    .unwrap();
            }
            scalar(track.sample(*time))
        })
        .collect()
}

fn grid(step: f64, end: f64) -> Vec<f64> {
    (0..=(end / step) as usize)
        .map(|index| index as f64 * step)
        .collect()
}

#[test]
fn presented_value_does_not_depend_on_the_sampling_rate() {
    for seed in 1..=40 {
        let plan = schedule(seed);
        let end = plan.last().unwrap().at + 3.0;
        let fine = grid(1.0 / 1440.0, end);
        let reference = sampled(&plan, &fine);
        for step in [1.0 / 30.0, 1.0 / 60.0, 1.0 / 144.0, 1.0 / 240.0] {
            let times = grid(step, end);
            let values = sampled(&plan, &times);
            for (time, value) in times.iter().zip(&values) {
                let index = (time * 1440.0).round() as usize;
                let expected = reference[index.min(reference.len() - 1)];
                assert!(
                    (value - expected).abs() <= 0.05 + expected.abs() * 1e-3,
                    "seed {seed} step {step}: t={time} got {value}, fine grid {expected}"
                );
            }
        }
    }
}

#[test]
fn retargeting_never_produces_a_one_frame_jump() {
    for seed in 1..=60 {
        let plan = schedule(seed);
        let end = plan.last().unwrap().at + 2.0;
        let times = grid(1.0 / 1000.0, end);
        let values = sampled(&plan, &times);
        let travel = plan
            .iter()
            .map(|retarget| retarget.to.abs())
            .fold(1.0, f32::max)
            * 2.0;
        let tolerance = travel * 1e-5;
        let spikes: Vec<usize> = values
            .windows(4)
            .enumerate()
            .filter(|(_, window)| {
                let before = (window[1] - window[0]).abs();
                let step = (window[2] - window[1]).abs();
                let after = (window[3] - window[2]).abs();
                step > before.max(after) * 1.5 + tolerance
            })
            .map(|(index, _)| index)
            .collect();
        assert!(
            spikes.is_empty(),
            "seed {seed}: one-step jumps at samples {spikes:?}"
        );
    }
}

#[test]
fn settling_lands_exactly_on_the_target_and_stays() {
    for seed in 1..=40 {
        let plan = schedule(seed);
        let last = *plan.last().unwrap();
        let times = grid(1.0 / 60.0, last.at + 10.0);
        let values = sampled(&plan, &times);
        let tail = &values[values.len() - 60..];
        assert!(tail.iter().all(|value| *value == last.to), "seed {seed}");
    }
}

#[test]
fn fuzz_scene_generator_is_stable_for_replays() {
    let first: Vec<String> = scenes(42, 20)
        .iter()
        .map(|s| format!("{:?}", s.commands))
        .collect();
    let second: Vec<String> = scenes(42, 20)
        .iter()
        .map(|s| format!("{:?}", s.commands))
        .collect();
    assert_eq!(first, second);
}
