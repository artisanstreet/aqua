#![cfg(target_os = "macos")]

mod support;

use std::collections::BTreeSet;

use aqua_gpu_metal::scene::{OffscreenBackend, SurfaceFormat};
use aqua_gpu_ref::conformance::{
    all_cases, requirements, run_suite, Outcome, Requirement, SuiteConfig, Tolerance,
};
use aqua_gpu_ref::RefBackend;

const EIGHT_BIT: Tolerance = Tolerance {
    interior: 6,
    soft: 16,
    edge: 64,
    edge_band: 2,
};

fn fuzz_count() -> usize {
    std::env::var("AQUA_CONFORMANCE_FUZZ")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(SuiteConfig::default().fuzz_count)
}

fn run(mut backend: OffscreenBackend, tolerance: Tolerance) {
    let info = backend.device_info().clone();
    eprintln!(
        "adapter={} families={:?} working_set={} classes={}",
        info.name,
        info.families,
        info.recommended_working_set,
        backend.metal_object_classes()
    );
    let mut reference = RefBackend::new();
    let config = SuiteConfig {
        tolerance,
        fuzz_count: fuzz_count(),
        ..SuiteConfig::default()
    };
    let custom: BTreeSet<String> = all_cases(&config)
        .into_iter()
        .filter(|case| requirements(&case.plan).contains(&Requirement::CustomPasses))
        .map(|case| case.name)
        .collect();
    let report = run_suite(&mut reference, &mut backend, &config);
    eprintln!("{report}");
    let skipped: BTreeSet<String> = report
        .cases
        .iter()
        .filter_map(|case| match &case.outcome {
            Outcome::Unsupported(reason) => Some(format!("{}: {reason}", case.name)),
            _ => None,
        })
        .collect();
    let expected: BTreeSet<String> = custom
        .iter()
        .map(|name| format!("{name}: not offered by the backend: custom passes"))
        .collect();
    assert!(!custom.is_empty(), "the suite has custom-pass cases");
    assert_eq!(
        skipped, expected,
        "only the custom-pass cases (M12) may be skipped"
    );
    assert!(report.passed(), "{}", report.failures().join("\n"));
    support::assert_clean(&backend);
}

#[test]
fn a_float_surface_matches_the_reference() {
    if let Some(backend) = support::open(SurfaceFormat::Rgba16Float) {
        run(backend, Tolerance::default());
    }
}

#[test]
fn an_eight_bit_surface_matches_the_reference() {
    if let Some(backend) = support::open(SurfaceFormat::Bgra8Unorm) {
        run(backend, EIGHT_BIT);
    }
}
