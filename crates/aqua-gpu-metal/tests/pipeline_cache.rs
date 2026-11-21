#![cfg(target_os = "macos")]

mod support;

use aqua_gpu_metal::scene::{OffscreenBackend, OffscreenOptions, SurfaceFormat};
use aqua_gpu_ref::conformance::{cases, Case};
use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, SurfacePixels, SurfaceTarget};

const CASES: [&str; 3] = [
    "rounded_clip_glyphs_and_image",
    "explicit_clear_before_backdrop_read",
    "rounded_clip_inside_opacity_group",
];

fn chosen() -> Vec<Case> {
    cases()
        .into_iter()
        .filter(|case| CASES.contains(&case.name.as_str()))
        .collect()
}

fn open(cache: Option<Vec<u8>>) -> Option<OffscreenBackend> {
    support::open_with(OffscreenOptions {
        format: SurfaceFormat::Rgba16Float,
        pipeline_cache: cache,
        ..OffscreenOptions::default()
    })
}

fn render_all(backend: &mut OffscreenBackend, cases: &[Case]) -> Vec<SurfacePixels> {
    cases
        .iter()
        .map(|case| {
            let surface = backend
                .create_surface(SurfaceTarget::Offscreen(case.plan.extent))
                .expect("surface");
            backend.upload(case.uploads.clone()).expect("upload");
            backend
                .execute(&case.plan, &FrameInputs::new(surface))
                .expect("execute");
            backend.read_surface(surface).expect("readback")
        })
        .collect()
}

#[test]
fn a_saved_pipeline_hint_builds_every_used_pipeline_before_the_first_frame() {
    let cases = chosen();
    assert_eq!(cases.len(), CASES.len());
    let Some(mut cold) = open(None) else {
        return;
    };
    let expected = render_all(&mut cold, &cases);
    assert!(
        !cold.late_pipelines().is_empty(),
        "a cold start builds some pipelines late"
    );
    let cache = cold.pipeline_cache();
    assert!(cache.starts_with(b"AQH1"), "the cache is the AQH1 hint");
    drop(cold);

    let mut warm = open(Some(cache)).expect("the device opened once already");
    assert_eq!(render_all(&mut warm, &cases), expected);
    assert!(
        warm.late_pipelines().is_empty(),
        "late after a warm start: {:?}",
        warm.late_pipelines()
    );
    support::assert_clean(&warm);
}

#[test]
fn a_corrupt_cache_or_a_hint_naming_unknown_pipelines_is_ignored() {
    let cases = chosen();
    let Some(mut cold) = open(None) else {
        return;
    };
    let expected = render_all(&mut cold, &cases);
    drop(cold);
    let mut foreign = b"AQH1".to_vec();
    let names = b"Quad\n+Raytrace";
    foreign.extend_from_slice(&(names.len() as u32).to_le_bytes());
    foreign.extend_from_slice(names);
    foreign.extend_from_slice(&[0x5a; 64]);
    for cache in [b"not a pipeline cache".to_vec(), foreign] {
        let mut backend = open(Some(cache)).expect("the device opened once already");
        assert_eq!(render_all(&mut backend, &cases), expected);
        support::assert_clean(&backend);
    }
}
