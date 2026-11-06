use aqua_gpu_d3d12::hal::selection::AdapterPreference;
use aqua_gpu_ref::conformance::glass::{blurred, column_steepest, glass_cases_in, profile, rows};
use aqua_gpu_ref::RefBackend;
use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, PresentHint, SurfaceTarget};

use crate::backend::{backend, open, worst_difference};
use crate::test_window::TestWindow;

fn glass_on_window_surface(preference: AdapterPreference, label: &str) {
    let mut backend = backend(preference);
    let window = TestWindow::open(480, 380);
    let (id, extent) = open(&mut backend, &window);
    let mut reference = RefBackend::new();
    let reference_id = reference
        .create_surface(SurfaceTarget::Offscreen(extent))
        .unwrap();
    for glass in glass_cases_in(extent) {
        let plan = &glass.case.plan;
        reference
            .execute(plan, &FrameInputs::new(reference_id))
            .unwrap();
        let expected = reference.read_surface(reference_id).unwrap();
        backend.wait_for_frame_slot(id).unwrap();
        backend.execute(plan, &FrameInputs::new(id)).unwrap();
        let actual = backend.read_surface(id).unwrap();
        backend.present(id, PresentHint::Vsync).unwrap();
        window.pump();
        let name = &glass.case.name;
        for row in rows(glass.glass) {
            let seen = profile(&actual, glass.glass, row);
            assert!(
                blurred(seen),
                "[{label}] {name} row {row}: the window surface shows {seen:?} instead of the blurred stripes"
            );
        }
        let seam = column_steepest(&actual, glass.glass);
        assert!(
            seam <= aqua_gpu_ref::conformance::glass::MAX_STEP,
            "[{label}] {name}: vertical step {seam} is a seam"
        );
        let (worst, over) = worst_difference(&expected.rgba8, &actual.rgba8);
        assert!(
            over * 200 < expected.rgba8.len(),
            "[{label}] {name}: {over} channel values differ from the reference by more than 10 (worst {worst})"
        );
    }
}

#[test]
fn hardware_window_glass_blurs_what_lies_behind_it() {
    glass_on_window_surface(AdapterPreference::HighPerformance, "hardware");
}

#[test]
fn warp_window_glass_blurs_what_lies_behind_it() {
    glass_on_window_surface(AdapterPreference::Warp, "warp");
}
