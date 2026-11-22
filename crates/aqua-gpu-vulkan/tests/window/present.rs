use std::time::Duration;

use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_gpu_vulkan::scene::{OffscreenBackend, OffscreenOptions, SurfaceFormat};
use aqua_gpu_vulkan::window::Alpha;
use aqua_graph::{Extent, FramePlan};
use aqua_protocol::{
    FrameInputs, GpuBackend, GpuError, GpuReadback, MetalSurface, PresentHint, SurfaceTarget,
    WindowId,
};
use aqua_shell_wayland::testing::capture;

use super::support::{
    self, bgr, close, device_options, drive, extent_of, mapped, painted, quad, shown, srgb, Flow,
    RenderThread,
};

fn scene(extent: Extent) -> FramePlan {
    painted(
        extent,
        srgb(240, 240, 236),
        &[
            quad(
                Rect {
                    x: 12.0,
                    y: 10.0,
                    w: 80.0,
                    h: 50.0,
                },
                srgb(200, 40, 40),
                0.0,
            ),
            quad(
                Rect {
                    x: 110.5,
                    y: 20.25,
                    w: 90.0,
                    h: 90.0,
                },
                srgb(30, 120, 220),
                18.0,
            ),
            quad(
                Rect {
                    x: 20.0,
                    y: 90.0,
                    w: 60.0,
                    h: 40.0,
                },
                Oklch::from_srgb8(20, 160, 60, 0.6),
                6.0,
            ),
        ],
    )
}

fn offscreen(plan: &FramePlan) -> Vec<u8> {
    let mut backend = OffscreenBackend::new(OffscreenOptions {
        device: device_options(),
        format: SurfaceFormat::Bgra8Unorm,
        ..OffscreenOptions::default()
    })
    .expect("the offscreen backend starts");
    let surface = backend
        .create_surface(SurfaceTarget::Offscreen(plan.extent))
        .expect("surface");
    backend
        .execute(plan, &FrameInputs::new(surface))
        .expect("execute");
    backend.read_surface(surface).expect("readback").rgba8
}

#[test]
fn a_presented_window_shows_the_offscreen_render_on_screen() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let app = "com.aqua.test.vulkan.present";
    let window = support::window(&compositor, app, (240.0, 160.0), (40, 30));
    let extent = extent_of(window);
    let surface = gpu.attach(window, Alpha::Opaque);
    let plan = scene(extent);
    let expected = offscreen(&plan);
    let drawn = plan.clone();
    let (read, mode) = gpu.run(move |backend| {
        backend
            .wait_for_frame_slot_with_hint(surface, PresentHint::Vsync)
            .expect("a frame slot");
        backend
            .execute(&drawn, &FrameInputs::new(surface))
            .expect("execute");
        let read = backend.read_surface(surface).expect("readback");
        backend
            .present(surface, PresentHint::Vsync)
            .expect("present");
        (read, backend.present_mode(surface))
    });
    assert_eq!(read.extent, extent);
    assert!(
        read.rgba8 == expected,
        "the swapchain image holds exactly the offscreen render"
    );
    assert_eq!(
        mode,
        Some(ash::vk::PresentModeKHR::MAILBOX),
        "the pacer throttles, so MAILBOX is preferred"
    );
    let probe = compositor.clone();
    drive(Duration::from_secs(10), move |_| {
        let rect = match (mapped(&probe, app), shown(&gpu, surface)) {
            (Some(rect), shown) if shown >= 1 => rect,
            _ => return Flow::Wait(Duration::from_millis(10)),
        };
        let capture = &capture(&probe, false).expect("screencopy");
        let mismatched: Vec<(u32, u32)> = (0..extent.height)
            .flat_map(|y| (0..extent.width).map(move |x| (x, y)))
            .filter(|(x, y)| {
                let offset = ((y * extent.width + x) * 4) as usize;
                let wanted = [expected[offset], expected[offset + 1], expected[offset + 2]];
                !close(
                    bgr(capture, rect.x + *x as i32, rect.y + *y as i32),
                    wanted,
                    1,
                )
            })
            .collect();
        assert!(
            mismatched.is_empty(),
            "{} of {} on-screen pixels differ from the offscreen render, first at {:?}",
            mismatched.len(),
            extent.texels(),
            mismatched.first()
        );
        gpu.assert_clean();
        Flow::Done
    });
}

#[test]
fn a_transparent_window_composites_premultiplied_alpha_over_the_window_below() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let below_app = "com.aqua.test.vulkan.below";
    let above_app = "com.aqua.test.vulkan.above";
    let below = support::window(&compositor, below_app, (300.0, 200.0), (0, 0));
    let below_surface = gpu.attach(below, Alpha::Opaque);
    gpu.frame(
        below_surface,
        support::solid(extent_of(below), srgb(220, 40, 30)),
    );
    let above = support::window(&compositor, above_app, (160.0, 100.0), (60, 50));
    let above_surface = gpu.attach(above, Alpha::Premultiplied);
    let glass = support::solid(extent_of(above), Oklch::from_srgb8(20, 60, 230, 0.5));
    let read = gpu.run(move |backend| {
        backend
            .wait_for_frame_slot(above_surface)
            .expect("a frame slot");
        backend
            .execute(&glass, &FrameInputs::new(above_surface))
            .expect("execute");
        let read = backend.read_surface(above_surface).expect("readback");
        backend
            .present(above_surface, PresentHint::Vsync)
            .expect("present");
        read
    });
    let [r, g, b, a] = [read.rgba8[0], read.rgba8[1], read.rgba8[2], read.rgba8[3]];
    assert!(
        (100..200).contains(&a),
        "the glass is half transparent: alpha {a}"
    );
    let probe = compositor.clone();
    drive(Duration::from_secs(10), move |_| {
        if shown(&gpu, above_surface) == 0 {
            return Flow::Wait(Duration::from_millis(10));
        }
        let capture = capture(&probe, false).expect("screencopy");
        let under = bgr(&capture, 10, 10);
        let blend = |top: u8, bottom: u8| top as f32 + bottom as f32 * (255 - a) as f32 / 255.0;
        let expected = [
            blend(r, under[0]).round() as u8,
            blend(g, under[1]).round() as u8,
            blend(b, under[2]).round() as u8,
        ];
        let seen = bgr(&capture, 60 + 40, 50 + 40);
        assert!(
            close(seen, expected, 2),
            "premultiplied {:?} over {under:?} composites to {expected:?}, screen has {seen:?}",
            [r, g, b, a]
        );
        assert!(
            !close(seen, [r, g, b], 20),
            "the window below shows through"
        );
        gpu.assert_clean();
        Flow::Done
    });
}

#[test]
fn a_metal_layer_target_is_refused_as_unsupported() {
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let target = SurfaceTarget::Metal(MetalSurface {
        window: WindowId::new(1).expect("window ids start at one"),
        layer: 1,
        extent: Extent::new(8, 8),
    });
    let refused = Err(GpuError::Unsupported("Metal layers need aqua-gpu-metal"));
    assert_eq!(
        gpu.run(move |backend| backend.create_surface(target)),
        refused
    );
    let mut offscreen = OffscreenBackend::new(OffscreenOptions {
        device: device_options(),
        ..OffscreenOptions::default()
    })
    .expect("the offscreen backend starts where the window one does");
    assert_eq!(offscreen.create_surface(target), refused);
}
