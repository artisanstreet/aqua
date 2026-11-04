#![cfg(windows)]

mod support;

use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat};
use aqua_gpu_ref::conformance::{run_suite, SuiteConfig, Tolerance};
use aqua_gpu_ref::RefBackend;

fn run(preference: AdapterPreference, format: SurfaceFormat, tolerance: Tolerance) {
    let mut backend = match OffscreenBackend::with_format(preference, format) {
        Ok(backend) => backend,
        Err(error) => match preference {
            AdapterPreference::Warp => panic!("WARP must always be available: {error}"),
            _ => panic!("adapter {preference:?} required but unavailable: {error}"),
        },
    };
    eprintln!(
        "{format:?} software={} unified={}",
        backend.is_software(),
        backend.is_unified_memory()
    );
    let mut reference = RefBackend::new();
    let config = SuiteConfig {
        tolerance,
        ..SuiteConfig::default()
    };
    let report = run_suite(&mut reference, &mut backend, &config);
    eprintln!("{report}");
    assert_eq!(report.skipped(), 0, "{report}");
    assert!(
        report.passed_without_skips(),
        "{}",
        report.failures().join("\n")
    );
}

const EIGHT_BIT: Tolerance = Tolerance {
    interior: 6,
    soft: 16,
    edge: 64,
    edge_band: 2,
};
const SRGB_ENCODED: Tolerance = Tolerance {
    interior: 12,
    soft: 24,
    edge: 96,
    edge_band: 2,
};

#[test]
fn warp_float_surface_matches_reference() {
    let _software = support::software();
    run(
        AdapterPreference::Warp,
        SurfaceFormat::Rgba16Float,
        Tolerance::default(),
    );
}

#[test]
fn hardware_float_surface_matches_reference() {
    let _software = support::software();
    run(
        AdapterPreference::HighPerformance,
        SurfaceFormat::Rgba16Float,
        Tolerance::default(),
    );
}

#[test]
fn warp_unorm_surface_matches_reference() {
    let _software = support::software();
    run(
        AdapterPreference::Warp,
        SurfaceFormat::Bgra8Unorm,
        EIGHT_BIT,
    );
}

#[test]
fn hardware_unorm_surface_matches_reference() {
    let _software = support::software();
    run(
        AdapterPreference::HighPerformance,
        SurfaceFormat::Bgra8Unorm,
        EIGHT_BIT,
    );
}

#[test]
fn warp_srgb_surface_matches_reference() {
    let _software = support::software();
    run(
        AdapterPreference::Warp,
        SurfaceFormat::Bgra8UnormSrgb,
        SRGB_ENCODED,
    );
}

#[test]
fn hardware_srgb_surface_matches_reference() {
    let _software = support::software();
    run(
        AdapterPreference::HighPerformance,
        SurfaceFormat::Bgra8UnormSrgb,
        SRGB_ENCODED,
    );
}
