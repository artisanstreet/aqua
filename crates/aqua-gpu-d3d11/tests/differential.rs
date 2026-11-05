#![cfg(windows)]

mod support;

use aqua_gpu_d3d11::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat};
use aqua_gpu_ref::RefBackend;
use aqua_gpu_ref::conformance::{Comparison, Session, Tolerance, compare};
use aqua_verify::differential::{VARIANTS, scenes, variant_case};

const SEED: u64 = 0xD3D1_2D1F;

fn count() -> usize {
    std::env::var("AQUA_DIFFERENTIAL_FUZZ")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(400)
}

fn backend(preference: AdapterPreference) -> OffscreenBackend {
    if preference == AdapterPreference::Warp {
        support::hold_software();
    }
    OffscreenBackend::with_format(preference, SurfaceFormat::Rgba16Float)
        .unwrap_or_else(|error| panic!("no adapter {preference:?}: {error}"))
}

fn summary(name: &str, comparison: &Comparison) -> String {
    format!(
        "{name}: interior {}, soft {}, edge {}",
        comparison.interior_max, comparison.soft_max, comparison.edge_max
    )
}

fn assert_none(failures: Vec<String>) {
    let shown = failures.len().min(25);
    assert!(
        failures.is_empty(),
        "{} failures, first {shown}:\n{}",
        failures.len(),
        failures[..shown].join("\n")
    );
}

fn against_reference(preference: AdapterPreference) {
    let mut reference = RefBackend::new();
    let mut expected_session = Session::new(&mut reference);
    let mut device = backend(preference);
    let adapter = device.device_info().adapter.name.clone();
    let mut actual_session = Session::new(&mut device);
    let mut failures = Vec::new();
    let mut worst = (0u8, 0u8, 0u8);
    let mut runs = 0;
    for scene in scenes(SEED, count()) {
        for variant in VARIANTS {
            let case = variant_case(&scene, variant).unwrap();
            let expected = expected_session.render_case(&case).unwrap();
            let actual = match actual_session.render_case(&case) {
                Ok(actual) => actual,
                Err(error) => {
                    failures.push(format!("{}: {error}", case.name));
                    continue;
                }
            };
            let comparison = compare(&case.plan, &expected, &actual, Tolerance::default());
            worst = (
                worst.0.max(comparison.interior_max),
                worst.1.max(comparison.soft_max),
                worst.2.max(comparison.edge_max),
            );
            runs += 1;
            if !comparison.passed() {
                failures.push(summary(&case.name, &comparison));
            }
        }
    }
    eprintln!(
        "differential {preference:?} ({adapter}): {runs} runs, worst interior {} soft {} edge {}",
        worst.0, worst.1, worst.2
    );
    assert_none(failures);
}

fn agree(first: AdapterPreference, second: AdapterPreference, seed: u64, variant: usize) {
    let mut a = backend(first);
    let mut b = backend(second);
    let mut a_session = Session::new(&mut a);
    let mut b_session = Session::new(&mut b);
    let failures = scenes(seed, count())
        .iter()
        .filter_map(|scene| {
            let case = variant_case(scene, VARIANTS[variant]).unwrap();
            let left = a_session.render_case(&case).unwrap();
            let right = b_session.render_case(&case).unwrap();
            let comparison = compare(&case.plan, &left, &right, Tolerance::default());
            (!comparison.passed()).then(|| summary(&case.name, &comparison))
        })
        .collect();
    assert_none(failures);
}

#[test]
fn warp_matches_the_oracle_on_every_plan_variant() {
    against_reference(AdapterPreference::Warp);
}

#[test]
fn hardware_matches_the_oracle_on_every_plan_variant() {
    against_reference(AdapterPreference::HighPerformance);
}

#[test]
fn integrated_matches_the_oracle_on_every_plan_variant() {
    against_reference(support::integrated());
}

#[test]
fn hardware_and_warp_agree_with_each_other() {
    agree(
        AdapterPreference::HighPerformance,
        AdapterPreference::Warp,
        SEED ^ 7,
        0,
    );
}

#[test]
fn integrated_and_hardware_agree_with_each_other() {
    agree(
        support::integrated(),
        AdapterPreference::HighPerformance,
        SEED ^ 11,
        1,
    );
}
