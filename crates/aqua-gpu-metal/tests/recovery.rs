#![cfg(target_os = "macos")]

mod support;

use aqua_gpu_metal::scene::{OffscreenBackend, SurfaceFormat};
use aqua_gpu_ref::conformance::{cases, Case};
use aqua_protocol::{
    DeviceStatus, FrameInputs, GpuBackend, GpuError, GpuReadback, SurfaceId, SurfacePixels,
    SurfaceTarget,
};

fn case(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("conformance case {name} exists"))
}

fn render(backend: &mut OffscreenBackend, surface: SurfaceId, case: &Case) -> SurfacePixels {
    backend.upload(case.uploads.clone()).expect("upload");
    backend
        .execute(&case.plan, &FrameInputs::new(surface))
        .expect("execute");
    backend.read_surface(surface).expect("readback")
}

#[test]
fn a_lost_device_fails_frames_and_recovers_after_the_host_uploads_its_textures_again() {
    let Some(mut backend) = support::open(SurfaceFormat::Rgba16Float) else {
        return;
    };
    let case = case("rounded_clip_glyphs_and_image");
    let surface = backend
        .create_surface(SurfaceTarget::Offscreen(case.plan.extent))
        .expect("surface");
    let before = render(&mut backend, surface, &case);

    backend.inject_device_removal();
    let lost = backend.execute(&case.plan, &FrameInputs::new(surface));
    assert!(
        matches!(&lost, Err(GpuError::DeviceFailure(message)) if message.contains("lost")),
        "a lost device fails the frame: {lost:?}"
    );
    assert_eq!(backend.device_status(), DeviceStatus::Lost);
    assert!(backend.read_surface(surface).is_err());

    let report = backend.recover().expect("recover");
    assert_eq!(report.rebuilt_surfaces, 1);
    assert_eq!(backend.device_status(), DeviceStatus::Healthy);
    let stale = backend.execute(&case.plan, &FrameInputs::new(surface));
    assert!(
        matches!(stale, Err(GpuError::UnknownTexture(_))),
        "textures are gone until the host uploads them again: {stale:?}"
    );

    let after = render(&mut backend, surface, &case);
    assert_eq!(after, before);
    support::assert_clean(&backend);
}
