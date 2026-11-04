use std::time::Duration;

use aqua_transport::{TrimAction, TrimInput, TrimPolicy};

fn quiet() -> TrimInput {
    TrimInput {
        uploaded: false,
        cpu_time: Duration::from_millis(1),
        gpu_time: Some(Duration::from_millis(1)),
        budget: Duration::from_micros(16_667),
        requested: false,
    }
}

fn run(policy: &mut TrimPolicy, frames: u32, input: TrimInput) -> Vec<TrimAction> {
    (0..frames).map(|_| policy.observe(input)).collect()
}

#[test]
fn trims_unused_entries_once_per_interval_of_quiet_frames() {
    let mut policy = TrimPolicy::new();
    let actions = run(&mut policy, 650, quiet());
    let trims: Vec<usize> = actions
        .iter()
        .enumerate()
        .filter(|(_, action)| matches!(action, TrimAction::Unused(_)))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(trims, vec![299, 599]);
}

#[test]
fn never_trims_a_frame_that_uploaded_or_that_used_most_of_its_budget() {
    let mut policy = TrimPolicy::new();
    let uploading = TrimInput {
        uploaded: true,
        ..quiet()
    };
    assert!(run(&mut policy, 400, uploading)
        .iter()
        .all(|action| *action == TrimAction::Keep));
    let slow_gpu = TrimInput {
        gpu_time: Some(Duration::from_millis(12)),
        ..quiet()
    };
    assert!(run(&mut policy, 400, slow_gpu)
        .iter()
        .all(|action| *action == TrimAction::Keep));
    let slow_cpu = TrimInput {
        cpu_time: Duration::from_millis(12),
        gpu_time: None,
        ..quiet()
    };
    assert!(run(&mut policy, 400, slow_cpu)
        .iter()
        .all(|action| *action == TrimAction::Keep));
}

#[test]
fn a_deferred_trim_happens_on_the_first_quiet_frame_afterwards() {
    let mut policy = TrimPolicy::new();
    let uploading = TrimInput {
        uploaded: true,
        ..quiet()
    };
    run(&mut policy, 500, uploading);
    assert!(matches!(policy.observe(quiet()), TrimAction::Unused(_)));
}

#[test]
fn an_explicit_request_trims_everything_immediately_and_restarts_the_interval() {
    let mut policy = TrimPolicy::new();
    run(&mut policy, 100, quiet());
    let requested = TrimInput {
        requested: true,
        ..quiet()
    };
    assert_eq!(policy.observe(requested), TrimAction::Everything);
    let after = run(&mut policy, 299, quiet());
    assert!(after.iter().all(|action| *action == TrimAction::Keep));
    assert!(matches!(policy.observe(quiet()), TrimAction::Unused(_)));
}
