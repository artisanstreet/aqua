#![cfg(windows)]

use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend};
use aqua_gpu_ref::RefBackend;
use aqua_gpu_ref::conformance::{SuiteConfig, run_suite};

fn config() -> SuiteConfig {
    let fuzz_count = std::env::var("AQUA_CONFORMANCE_FUZZ")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(SuiteConfig::default().fuzz_count);
    let tolerate_unsupported =
        std::env::var("AQUA_CONFORMANCE_TOLERATE_UNSUPPORTED").is_ok_and(|value| value == "1");
    SuiteConfig {
        fuzz_count,
        tolerate_unsupported,
        ..SuiteConfig::default()
    }
}

fn run(preference: AdapterPreference) {
    let mut backend = match OffscreenBackend::new(preference) {
        Ok(backend) => backend,
        Err(error) => match (
            preference,
            std::env::var("AQUA_REQUIRE_GPU").is_ok_and(|value| value == "1"),
        ) {
            (AdapterPreference::Warp, _) => panic!("WARP must always be available: {error}"),
            (_, true) => panic!("adapter {preference:?} required but unavailable: {error}"),
            _ => {
                eprintln!("SKIP-NO-ADAPTER {preference:?}: {error}");
                return;
            }
        },
    };
    let mut reference = RefBackend::new();
    let report = run_suite(&mut reference, &mut backend, &config());
    eprintln!(
        "software={} unified={}\n{report}",
        backend.is_software(),
        backend.is_unified_memory()
    );
    assert!(report.passed(), "{}", report.failures().join("\n"));
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
