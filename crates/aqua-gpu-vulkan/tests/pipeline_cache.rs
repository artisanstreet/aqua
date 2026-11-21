#![cfg(target_os = "linux")]

mod support;

use aqua_gpu_ref::conformance::{cases, Case};
use aqua_gpu_vulkan::scene::{
    AdapterPreference, Descriptors, OffscreenBackend, OffscreenOptions, SurfaceFormat,
};
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
        device: support::options(AdapterPreference::Software, Descriptors::Automatic),
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
fn a_saved_pipeline_cache_builds_every_used_pipeline_before_the_first_frame() {
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
    let cache = cold.pipeline_cache().expect("cache bytes");
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
fn a_corrupt_cache_or_one_from_another_device_is_ignored() {
    let cases = chosen();
    let Some(mut cold) = open(None) else {
        return;
    };
    let expected = render_all(&mut cold, &cases);
    let info = cold.device_info().clone();
    let mut foreign = cold.pipeline_cache().expect("cache bytes");
    drop(cold);
    let names = u32::from_le_bytes([foreign[4], foreign[5], foreign[6], foreign[7]]) as usize;
    foreign.truncate(8 + names);
    let mut header = Vec::new();
    for word in [32u32, 1, info.vendor_id, info.device_id] {
        header.extend_from_slice(&word.to_le_bytes());
    }
    header.extend(info.cache_uuid.iter().map(|byte| !byte));
    header.extend_from_slice(&[0x5a; 64]);
    foreign.extend_from_slice(&header);
    for cache in [b"not a pipeline cache".to_vec(), foreign] {
        let mut backend = open(Some(cache)).expect("the device opened once already");
        assert_eq!(render_all(&mut backend, &cases), expected);
        support::assert_clean(&backend);
    }
}
