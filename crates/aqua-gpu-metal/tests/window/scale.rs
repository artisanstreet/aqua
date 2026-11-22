use std::time::Duration;

use aqua_gpu_metal::window::Alpha;
use aqua_graph::Extent;
use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, PresentHint, SurfaceId};
use aqua_shell_appkit::testing;
use aqua_shell_appkit::ShellEvent;

use super::support::{self, drive, extent_of, solid, srgb, Flow, RenderThread, LIMIT};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Settle,
    Doubled,
    Restored,
}

fn render(gpu: &RenderThread, surface: SurfaceId, extent: Extent) -> Option<Extent> {
    let plan = solid(extent, srgb(60, 90, 140));
    gpu.run(move |backend| {
        backend.resize_surface(surface, extent).expect("resize");
        backend
            .wait_for_frame_slot_with_hint(surface, PresentHint::Vsync)
            .expect("a frame slot");
        backend
            .execute(&plan, &FrameInputs::new(surface))
            .expect("execute");
        let read = backend.read_surface(surface).ok().map(|read| read.extent);
        backend
            .present(surface, PresentHint::Vsync)
            .expect("present");
        read
    })
}

pub fn backing_scale_change() {
    if !support::appkit() {
        return;
    }
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let window = support::window("metal scale", (200.0, 120.0), (40, 80));
    let surface = gpu.attach(window, Alpha::Opaque);
    let base = window.scale();
    let doubled = (400, 240);
    let restored = ((200.0 * base).round() as u32, (120.0 * base).round() as u32);
    let mut phase = Phase::Settle;
    let mut seen = 0;
    drive(LIMIT, move |log, _| {
        let resized = |width: u32, height: u32| {
            log[seen..]
                .iter()
                .any(|(_, event, _)| *event == ShellEvent::Resized { width, height })
        };
        let scaled = |scale: f64| {
            log[seen..]
                .iter()
                .any(|(_, event, _)| *event == ShellEvent::ScaleChanged { scale })
        };
        match phase {
            Phase::Settle => {
                render(&gpu, surface, extent_of(window));
                if !log
                    .iter()
                    .any(|(_, event, _)| *event == ShellEvent::Covered(false))
                {
                    return Flow::Wait(Duration::from_millis(10));
                }
                seen = log.len();
                testing::report_backing_scale(window, Some(2.0));
                phase = Phase::Doubled;
                Flow::Wait(Duration::from_millis(10))
            }
            Phase::Doubled => {
                if !(scaled(2.0) && resized(doubled.0, doubled.1)) {
                    return Flow::Wait(Duration::from_millis(10));
                }
                assert_eq!(window.scale(), 2.0);
                assert_eq!(testing::contents_scale(window), Some(2.0));
                let extent = extent_of(window);
                assert_eq!((extent.width, extent.height), doubled);
                assert_eq!(
                    render(&gpu, surface, extent),
                    Some(extent),
                    "the next frame's drawable has twice the pixels"
                );
                seen = log.len();
                testing::report_backing_scale(window, None);
                phase = Phase::Restored;
                Flow::Wait(Duration::from_millis(10))
            }
            Phase::Restored => {
                if !(scaled(base) && resized(restored.0, restored.1)) {
                    return Flow::Wait(Duration::from_millis(10));
                }
                assert_eq!(testing::contents_scale(window), Some(base));
                let extent = extent_of(window);
                assert_eq!(render(&gpu, surface, extent), Some(extent));
                gpu.assert_clean();
                Flow::Done
            }
        }
    });
}
