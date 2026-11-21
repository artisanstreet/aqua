use std::sync::{Arc, Mutex};
use std::time::Duration;

use aqua_gpu_vulkan::window::{Alpha, RecoveryNotice, TextureRecovery};
use aqua_protocol::{DeviceStatus, FrameInputs, GpuBackend, GpuError, PresentHint};
use aqua_shell_wayland::testing::capture;
use aqua_shell_wayland::{ShellEvent, Window};

use super::support::{
    self, bgr, close, drive, extent_of, frame, mapped, shown, srgb, Flow, RenderThread,
};

#[test]
fn device_loss_with_two_live_windows_recovers_both_surfaces() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let (left_app, right_app) = ("com.aqua.test.vulkan.left", "com.aqua.test.vulkan.right");
    let left = support::window(&compositor, left_app, (200.0, 120.0), (20, 20));
    let right = support::window(&compositor, right_app, (200.0, 120.0), (400, 20));
    let left_surface = gpu.attach(left, Alpha::Opaque);
    let right_surface = gpu.attach(right, Alpha::Opaque);
    let (left_extent, right_extent) = (extent_of(left), extent_of(right));
    gpu.frame(left_surface, support::solid(left_extent, srgb(200, 50, 50)));
    gpu.frame(
        right_surface,
        support::solid(right_extent, srgb(50, 50, 200)),
    );
    let notices: Arc<Mutex<Vec<RecoveryNotice>>> = Arc::default();
    let heard = notices.clone();
    gpu.run(move |backend| {
        backend.set_recovery_listener(Box::new(move |notice| {
            heard.lock().expect("the notice list").push(notice)
        }))
    });
    let (lost, status, report) = gpu.run(move |backend| {
        backend.inject_device_removal();
        let plan = support::solid(left_extent, srgb(0, 0, 0));
        let lost = backend.execute(&plan, &FrameInputs::new(left_surface));
        let status = backend.device_status();
        let report = backend.recover().expect("recovery rebuilds the device");
        (lost, status, report)
    });
    assert!(
        matches!(&lost, Err(GpuError::DeviceFailure(message)) if message.contains("lost")),
        "a lost device fails the frame: {lost:?}"
    );
    assert_eq!(status, DeviceStatus::Lost);
    assert_eq!(report.rebuilt_surfaces, 2);
    assert_eq!(
        notices.lock().expect("the notice list").as_slice(),
        &[RecoveryNotice {
            generation: 1,
            software: true,
            textures: TextureRecovery::HostMustReupload,
        }]
    );
    let before = (shown(&gpu, left_surface), shown(&gpu, right_surface));
    gpu.frame(left_surface, support::solid(left_extent, srgb(250, 200, 0)));
    gpu.frame(
        right_surface,
        support::solid(right_extent, srgb(0, 200, 120)),
    );
    let probe = compositor.clone();
    drive(Duration::from_secs(15), move |_| {
        let placed = mapped(&probe, left_app).zip(mapped(&probe, right_app));
        let fresh = shown(&gpu, left_surface) > before.0 && shown(&gpu, right_surface) > before.1;
        let Some((left_rect, right_rect)) = placed.filter(|_| fresh) else {
            return Flow::Wait(Duration::from_millis(10));
        };
        let capture = capture(&probe, false).expect("screencopy");
        let left_seen = bgr(&capture, left_rect.x + 20, left_rect.y + 20);
        let right_seen = bgr(&capture, right_rect.x + 20, right_rect.y + 20);
        assert!(close(left_seen, [250, 200, 0], 1), "{left_seen:?}");
        assert!(close(right_seen, [0, 200, 120], 1), "{right_seen:?}");
        assert_eq!(gpu.run(|backend| backend.generation()), 1);
        gpu.assert_clean();
        Flow::Done
    });
}

#[test]
fn destroying_windows_and_surfaces_with_frames_in_flight_is_clean() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let (doomed_app, kept_app) = ("com.aqua.test.vulkan.doomed", "com.aqua.test.vulkan.kept");
    let doomed = support::window(&compositor, doomed_app, (200.0, 120.0), (20, 20));
    let unpresented_app = "com.aqua.test.vulkan.unpresented";
    support::floating(&compositor, unpresented_app);
    let unpresented = Window::create(&support::options(unpresented_app, (160.0, 100.0)))
        .expect("the window is created");
    let kept = support::window(&compositor, kept_app, (200.0, 120.0), (400, 20));
    let doomed_surface = gpu.attach(doomed, Alpha::Opaque);
    let unpresented_surface = gpu.attach(unpresented, Alpha::Opaque);
    let kept_surface = gpu.attach(kept, Alpha::Opaque);
    let doomed_plan = support::solid(extent_of(doomed), srgb(120, 0, 0));
    let unpresented_plan = support::solid(extent_of(unpresented), srgb(0, 120, 0));
    let kept_plan = support::solid(extent_of(kept), srgb(0, 0, 120));
    gpu.frame(doomed_surface, doomed_plan.clone());
    gpu.frame(kept_surface, kept_plan.clone());
    gpu.run(move |backend| {
        backend
            .wait_for_frame_slot(unpresented_surface)
            .expect("a frame slot");
        backend
            .execute(&unpresented_plan, &FrameInputs::new(unpresented_surface))
            .expect("execute");
        backend
            .destroy_surface(unpresented_surface)
            .expect("a surface with an unpresented frame is destroyed");
    });
    unpresented.destroy();
    let probe = compositor.clone();
    let mut step = 0;
    let mut kept_before = 0;
    let log = drive(Duration::from_secs(15), move |_| {
        match step {
            0 if mapped(&probe, doomed_app).is_some() && shown(&gpu, doomed_surface) >= 1 => {
                let plan = doomed_plan.clone();
                gpu.run(move |backend| {
                    frame(backend, doomed_surface, &plan, PresentHint::Immediate, None)
                });
                doomed.destroy();
                step = 1;
            }
            1 => {
                let plan = doomed_plan.clone();
                gpu.run(move |backend| {
                    (0..3).for_each(|_| {
                        frame(backend, doomed_surface, &plan, PresentHint::Vsync, None);
                    });
                    backend
                        .destroy_surface(doomed_surface)
                        .expect("the surface of a destroyed window is destroyed");
                });
                step = 2;
            }
            2 => {
                kept_before = shown(&gpu, kept_surface);
                gpu.frame(kept_surface, kept_plan.clone());
                step = 3;
            }
            3 if shown(&gpu, kept_surface) > kept_before => {
                gpu.assert_clean();
                return Flow::Done;
            }
            _ => {}
        }
        Flow::Wait(Duration::from_millis(10))
    });
    let destroyed = log
        .iter()
        .filter(|(_, event)| *event == ShellEvent::Destroyed)
        .count();
    assert_eq!(destroyed, 2, "both destroyed windows report it");
    assert!(
        kept.is_visible(),
        "the remaining window and its connection live on"
    );
}
