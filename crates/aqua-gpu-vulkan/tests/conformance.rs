#![cfg(target_os = "linux")]

mod support;

use aqua_gpu_ref::conformance::{run_suite, Outcome, SuiteConfig, Tolerance};
use aqua_gpu_ref::RefBackend;
use aqua_gpu_vulkan::hal::Binding;
use aqua_gpu_vulkan::scene::{
    AdapterPreference, Descriptors, OffscreenBackend, OffscreenOptions, SurfaceFormat,
};

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
        "adapter={} kind={:?} binding={:?} budget={}",
        info.name, info.kind, info.binding, info.memory_budget
    );
    let mut reference = RefBackend::new();
    let config = SuiteConfig {
        tolerance,
        fuzz_count: fuzz_count(),
        ..SuiteConfig::default()
    };
    let report = run_suite(&mut reference, &mut backend, &config);
    eprintln!("{report}");
    let skipped: Vec<String> = report
        .cases
        .iter()
        .filter_map(|case| match &case.outcome {
            Outcome::Unsupported(reason) => Some(format!("{}: {reason}", case.name)),
            _ => None,
        })
        .collect();
    assert!(
        skipped.is_empty(),
        "no case may be skipped:\n{}",
        skipped.join("\n")
    );
    assert!(report.passed(), "{}", report.failures().join("\n"));
    support::assert_clean(&backend);
}

#[test]
fn software_float_surface_matches_reference() {
    if let Some(backend) = support::open(AdapterPreference::Software, SurfaceFormat::Rgba16Float)
        .and_then(support::compiling)
    {
        run(backend, Tolerance::default());
    }
}

#[test]
fn software_eight_bit_surface_matches_reference() {
    if let Some(backend) = support::open(AdapterPreference::Software, SurfaceFormat::Bgra8Unorm)
        .and_then(support::compiling)
    {
        run(backend, EIGHT_BIT);
    }
}

#[test]
fn without_push_descriptors_the_pool_path_matches_reference() {
    let options = OffscreenOptions {
        device: support::options(AdapterPreference::Software, Descriptors::Pool),
        format: SurfaceFormat::Rgba16Float,
        ..OffscreenOptions::default()
    };
    if let Some(backend) = support::open_with(options).and_then(support::compiling) {
        assert_eq!(backend.device_info().binding, Binding::Pool);
        run(backend, Tolerance::default());
    }
}

#[test]
fn hardware_float_surface_matches_reference() {
    if let Some(backend) = support::open(AdapterPreference::Hardware, SurfaceFormat::Rgba16Float)
        .and_then(support::compiling)
    {
        run(backend, Tolerance::default());
    }
}

#[test]
fn hardware_eight_bit_surface_matches_reference() {
    if let Some(backend) = support::open(AdapterPreference::Hardware, SurfaceFormat::Bgra8Unorm)
        .and_then(support::compiling)
    {
        run(backend, EIGHT_BIT);
    }
}
