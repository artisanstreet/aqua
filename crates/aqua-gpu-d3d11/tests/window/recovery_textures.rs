use std::cell::RefCell;
use std::rc::Rc;

use aqua_core::Oklch;
use aqua_core::geometry::Rect;
use aqua_gpu_d3d11::hal::selection::AdapterPreference;
use aqua_gpu_d3d11::window::{RecoveryNotice, TextureRecovery, WindowBackend};
use aqua_gpu_ref::RefBackend;
use aqua_graph::{Extent, FramePlan, PlanLimits, RenderGraph, TextureRef};
use aqua_media::atlas::{ImageKey, Placement, Slot};
use aqua_media::residency::{Residency, ResidencyConfig};
use aqua_media::upload::{image_instance, image_pass, upload_batch};
use aqua_protocol::{
    FrameInputs, GpuBackend, GpuReadback, PresentHint, SurfaceId, SurfaceTarget, UploadBatch,
};

use crate::backend::{options, worst_difference};
use crate::test_window::TestWindow;

const KEY: ImageKey = ImageKey {
    asset: 3,
    frame: 0,
    width: 32,
    height: 32,
};

fn pixels() -> Vec<u8> {
    (0..32u32 * 32)
        .flat_map(|index| {
            let (x, y) = (index % 32, index / 32);
            [(x * 8) as u8, (y * 8) as u8, 160, 255]
        })
        .collect()
}

fn placement(slot: Slot) -> Placement {
    match slot {
        Slot::Resident(placement) => placement,
        Slot::Blank { .. } => panic!("image must be resident"),
    }
}

fn plan(placement: &Placement, extent: Extent) -> FramePlan {
    let mut graph = RenderGraph::new(extent, Oklch::from_hex("#20242c").unwrap());
    graph.push(image_pass(
        vec![image_instance(
            placement,
            Rect {
                x: 20.0,
                y: 20.0,
                w: 128.0,
                h: 128.0,
            },
            0.0,
            1.0,
            None,
        )],
        TextureRef::Surface,
    ));
    graph.plan(PlanLimits::default()).unwrap()
}

fn residency_image(residency: &mut Residency) -> (Placement, UploadBatch) {
    residency.begin_frame();
    let placement = placement(residency.image(KEY, &pixels()).unwrap());
    (placement, upload_batch(residency.finish_frame()))
}

fn reference_of(plan: &FramePlan, uploads: &[UploadBatch]) -> Vec<u8> {
    let mut reference = RefBackend::new();
    uploads
        .iter()
        .for_each(|batch| reference.upload(batch.clone()).unwrap());
    let id = reference
        .create_surface(SurfaceTarget::Offscreen(plan.extent))
        .unwrap();
    reference.execute(plan, &FrameInputs::new(id)).unwrap();
    reference.read_surface(id).unwrap().rgba8
}

fn draws_like_the_reference(
    backend: &mut WindowBackend,
    id: SurfaceId,
    plan: &FramePlan,
    uploads: &[UploadBatch],
    label: &str,
) {
    let expected = reference_of(plan, uploads);
    backend.wait_for_frame_slot(id).unwrap();
    backend.execute(plan, &FrameInputs::new(id)).unwrap();
    let actual = backend.read_surface(id).unwrap().rgba8;
    backend.present(id, PresentHint::Vsync).unwrap();
    let (worst, over) = worst_difference(&expected, &actual);
    let lit = actual
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[0] != actual[0])
        .count();
    assert!(lit > 1000, "[{label}] the image must be visible");
    assert!(
        over * 200 < expected.len(),
        "[{label}] worst {worst}, {over} over"
    );
}

type Setup = (
    WindowBackend,
    TestWindow,
    SurfaceId,
    Extent,
    Rc<RefCell<Vec<RecoveryNotice>>>,
);

fn setup(mirror: bool, preference: AdapterPreference) -> Setup {
    crate::support::hold_software();
    let options = options(preference).with_mirror(mirror);
    let mut backend = WindowBackend::new(options)
        .unwrap_or_else(|error| panic!("no adapter {preference:?}: {error}"));
    let notices: Rc<RefCell<Vec<RecoveryNotice>>> = Rc::default();
    let sink = notices.clone();
    backend.set_recovery_listener(Box::new(move |notice| sink.borrow_mut().push(notice)));
    let window = TestWindow::open(300, 200);
    let (id, extent) = crate::backend::open(&mut backend, &window);
    (backend, window, id, extent, notices)
}

fn mirrored_textures_survive_device_loss(preference: AdapterPreference, label: &str) {
    let (mut backend, window, id, extent, notices) = setup(true, preference);
    let mut residency = Residency::new(ResidencyConfig::default());
    let (placement, batch) = residency_image(&mut residency);
    backend.upload(batch.clone()).unwrap();
    let frame = plan(&placement, extent);
    draws_like_the_reference(
        &mut backend,
        id,
        &frame,
        std::slice::from_ref(&batch),
        label,
    );
    window.pump();
    assert!(backend.mirrored_textures() > 0);

    backend.inject_device_removal().unwrap();
    assert!(
        backend
            .execute(&frame, &FrameInputs::new(id))
            .and_then(|_| backend.present(id, PresentHint::Vsync))
            .is_err()
    );
    backend.recover().unwrap();
    let notice = notices.borrow()[0];
    assert!(
        matches!(notice.textures, TextureRecovery::Replayed { textures } if textures > 0),
        "{notice:?}"
    );
    draws_like_the_reference(&mut backend, id, &frame, &[batch], label);
    window.pump();
}

fn unmirrored_textures_need_the_host_to_reupload(preference: AdapterPreference, label: &str) {
    let (mut backend, window, id, extent, notices) = setup(false, preference);
    let mut residency = Residency::new(ResidencyConfig::default());
    let (placement, batch) = residency_image(&mut residency);
    backend.upload(batch.clone()).unwrap();
    let frame = plan(&placement, extent);
    draws_like_the_reference(&mut backend, id, &frame, &[batch], label);
    window.pump();
    assert_eq!(backend.mirrored_textures(), 0);

    backend.inject_device_removal().unwrap();
    assert!(backend.execute(&frame, &FrameInputs::new(id)).is_err());
    backend.recover().unwrap();
    assert_eq!(
        notices.borrow()[0].textures,
        TextureRecovery::HostMustReupload
    );
    assert!(
        backend.execute(&frame, &FrameInputs::new(id)).is_err(),
        "[{label}] a texture that was never re-uploaded must not draw"
    );

    residency.clear();
    let (placement, batch) = residency_image(&mut residency);
    backend.upload(batch.clone()).unwrap();
    let frame = plan(&placement, extent);
    draws_like_the_reference(&mut backend, id, &frame, &[batch], label);
    window.pump();
}

#[test]
fn hardware_replays_mirrored_textures_after_recovery() {
    mirrored_textures_survive_device_loss(AdapterPreference::HighPerformance, "hardware-mirror");
}

#[test]
fn warp_replays_mirrored_textures_after_recovery() {
    mirrored_textures_survive_device_loss(AdapterPreference::Warp, "warp-mirror");
}

#[test]
fn hardware_host_reuploads_after_recovery_when_unmirrored() {
    unmirrored_textures_need_the_host_to_reupload(
        AdapterPreference::HighPerformance,
        "hardware-host",
    );
}

#[test]
fn warp_host_reuploads_after_recovery_when_unmirrored() {
    unmirrored_textures_need_the_host_to_reupload(AdapterPreference::Warp, "warp-host");
}
