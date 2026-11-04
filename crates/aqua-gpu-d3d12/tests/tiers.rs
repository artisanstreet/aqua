#![cfg(windows)]

mod support;

use aqua_gpu_d3d12::hal::tier::Tier;
use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat, TierLimit};
use aqua_gpu_ref::conformance::{run_suite, SuiteConfig, Tolerance};
use aqua_gpu_ref::RefBackend;
use aqua_protocol::FeatureTier;

#[derive(Clone, Copy, Debug)]
struct Combination {
    name: &'static str,
    limit: TierLimit,
}

const COMBINATIONS: [Combination; 4] = [
    Combination {
        name: "baseline",
        limit: TierLimit::BASELINE,
    },
    Combination {
        name: "bindless",
        limit: TierLimit {
            binding: Tier::Bindless,
            enhanced_barriers: false,
        },
    },
    Combination {
        name: "enhanced-barriers",
        limit: TierLimit {
            binding: Tier::Baseline,
            enhanced_barriers: true,
        },
    },
    Combination {
        name: "bindless+enhanced-barriers",
        limit: TierLimit::MAX,
    },
];

const EIGHT_BIT: Tolerance = Tolerance {
    interior: 6,
    soft: 16,
    edge: 64,
    edge_band: 2,
};

struct Outcome {
    binding: Tier,
    enhanced_barriers: bool,
    failing: Vec<String>,
    skipped: usize,
}

fn failing_names(report: &aqua_gpu_ref::conformance::SuiteReport) -> Vec<String> {
    let mut names: Vec<String> = report
        .cases
        .iter()
        .filter(|case| !case.passed())
        .map(|case| case.name.clone())
        .chain(
            report
                .contract
                .iter()
                .filter(|check| !check.passed())
                .map(|check| format!("contract {}", check.name)),
        )
        .collect();
    names.sort();
    names
}

fn exercise(
    preference: AdapterPreference,
    format: SurfaceFormat,
    tolerance: Tolerance,
    combination: Combination,
) -> Outcome {
    let mut backend = match OffscreenBackend::with_tiers(preference, format, combination.limit) {
        Ok(backend) => backend,
        Err(error) => match preference {
            AdapterPreference::Warp => panic!("WARP must always be available: {error}"),
            _ => panic!("adapter {preference:?} required but unavailable: {error}"),
        },
    };
    let tiers = backend.tiers();
    eprintln!(
        "{preference:?} {format:?} {}: binding {:?}, enhanced barriers {}, software {}",
        combination.name,
        tiers.binding,
        tiers.enhanced_barriers,
        backend.is_software()
    );
    assert!(
        tiers.binding <= combination.limit.binding,
        "tier exceeds the requested limit"
    );
    assert!(
        !tiers.enhanced_barriers || combination.limit.enhanced_barriers,
        "enhanced barriers exceed the requested limit"
    );
    let mut reference = RefBackend::new();
    let config = SuiteConfig {
        tolerance,
        ..SuiteConfig::default()
    };
    let report = run_suite(&mut reference, &mut backend, &config);
    eprintln!("{report}");
    let reported = match tiers.binding {
        Tier::Baseline => FeatureTier::Baseline,
        Tier::Bindless => FeatureTier::Bindless,
    };
    assert_eq!(report.tier, reported);
    Outcome {
        binding: tiers.binding,
        enhanced_barriers: tiers.enhanced_barriers,
        failing: failing_names(&report),
        skipped: report.skipped(),
    }
}

fn matrix(preference: AdapterPreference, format: SurfaceFormat, tolerance: Tolerance) {
    for combination in COMBINATIONS {
        let outcome = exercise(preference, format, tolerance, combination);
        assert_eq!(
            outcome.skipped, 0,
            "{preference:?} {format:?} {} skipped cases",
            combination.name
        );
        assert!(
            outcome.failing.is_empty(),
            "{preference:?} {format:?} {} (binding {:?}, enhanced {}) fails: {:?}",
            combination.name,
            outcome.binding,
            outcome.enhanced_barriers,
            outcome.failing
        );
    }
}

fn top_tier(preference: AdapterPreference, format: SurfaceFormat, tolerance: Tolerance) {
    for combination in [COMBINATIONS[0], COMBINATIONS[3]] {
        let outcome = exercise(preference, format, tolerance, combination);
        assert_eq!(
            outcome.skipped, 0,
            "{preference:?} {format:?} {} skipped cases",
            combination.name
        );
        assert!(
            outcome.failing.is_empty(),
            "{preference:?} {format:?} {} fails: {:?}",
            combination.name,
            outcome.failing
        );
    }
}

#[test]
fn warp_float_matches_reference_on_every_tier() {
    let _software = support::software();
    matrix(
        AdapterPreference::Warp,
        SurfaceFormat::Rgba16Float,
        Tolerance::default(),
    );
}

#[test]
fn hardware_float_matches_reference_on_every_tier() {
    let _software = support::software();
    matrix(
        AdapterPreference::HighPerformance,
        SurfaceFormat::Rgba16Float,
        Tolerance::default(),
    );
}

#[test]
fn integrated_float_matches_reference_on_every_tier() {
    let _software = support::software();
    matrix(
        AdapterPreference::MinimumPower,
        SurfaceFormat::Rgba16Float,
        Tolerance::default(),
    );
}

#[test]
fn warp_unorm_matches_the_baseline_tier_on_the_top_tier() {
    let _software = support::software();
    top_tier(
        AdapterPreference::Warp,
        SurfaceFormat::Bgra8Unorm,
        EIGHT_BIT,
    );
}

#[test]
fn hardware_unorm_matches_the_baseline_tier_on_the_top_tier() {
    let _software = support::software();
    top_tier(
        AdapterPreference::HighPerformance,
        SurfaceFormat::Bgra8Unorm,
        EIGHT_BIT,
    );
}

#[test]
fn the_default_backend_reaches_the_best_tier_the_adapter_offers() {
    let _software = support::software();
    for preference in [AdapterPreference::Warp, AdapterPreference::HighPerformance] {
        let Ok(default) = OffscreenBackend::new(preference) else {
            continue;
        };
        let Ok(max) =
            OffscreenBackend::with_tiers(preference, SurfaceFormat::Rgba16Float, TierLimit::MAX)
        else {
            continue;
        };
        assert_eq!(default.tiers(), max.tiers());
    }
}
