use std::time::Duration;

use aqua_core::geometry::Rect;
use aqua_gpu_metal::window::Alpha;
use aqua_graph::{Extent, FramePlan};
use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, PresentHint};
use aqua_shell_appkit::testing::{self, capture};

use super::support::{
    self, close, content, drive, extent_of, painted, pixel, quad, shown, srgb, Flow, RenderThread,
    CAPTURES, LIMIT,
};

const STEPS: usize = 40;
const CORNER: [u8; 3] = [200, 40, 40];
const FAR: [u8; 3] = [30, 120, 220];
const GROUND: [u8; 3] = [240, 240, 236];

fn pinned(extent: Extent) -> FramePlan {
    let (width, height) = (extent.width as f32, extent.height as f32);
    painted(
        extent,
        srgb(GROUND[0], GROUND[1], GROUND[2]),
        &[
            quad(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 24.0,
                    h: 24.0,
                },
                srgb(CORNER[0], CORNER[1], CORNER[2]),
                0.0,
            ),
            quad(
                Rect {
                    x: width - 24.0,
                    y: height - 24.0,
                    w: 24.0,
                    h: 24.0,
                },
                srgb(FAR[0], FAR[1], FAR[2]),
                0.0,
            ),
        ],
    )
}

pub fn resize_storm() {
    if !support::appkit() {
        return;
    }
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let window = support::window("metal resize", (300.0, 200.0), (40, 80));
    let surface = gpu.attach(window, Alpha::Opaque);
    let lease = window.layer_lease().expect("a live window");
    let mut step = 0;
    let mut captures = 0;
    drive(LIMIT, move |_, _| {
        if step < STEPS {
            let frame = testing::frame(window).expect("a live window");
            let grow = (step % 7) as f64 * 23.0 - 60.0;
            testing::set_frame_size(window, frame.width + grow, frame.height - grow / 2.0);
            step += 1;
        }
        let extent = extent_of(window);
        let plan = pinned(extent);
        let read = gpu.run(move |backend| {
            backend.resize_surface(surface, extent).expect("resize");
            backend
                .wait_for_frame_slot_with_hint(surface, PresentHint::Vsync)
                .expect("a frame slot");
            backend
                .execute(&plan, &FrameInputs::new(surface))
                .expect("execute");
            let read = backend.read_surface(surface).ok();
            backend
                .present(surface, PresentHint::Vsync)
                .expect("present");
            read
        });
        if let Some(read) = read {
            assert_eq!(
                read.extent, extent,
                "step {step}: the drawable has the size the frame was planned for"
            );
            let far = ((extent.height - 1) * extent.width + extent.width - 1) as usize * 4;
            assert_eq!(
                &read.rgba8[..3],
                &CORNER,
                "step {step}: the top-left corner"
            );
            assert_eq!(
                &read.rgba8[far..far + 3],
                &FAR,
                "step {step}: the bottom-right corner"
            );
        }
        let physical = lease.geometry().physical;
        assert_eq!(
            (extent.width, extent.height),
            physical,
            "the main thread reports the size the drawable was made at"
        );
        if step < STEPS || shown(&gpu, surface) < STEPS as u64 / 2 {
            return Flow::Wait(Duration::from_millis(5));
        }
        let Some(image) = capture::window(window) else {
            return Flow::Done;
        };
        captures += 1;
        let caught_up = image.width() == extent.width && image.height() >= extent.height;
        match (caught_up, captures < CAPTURES) {
            (false, true) => return Flow::Wait(Duration::from_millis(30)),
            (false, false) => panic!(
                "after {captures} captures the window server still shows {}x{} for the {}x{} drawable",
                image.width(),
                image.height(),
                extent.width,
                extent.height
            ),
            (true, _) => {}
        }
        let (left, top) = content(&image, extent);
        let corner = pixel(&image, left + 4, top + 4);
        let far = pixel(&image, left + extent.width - 4, top + extent.height - 4);
        let middle = pixel(&image, left + extent.width / 2, top + extent.height / 2);
        match close(corner, CORNER, 3) && close(far, FAR, 3) && close(middle, GROUND, 3) {
            true => {
                gpu.assert_clean();
                Flow::Done
            }
            false if captures < CAPTURES => Flow::Wait(Duration::from_millis(30)),
            false => panic!(
                "after the storm the screen shows corner {corner:?}, far {far:?}, middle {middle:?}"
            ),
        }
    });
}
