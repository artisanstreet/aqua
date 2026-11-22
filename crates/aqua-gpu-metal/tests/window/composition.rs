use std::time::Duration;

use aqua_core::Oklch;
use aqua_gpu_metal::window::Alpha;
use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, PresentHint};
use aqua_shell_appkit::testing::{self, capture};
use aqua_shell_appkit::{Window, WindowOptions};

use super::support::{
    self, close, drive, extent_of, pixel, shown, solid, srgb, Flow, RenderThread, LIMIT,
};

pub fn premultiplied_over_window_behind() {
    if !support::appkit() {
        return;
    }
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let below = support::window("metal below", (300.0, 200.0), (40, 80));
    let below_surface = gpu.attach(below, Alpha::Opaque);
    gpu.frame(below_surface, solid(extent_of(below), srgb(220, 40, 30)));
    let above = Window::create(&WindowOptions {
        alpha_surface: true,
        ..support::options("metal above", (160.0, 100.0), (100, 140))
    })
    .expect("the transparent window is created");
    testing::set_shadow(above, false);
    let above_surface = gpu.attach(above, Alpha::Premultiplied);
    let glass = solid(extent_of(above), Oklch::from_srgb8(20, 60, 230, 0.5));
    let drawn = glass.clone();
    let read = gpu.run(move |backend| {
        backend
            .wait_for_frame_slot(above_surface)
            .expect("a frame slot");
        backend
            .execute(&drawn, &FrameInputs::new(above_surface))
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
    drive(LIMIT, move |_, _| {
        gpu.frame(above_surface, glass.clone());
        if shown(&gpu, above_surface) < 3 || shown(&gpu, below_surface) < 1 {
            return Flow::Wait(Duration::from_millis(10));
        }
        let (x, y) = testing::screen_point(above, (80.0, 50.0)).expect("a live window");
        let (under_x, under_y) = testing::screen_point(below, (20.0, 20.0)).expect("a live window");
        let Some(seen) = capture::screen(x - 2.0, y - 2.0, 4.0, 4.0) else {
            return Flow::Done;
        };
        let under = capture::screen(under_x, under_y, 4.0, 4.0).expect("the same facility");
        let (seen, under) = (pixel(&seen, 2, 2), pixel(&under, 1, 1));
        let blend = |top: u8, bottom: u8| top as f32 + bottom as f32 * (255 - a) as f32 / 255.0;
        let expected = [
            blend(r, under[0]).round() as u8,
            blend(g, under[1]).round() as u8,
            blend(b, under[2]).round() as u8,
        ];
        assert!(
            close(seen, expected, 3),
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
