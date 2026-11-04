use std::cell::RefCell;
use std::rc::Rc;

use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_gpu_d3d12::hal::DeviceOptions;
use aqua_gpu_d3d12::hal::selection::AdapterPreference;
use aqua_gpu_d3d12::window::damage::{Damage, PixelRect};
use aqua_gpu_d3d12::window::{RecoveryNotice, WindowBackend, WindowOptions};
use aqua_gpu_ref::RefBackend;
use aqua_graph::{Extent, FramePlan, PaintCommand, Quad, plan_paint};
use aqua_protocol::{
    DeviceStatus, FrameInputs, GpuBackend, GpuError, GpuReadback, NativeSurface, PresentHint,
    SurfaceId, SurfaceTarget, WindowId,
};

use crate::test_window::TestWindow;

fn options(preference: AdapterPreference) -> WindowOptions {
    WindowOptions {
        device: DeviceOptions::from_env().with_preference(preference),
    }
}

fn backend(preference: AdapterPreference) -> Option<WindowBackend> {
    match WindowBackend::new(options(preference)) {
        Ok(backend) => Some(backend),
        Err(error) => match preference {
            AdapterPreference::Warp => panic!("WARP must always be available: {error}"),
            _ => {
                eprintln!("SKIP-NO-ADAPTER {preference:?}: {error}");
                None
            }
        },
    }
}

fn scene(extent: Extent, shade: f32) -> FramePlan {
    let quad = |x: f32, y: f32, w: f32, h: f32, hex: &str, radius: f32| {
        PaintCommand::Quad(Quad {
            bounds: Rect { x, y, w, h },
            radius,
            border_width: 0.0,
            fill: Oklch::from_hex(hex).unwrap(),
            border_color: Oklch::from_hex("#000000").unwrap(),
        })
    };
    let width = extent.width as f32;
    let height = extent.height as f32;
    let commands = [
        quad(8.0, 8.0, width * 0.5, height * 0.4, "#3b82f6", 0.0),
        quad(width * 0.4, height * 0.3, width * 0.5, height * 0.5, "#f97316", 12.0),
        quad(shade * 40.0, height - 30.0, 60.0, 20.0, "#22c55e", 0.0),
    ];
    plan_paint(extent, Oklch::from_hex("#20242c").unwrap(), &commands).unwrap()
}

fn open(
    backend: &mut WindowBackend,
    window: &TestWindow,
) -> (SurfaceId, Extent) {
    let (width, height) = window.client();
    let extent = Extent { width, height };
    let id = backend
        .create_surface(SurfaceTarget::Native(NativeSurface {
            window: WindowId::new(1).unwrap(),
            hwnd: window.raw() as u64,
            extent,
        }))
        .expect("surface");
    (id, extent)
}

fn frame(backend: &mut WindowBackend, window: &TestWindow, id: SurfaceId, plan: &FramePlan) {
    backend.wait_for_frame_slot(id).expect("slot");
    backend
        .execute(plan, &FrameInputs::new(id))
        .expect("execute");
    backend.present(id, PresentHint::Vsync).expect("present");
    window.pump();
}

fn worst_difference(left: &[u8], right: &[u8]) -> (u8, usize) {
    left.iter()
        .zip(right)
        .map(|(a, b)| a.abs_diff(*b))
        .fold((0u8, 0usize), |(worst, over), diff| {
            (worst.max(diff), over + (diff > 10) as usize)
        })
}

