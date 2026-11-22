use std::time::Duration;

use aqua_gpu_metal::window::Alpha;
use aqua_protocol::{FrameInputs, GpuBackend, PresentHint};

use super::support::{self, drive, extent_of, shown, solid, srgb, Flow, RenderThread, LIMIT};

pub fn closing_with_frames_in_flight() {
    if !support::appkit() {
        return;
    }
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let kept = support::window("metal kept", (200.0, 120.0), (20, 80));
    let closed = support::window("metal closed", (200.0, 120.0), (400, 80));
    let kept_surface = gpu.attach(kept, Alpha::Opaque);
    let closed_surface = gpu.attach(closed, Alpha::Opaque);
    let kept_plan = solid(extent_of(kept), srgb(70, 70, 160));
    let closed_plan = solid(extent_of(closed), srgb(160, 70, 70));
    let mut closed_at = None;
    drive(LIMIT, move |_, _| {
        gpu.frame(kept_surface, kept_plan.clone());
        match closed_at {
            None => {
                let plan = closed_plan.clone();
                gpu.run(move |backend| {
                    (0..3).for_each(|_| {
                        backend
                            .execute(&plan, &FrameInputs::new(closed_surface))
                            .expect("execute");
                        backend
                            .present(closed_surface, PresentHint::Vsync)
                            .expect("present");
                    })
                });
                closed.destroy();
                gpu.run(move |backend| backend.destroy_surface(closed_surface))
                    .expect("the closed window's surface is destroyed");
                closed_at = Some(shown(&gpu, kept_surface));
                Flow::Wait(Duration::from_millis(10))
            }
            Some(at) if shown(&gpu, kept_surface) < at + 10 => {
                Flow::Wait(Duration::from_millis(10))
            }
            Some(_) => {
                let plan = kept_plan.clone();
                gpu.run(move |backend| {
                    backend
                        .execute(&plan, &FrameInputs::new(kept_surface))
                        .expect("execute");
                    backend
                        .present(kept_surface, PresentHint::Vsync)
                        .expect("present");
                });
                gpu.assert_clean();
                Flow::Done
            }
        }
    });
}
