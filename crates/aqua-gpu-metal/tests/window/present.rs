use std::time::Duration;

use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_gpu_metal::scene::{OffscreenBackend, OffscreenOptions};
use aqua_gpu_metal::window::Alpha;
use aqua_graph::{Extent, FramePlan};
use aqua_protocol::{
    FrameInputs, GpuBackend, GpuError, GpuReadback, MetalSurface, PresentHint, SurfaceTarget,
    WindowId,
};
use aqua_shell_appkit::testing::capture;

use super::support::{
    self, drive, extent_of, frame, mismatches, painted, quad, shown, srgb, Flow, RenderThread,
    CAPTURES, LIMIT,
};

pub fn scene(extent: Extent) -> FramePlan {
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

pub fn drawable_equals_offscreen() {
    if !support::appkit() {
        return;
    }
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let window = support::window("metal present", (240.0, 160.0), (40, 80));
    let extent = extent_of(window);
    let surface = gpu.attach(window, Alpha::Opaque);
    let plan = scene(extent);
    let expected = support::offscreen(&plan);
    let read = gpu.run(move |backend| {
        backend
            .wait_for_frame_slot_with_hint(surface, PresentHint::Vsync)
            .expect("a frame slot");
        backend
            .execute(&plan, &FrameInputs::new(surface))
            .expect("execute");
        let read = backend.read_surface(surface).expect("readback");
        backend
            .present(surface, PresentHint::Vsync)
            .expect("present");
        read
    });
    assert_eq!(read.extent, extent);
    let differing = read
        .rgba8
        .chunks(4)
        .zip(expected.chunks(4))
        .filter(|(drawn, wanted)| drawn != wanted)
        .count();
    assert_eq!(
        differing, 0,
        "the drawable holds exactly the offscreen render"
    );
    let refused = Err(GpuError::Unsupported(
        "a Metal layer without its pacer: use WindowBackend::create_layer_surface",
    ));
    let target = SurfaceTarget::Metal(MetalSurface {
        window: WindowId::new(1).expect("window ids start at one"),
        layer: 1,
        extent: Extent::new(8, 8),
    });
    assert_eq!(
        gpu.run(move |backend| backend.create_surface(target)),
        refused
    );
    let mut offscreen =
        OffscreenBackend::new(OffscreenOptions::default()).expect("the offscreen backend starts");
    assert_eq!(
        offscreen.create_surface(target),
        Err(GpuError::Unsupported(
            "window surfaces on the offscreen backend"
        ))
    );
    gpu.assert_clean();
}

pub fn screen_shows_presented() {
    if !support::appkit() {
        return;
    }
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let window = support::window("metal screen", (240.0, 160.0), (40, 80));
    let extent = extent_of(window);
    let surface = gpu.attach(window, Alpha::Opaque);
    let plan = scene(extent);
    let expected = support::offscreen(&plan);
    let mut captures = 0;
    drive(LIMIT, move |_, _| {
        let drawn = plan.clone();
        gpu.run(move |backend| frame(backend, surface, &drawn, PresentHint::Vsync));
        if shown(&gpu, surface) < 3 {
            return Flow::Wait(Duration::from_millis(10));
        }
        let Some(image) = capture::window(window) else {
            return Flow::Done;
        };
        captures += 1;
        let last = mismatches(&image, &expected, extent, 3);
        match last.is_empty() {
            true => {
                gpu.assert_clean();
                Flow::Done
            }
            false if captures < CAPTURES => Flow::Wait(Duration::from_millis(30)),
            false => panic!(
                "{} of {} on-screen pixels differ from the offscreen render after {captures} captures, first at {:?}",
                last.len(),
                extent.texels(),
                last.first()
            ),
        }
    });
}
