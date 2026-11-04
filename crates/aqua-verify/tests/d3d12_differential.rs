#![cfg(windows)]

use aqua_gpu_d3d12::hal::tier::Tier;
use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat, TierLimit};
use aqua_gpu_ref::conformance::{compare, Session, Tolerance};
use aqua_gpu_ref::RefBackend;
use aqua_verify::differential::{scenes, variant_case, VARIANTS};

const SEED: u64 = 0xD3D1_2D1F;
const SAME_DEVICE: Tolerance = Tolerance {
    interior: 1,
    soft: 2,
    edge: 2,
    edge_band: 2,
};

const TIERS: [(&str, TierLimit); 3] = [
    ("baseline", TierLimit::BASELINE),
    (
        "bindless",
        TierLimit {
            binding: Tier::Bindless,
            enhanced_barriers: false,
        },
    ),
    ("max", TierLimit::MAX),
];

fn count() -> usize {
    std::env::var("AQUA_DIFFERENTIAL_FUZZ")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(400)
}

fn backend(preference: AdapterPreference, limit: TierLimit) -> OffscreenBackend {
    OffscreenBackend::with_tiers(preference, SurfaceFormat::Rgba16Float, limit)
        .unwrap_or_else(|error| panic!("no adapter {preference:?}: {error}"))
}

fn summary(name: &str, comparison: &aqua_gpu_ref::conformance::Comparison) -> String {
    format!(
        "{name}: interior {}, soft {}, edge {}",
        comparison.interior_max, comparison.soft_max, comparison.edge_max
    )
}

fn against_reference(preference: AdapterPreference) {
    let mut reference = RefBackend::new();
    let mut expected_session = Session::new(&mut reference);
    let mut failures = Vec::new();
    for (tier_name, limit) in TIERS {
        let mut device = backend(preference, limit);
        let mut actual_session = Session::new(&mut device);
        let mut worst = (0u8, 0u8, 0u8);
        let mut runs = 0;
        for scene in scenes(SEED, count()) {
            for variant in VARIANTS {
                let case = variant_case(&scene, variant).unwrap();
                let expected = expected_session.render_case(&case).unwrap();
                let name = format!("{tier_name}/{}", case.name);
                let actual = match actual_session.render_case(&case) {
                    Ok(actual) => actual,
                    Err(error) => {
                        failures.push(format!("{name}: {error}"));
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
                    failures.push(summary(&name, &comparison));
                }
            }
        }
        eprintln!(
            "differential {preference:?} {tier_name}: {runs} runs, worst interior {} soft {} edge {}",
            worst.0, worst.1, worst.2
        );
    }
    let shown = failures.len().min(25);
    assert!(
        failures.is_empty(),
        "{} failures, first {shown}:\n{}",
        failures.len(),
        failures[..shown].join("\n")
    );
}

fn tiers_agree(preference: AdapterPreference) {
    let mut baseline = backend(preference, TierLimit::BASELINE);
    let mut baseline_session = Session::new(&mut baseline);
    let mut failures = Vec::new();
    for (tier_name, limit) in TIERS.into_iter().skip(1) {
        let mut other = backend(preference, limit);
        let mut other_session = Session::new(&mut other);
        for scene in scenes(SEED ^ 3, count()) {
            let case = variant_case(&scene, VARIANTS[0]).unwrap();
            let a = baseline_session.render_case(&case).unwrap();
            let b = other_session.render_case(&case).unwrap();
            let comparison = compare(&case.plan, &a, &b, SAME_DEVICE);
            if !comparison.passed() {
                failures.push(summary(
                    &format!("{tier_name} vs baseline {}", case.name),
                    &comparison,
                ));
            }
        }
    }
    let shown = failures.len().min(25);
    assert!(
        failures.is_empty(),
        "{} failures, first {shown}:\n{}",
        failures.len(),
        failures[..shown].join("\n")
    );
}

#[test]
fn warp_matches_the_oracle_on_every_tier_and_plan_variant() {
    against_reference(AdapterPreference::Warp);
}

#[test]
fn integrated_matches_the_oracle_on_every_tier_and_plan_variant() {
    against_reference(AdapterPreference::MinimumPower);
}

#[test]
fn hardware_matches_the_oracle_on_every_tier_and_plan_variant() {
    against_reference(AdapterPreference::HighPerformance);
}

#[test]
fn warp_tiers_agree_with_each_other() {
    tiers_agree(AdapterPreference::Warp);
}

#[test]
fn hardware_tiers_agree_with_each_other() {
    tiers_agree(AdapterPreference::HighPerformance);
}

#[test]
fn integrated_tiers_agree_with_each_other() {
    tiers_agree(AdapterPreference::MinimumPower);
}

#[test]
fn integrated_and_hardware_agree_with_each_other() {
    let mut integrated = backend(AdapterPreference::MinimumPower, TierLimit::MAX);
    let mut hardware = backend(AdapterPreference::HighPerformance, TierLimit::MAX);
    let mut integrated_session = Session::new(&mut integrated);
    let mut hardware_session = Session::new(&mut hardware);
    let failures: Vec<String> = scenes(SEED ^ 11, count())
        .iter()
        .filter_map(|scene| {
            let case = variant_case(scene, VARIANTS[1]).unwrap();
            let a = integrated_session.render_case(&case).unwrap();
            let b = hardware_session.render_case(&case).unwrap();
            let comparison = compare(&case.plan, &a, &b, Tolerance::default());
            (!comparison.passed()).then(|| summary(&case.name, &comparison))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn the_fuzz_corpus_is_exactly_as_large_as_requested_and_never_repeats() {
    [64usize, 400, 1000].into_iter().for_each(|requested| {
        let corpus = scenes(SEED, requested);
        assert_eq!(corpus.len(), requested);
        let distinct: std::collections::HashSet<String> = corpus
            .iter()
            .map(|scene| format!("{:?}{:?}{:?}", scene.extent, scene.clear, scene.commands))
            .collect();
        assert!(
            distinct.len() * 100 >= requested * 98,
            "{} distinct of {requested}",
            distinct.len()
        );
    });
}

#[test]
fn hardware_and_warp_agree_with_each_other() {
    let mut hardware = backend(AdapterPreference::HighPerformance, TierLimit::MAX);
    let mut warp = backend(AdapterPreference::Warp, TierLimit::MAX);
    let mut hardware_session = Session::new(&mut hardware);
    let mut warp_session = Session::new(&mut warp);
    let mut failures = Vec::new();
    for scene in scenes(SEED ^ 7, count()) {
        let case = variant_case(&scene, VARIANTS[0]).unwrap();
        let a = hardware_session.render_case(&case).unwrap();
        let b = warp_session.render_case(&case).unwrap();
        let comparison = compare(&case.plan, &a, &b, Tolerance::default());
        if !comparison.passed() {
            failures.push(summary(&case.name, &comparison));
        }
    }
    let shown = failures.len().min(25);
    assert!(
        failures.is_empty(),
        "{} failures, first {shown}:\n{}",
        failures.len(),
        failures[..shown].join("\n")
    );
}