fn matches_reference(backend: &mut WindowBackend, id: SurfaceId, plan: &FramePlan, label: &str) {
    let mut reference = RefBackend::new();
    let reference_id = reference
        .create_surface(SurfaceTarget::Offscreen(plan.extent))
        .unwrap();
    reference
        .execute(plan, &FrameInputs::new(reference_id))
        .unwrap();
    let expected = reference.read_surface(reference_id).unwrap();
    backend.wait_for_frame_slot(id).unwrap();
    backend.execute(plan, &FrameInputs::new(id)).unwrap();
    let actual = backend.read_surface(id).unwrap();
    backend.present(id, PresentHint::Vsync).unwrap();
    assert_eq!(actual.extent, expected.extent);
    let (worst, over) = worst_difference(&expected.rgba8, &actual.rgba8);
    let total = expected.rgba8.len();
    eprintln!("[{label}] vs reference: worst={worst} over10={over}/{total}");
    assert!(
        over * 200 < total,
        "{label}: {over} of {total} channel values differ by more than 10"
    );
}

fn presents_and_matches(preference: AdapterPreference, label: &str) {
    let Some(mut backend) = backend(preference) else {
        return;
    };
    let window = TestWindow::open(520, 340);
    let (id, extent) = open(&mut backend, &window);
    eprintln!(
        "[{label}] adapter={} tearing={} software={}",
        backend.capabilities().adapter,
        backend.capabilities().features.tearing,
        backend.capabilities().features.software
    );
    matches_reference(&mut backend, id, &scene(extent, 0.0), label);
    for index in 0..60 {
        frame(&mut backend, &window, id, &scene(extent, index as f32 / 60.0));
    }
    let stats = backend.stats();
    assert_eq!(stats.presents, 61);
    assert_eq!(stats.frames, 61);
    assert_eq!(backend.device_status(), DeviceStatus::Healthy);
}

#[test]
fn hardware_window_presents_and_matches_the_reference() {
    presents_and_matches(AdapterPreference::HighPerformance, "hardware");
}

#[test]
fn warp_window_presents_and_matches_the_reference() {
    presents_and_matches(AdapterPreference::Warp, "warp");
}

fn resize_cycle(preference: AdapterPreference, label: &str) {
    let Some(mut backend) = backend(preference) else {
        return;
    };
    let window = TestWindow::open(520, 340);
    let (id, extent) = open(&mut backend, &window);
    frame(&mut backend, &window, id, &scene(extent, 0.0));
    for (width, height) in [(700, 420), (300, 200), (900, 600), (520, 340)] {
        window.resize(width, height);
        let (client_width, client_height) = window.client();
        let next = Extent {
            width: client_width,
            height: client_height,
        };
        let stale = backend.execute(&scene(extent, 0.0), &FrameInputs::new(id));
        if next != extent {
            assert!(matches!(stale, Err(GpuError::ExtentMismatch { .. })) || stale.is_ok());
        }
        backend.resize_surface(id, next).expect("resize");
        assert_eq!(backend.buffer_age(id), Some(0));
        matches_reference(&mut backend, id, &scene(next, 0.5), label);
        for _ in 0..4 {
            frame(&mut backend, &window, id, &scene(next, 0.25));
        }
    }
    let wrong = backend.execute(&scene(Extent { width: 64, height: 64 }, 0.0), &FrameInputs::new(id));
    assert!(matches!(wrong, Err(GpuError::ExtentMismatch { .. })));
}

#[test]
fn hardware_window_survives_repeated_resizes() {
    resize_cycle(AdapterPreference::HighPerformance, "hardware-resize");
}

#[test]
fn warp_window_survives_repeated_resizes() {
    resize_cycle(AdapterPreference::Warp, "warp-resize");
}

