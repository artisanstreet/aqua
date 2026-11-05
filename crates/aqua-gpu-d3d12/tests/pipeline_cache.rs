#![cfg(windows)]

mod support;

use aqua_core::Oklch;
use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat};
use aqua_gpu_ref::conformance::Session;
use aqua_gpu_ref::conformance::cases::{STANDARD, build, quad};

fn exercise(preference: AdapterPreference) {
    let Ok(first) =
        OffscreenBackend::with_pipeline_cache(preference, SurfaceFormat::Rgba16Float, None)
    else {
        return;
    };
    let cold = first.pipeline_stats();
    assert!(cold.compiled > 0);
    assert_eq!(cold.loaded, 0);
    let cache = first.pipeline_cache().expect("pipeline library serializes");
    assert!(!cache.is_empty());
    drop(first);
    let second =
        OffscreenBackend::with_pipeline_cache(preference, SurfaceFormat::Rgba16Float, Some(&cache))
            .expect("backend with a warm cache");
    let warm = second.pipeline_stats();
    assert_eq!(warm.compiled, 0, "warm start must not recompile: {warm:?}");
    assert_eq!(warm.loaded, cold.compiled);
}

#[test]
fn warp_reloads_pipelines_from_a_serialized_library() {
    let _software = support::software();
    exercise(AdapterPreference::Warp);
}

#[test]
fn hardware_reloads_pipelines_from_a_serialized_library() {
    let _software = support::software();
    exercise(AdapterPreference::HighPerformance);
}

#[test]
fn a_corrupt_cache_falls_back_to_compiling() {
    let _software = support::software();
    let backend = OffscreenBackend::with_pipeline_cache(
        AdapterPreference::Warp,
        SurfaceFormat::Rgba16Float,
        Some(&[1, 2, 3, 4, 5, 6, 7, 8]),
    )
    .expect("backend ignores an unusable cache");
    assert!(backend.pipeline_stats().compiled > 0);
}

#[test]
fn a_second_session_warms_only_the_pipelines_the_first_one_used() {
    let _software = support::software();
    let preference = AdapterPreference::Warp;
    let mut first =
        OffscreenBackend::with_pipeline_cache(preference, SurfaceFormat::Rgba16Float, None)
            .expect("backend");
    let total = first.pipeline_stats().compiled;
    let case = build(
        "one quad",
        STANDARD,
        Oklch::TRANSPARENT,
        &[quad(0.0, 0.0, 32.0, 32.0, "#ffffff", 1.0, 0.0, 0.0)],
    );
    Session::new(&mut first).render_case(&case).unwrap();
    let cache = first.pipeline_cache().expect("pipeline library serializes");
    drop(first);

    let mut second =
        OffscreenBackend::with_pipeline_cache(preference, SurfaceFormat::Rgba16Float, Some(&cache))
            .expect("backend with a usage hint");
    let warmed = second.pipeline_stats();
    assert_eq!(warmed.compiled, 0);
    assert!(
        warmed.loaded > 0 && warmed.loaded < total,
        "hinted warm-up loaded {} of {total}",
        warmed.loaded
    );
    Session::new(&mut second).render_case(&case).unwrap();
    assert_eq!(second.pipeline_stats().late, 0);
}
