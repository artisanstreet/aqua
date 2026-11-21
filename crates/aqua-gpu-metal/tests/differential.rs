#![cfg(target_os = "macos")]

mod support;

use aqua_gpu_metal::scene::SurfaceFormat;
use aqua_gpu_ref::conformance::{compare, Session, Tolerance};
use aqua_gpu_ref::RefBackend;
use aqua_verify::differential::{scenes, variant_case, VARIANTS};

const SEED: u64 = 0x7E1C_A9D1;

fn count() -> usize {
    std::env::var("AQUA_DIFFERENTIAL_FUZZ")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(400)
}

#[test]
fn metal_matches_the_oracle_on_every_plan_variant() {
    let Some(mut device) = support::open(SurfaceFormat::Rgba16Float) else {
        return;
    };
    let adapter = device.device_info().name.clone();
    let mut reference = RefBackend::new();
    let mut expected_session = Session::new(&mut reference);
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
                failures.push(format!(
                    "{}: interior {}, soft {}, edge {}",
                    case.name, comparison.interior_max, comparison.soft_max, comparison.edge_max
                ));
            }
        }
    }
    eprintln!(
        "differential metal ({adapter}): {runs} runs, worst interior {} soft {} edge {}",
        worst.0, worst.1, worst.2
    );
    let shown = failures.len().min(25);
    assert!(
        failures.is_empty(),
        "{} failures, first {shown}:\n{}",
        failures.len(),
        failures[..shown].join("\n")
    );
    support::assert_clean(&device);
}