#[test]
fn damage_ages_follow_the_two_buffer_swapchain() {
    let Some(mut backend) = backend(AdapterPreference::Warp) else {
        return;
    };
    let window = TestWindow::open(400, 300);
    let (id, extent) = open(&mut backend, &window);
    assert_eq!(backend.buffer_age(id), Some(0));
    let patch = [Rect { x: 10.0, y: 10.0, w: 20.0, h: 20.0 }];
    for _ in 0..2 {
        backend.wait_for_frame_slot(id).unwrap();
        backend.execute(&scene(extent, 0.0), &FrameInputs::new(id)).unwrap();
        backend.present_with_damage(id, PresentHint::Vsync, &[]).unwrap();
    }
    assert_eq!(backend.buffer_age(id), Some(2));
    assert_eq!(backend.repaint_region(id, &patch).unwrap(), Damage::Full);
    backend.wait_for_frame_slot(id).unwrap();
    backend.execute(&scene(extent, 0.0), &FrameInputs::new(id)).unwrap();
    backend.present_with_damage(id, PresentHint::Vsync, &patch).unwrap();
    backend.wait_for_frame_slot(id).unwrap();
    backend.execute(&scene(extent, 0.0), &FrameInputs::new(id)).unwrap();
    backend.present_with_damage(id, PresentHint::Vsync, &patch).unwrap();
    let region = backend.repaint_region(id, &patch).unwrap();
    assert_eq!(
        region,
        Damage::Rects(vec![
            PixelRect { left: 10, top: 10, right: 30, bottom: 30 }
        ])
    );
    window.pump();
}

fn survive_loss(preference: AdapterPreference, label: &str) {
    let Some(mut backend) = backend(preference) else {
        return;
    };
    let notices: Rc<RefCell<Vec<RecoveryNotice>>> = Rc::default();
    let sink = notices.clone();
    backend.set_recovery_listener(Box::new(move |notice| sink.borrow_mut().push(notice)));
    let window = TestWindow::open(480, 320);
    let (id, extent) = open(&mut backend, &window);
    for _ in 0..8 {
        frame(&mut backend, &window, id, &scene(extent, 0.1));
    }
    backend.inject_device_removal().expect("removal");
    let outcome = backend
        .execute(&scene(extent, 0.2), &FrameInputs::new(id))
        .and_then(|_| backend.present(id, PresentHint::Vsync));
    assert!(outcome.is_err(), "a removed device must fail the frame");
    assert_eq!(backend.device_status(), DeviceStatus::Lost);

    let report = backend.recover().expect("recover");
    assert_eq!(report.rebuilt_surfaces, 1);
    assert_eq!(backend.device_status(), DeviceStatus::Healthy);
    assert_eq!(backend.generation(), 1);
    assert_eq!(notices.borrow().len(), 1);
    eprintln!(
        "[{label}] recovered on {} generation={}",
        backend.capabilities().adapter,
        backend.generation()
    );
    matches_reference(&mut backend, id, &scene(extent, 0.3), label);
    for _ in 0..8 {
        frame(&mut backend, &window, id, &scene(extent, 0.4));
    }

    backend.inject_device_removal().expect("second removal");
    assert!(backend.execute(&scene(extent, 0.5), &FrameInputs::new(id)).is_err());
    backend.recover().expect("second recovery");
    assert_eq!(backend.generation(), 2);
    assert!(backend.capabilities().features.software, "two losses fall back to WARP");
    for _ in 0..8 {
        frame(&mut backend, &window, id, &scene(extent, 0.6));
    }
    assert_eq!(notices.borrow().len(), 2);
    assert_eq!(backend.stats().recoveries, 2);
}

#[test]
fn hardware_window_recovers_from_device_loss_then_falls_back_to_warp() {
    survive_loss(AdapterPreference::HighPerformance, "hardware-loss");
}

#[test]
fn warp_window_recovers_from_device_loss() {
    survive_loss(AdapterPreference::Warp, "warp-loss");
}

#[test]
fn surfaces_can_be_destroyed_and_recreated_on_the_same_window() {
    let Some(mut backend) = backend(AdapterPreference::Warp) else {
        return;
    };
    let window = TestWindow::open(400, 300);
    let (first, extent) = open(&mut backend, &window);
    frame(&mut backend, &window, first, &scene(extent, 0.0));
    backend.destroy_surface(first).expect("destroy");
    assert!(backend.execute(&scene(extent, 0.0), &FrameInputs::new(first)).is_err());
    let (second, _) = open(&mut backend, &window);
    frame(&mut backend, &window, second, &scene(extent, 0.0));
}
