#![cfg(windows)]

use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend};
use aqua_gpu_ref::conformance::{run_suite, SuiteConfig};
use aqua_gpu_ref::RefBackend;

fn config() -> SuiteConfig {
    let fuzz_count = std::env::var("AQUA_CONFORMANCE_FUZZ")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(SuiteConfig::default().fuzz_count);
    SuiteConfig {
        fuzz_count,
        ..SuiteConfig::default()
    }
}

fn run(preference: AdapterPreference) {
    let mut backend = match OffscreenBackend::new(preference) {
        Ok(backend) => backend,
        Err(error) => panic!("adapter {preference:?} required but unavailable: {error}"),
    };
    let mut reference = RefBackend::new();
    let report = run_suite(&mut reference, &mut backend, &config());
    eprintln!(
        "software={} unified={}\n{report}",
        backend.is_software(),
        backend.is_unified_memory()
    );
    assert_eq!(report.skipped(), 0, "{report}");
    assert!(
        report.passed_without_skips(),
        "{}",
        report.failures().join("\n")
    );
}

#[test]
fn warp_conforms_to_the_reference() {
    run(AdapterPreference::Warp);
}

#[test]
fn high_performance_adapter_conforms_to_the_reference() {
    run(AdapterPreference::HighPerformance);
}

#[test]
fn minimum_power_adapter_conforms_to_the_reference() {
    run(AdapterPreference::MinimumPower);
}
