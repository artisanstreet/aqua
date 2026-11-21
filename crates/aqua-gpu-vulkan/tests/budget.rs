#![cfg(target_os = "linux")]

mod support;

use aqua_gpu_ref::conformance::{cases, Case};
use aqua_gpu_vulkan::hal::budget::Visibility;
use aqua_gpu_vulkan::scene::{
    AdapterPreference, Descriptors, OffscreenBackend, OffscreenOptions, SceneOptions, SurfaceFormat,
};
use aqua_protocol::{
    FrameInputs, GpuBackend, GpuReadback, SurfaceId, SurfacePixels, SurfaceTarget,
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
fn a_hidden_backend_frees_its_transients_and_rings_and_the_next_frame_renders_the_same() {
    let Some(mut backend) = support::open(AdapterPreference::Software, SurfaceFormat::Rgba16Float)
    else {
        return;
    };
    let budget = backend
        .memory_budget()
        .expect("lavapipe reports VK_EXT_memory_budget");
    assert!(
        budget.usage > 0 && budget.usage <= budget.budget,
        "{budget:?}"
    );
    let case = case("explicit_clear_before_backdrop_read");
    let surface = backend
        .create_surface(SurfaceTarget::Offscreen(case.plan.extent))
        .expect("surface");
    let first = render(&mut backend, surface, &case);
    let transients = backend.transient_bytes();
    assert!(transients > 0, "a backdrop blur needs transients");
    assert!(backend.ring_bytes() > 0);

    let freed = backend.service_budget(Visibility::Hidden);
    assert!(
        freed >= transients,
        "freed {freed} of {transients} transient bytes"
    );
    assert_eq!(backend.transient_bytes(), 0);
    assert_eq!(backend.ring_bytes(), 0);

    assert_eq!(render(&mut backend, surface, &case), first);
    assert_eq!(backend.transient_bytes(), transients);
    support::assert_clean(&backend);
}

#[test]
fn a_plan_whose_transients_exceed_the_ceiling_is_refused_and_smaller_plans_still_render() {
    let options = OffscreenOptions {
        device: support::options(AdapterPreference::Software, Descriptors::Automatic),
        format: SurfaceFormat::Rgba16Float,
        scene: SceneOptions {
            transient_ceiling: Some(1024),
            ..SceneOptions::default()
        },
        ..OffscreenOptions::default()
    };
    let Some(mut backend) = support::open_with(options) else {
        return;
    };
    let heavy = case("explicit_clear_before_backdrop_read");
    let light = case("explicit_clear_translucent_empty_frame");
    let surface = backend
        .create_surface(SurfaceTarget::Offscreen(heavy.plan.extent))
        .expect("surface");
    backend.upload(heavy.uploads.clone()).expect("upload");
    let refused = backend.execute(&heavy.plan, &FrameInputs::new(surface));
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| error.to_string().contains("ceiling")),
        "{refused:?}"
    );
    assert!(
        light.plan.textures.is_empty(),
        "the light case needs no transients"
    );
    render(&mut backend, surface, &light);
    support::assert_clean(&backend);
}
