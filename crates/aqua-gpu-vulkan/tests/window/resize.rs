use std::time::Duration;

use aqua_core::geometry::Rect;
use aqua_gpu_vulkan::window::Alpha;
use aqua_graph::Extent;
use aqua_protocol::{GpuBackend, PresentHint, SurfaceId};
use aqua_shell_wayland::testing::{capture, Options};
use aqua_shell_wayland::{ShellEvent, Window};

use super::support::{
    self, bgr, close, drive, extent_of, frame, mapped, painted, quad, shown, srgb, Flow,
    RenderThread,
};

fn render_at(gpu: &RenderThread, surface: SurfaceId, extent: Extent) -> Extent {
    let plan = support::solid(extent, srgb(70, 110, 160));
    gpu.run(move |backend| {
        if backend.surface_extent(surface) != Some(extent) {
            backend
                .resize_surface(surface, extent)
                .expect("the swapchain is recreated");
        }
        frame(backend, surface, &plan, PresentHint::Vsync, None);
        backend.surface_extent(surface).expect("a live surface")
    })
}

#[test]
fn a_resize_storm_leaves_a_swapchain_of_the_final_window_size() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let app = "com.aqua.test.vulkan.storm";
    let window = support::window(&compositor, app, (320.0, 200.0), (20, 20));
    let surface = gpu.attach(window, Alpha::Opaque);
    render_at(&gpu, surface, extent_of(window));
    let sizes = [
        (400, 300),
        (401, 299),
        (180, 120),
        (640, 360),
        (333, 222),
        (250, 400),
        (251, 401),
        (500, 260),
        (300, 180),
        (420, 310),
    ];
    let probe = compositor.clone();
    let mut sent = 0;
    drive(Duration::from_secs(30), move |context| {
        let Some(placed) = mapped(&probe, app) else {
            return Flow::Wait(Duration::from_millis(10));
        };
        let rendered = render_at(&gpu, surface, extent_of(window));
        assert_eq!(rendered, extent_of(window));
        if sent < sizes.len() {
            let (width, height) = sizes[sent];
            probe
                .command(&format!(
                    "[app_id=\"{app}\"] resize set width {width} px height {height} px"
                ))
                .expect("sway resizes the floating window");
            sent += 1;
            return Flow::Wait(Duration::from_millis(2));
        }
        let last = sizes[sizes.len() - 1];
        let settled = context.count(|event| {
            *event
                == ShellEvent::Resized {
                    width: last.0,
                    height: last.1,
                }
        }) > 0;
        let presented = shown(&gpu, surface);
        if !settled || (placed.width, placed.height) != (last.0 as i32, last.1 as i32) {
            return Flow::Wait(Duration::from_millis(10));
        }
        let before = presented;
        render_at(&gpu, surface, extent_of(window));
        if shown(&gpu, surface) <= before {
            return Flow::Wait(Duration::from_millis(10));
        }
        let capture = capture(&probe, false).expect("screencopy");
        let corner = bgr(
            &capture,
            placed.x + placed.width - 1,
            placed.y + placed.height - 1,
        );
        assert!(
            close(corner, [70, 110, 160], 1),
            "the bottom-right pixel of the final size is painted: {corner:?}"
        );
        assert_eq!(
            gpu.run(move |backend| backend.surface_extent(surface)),
            Some(Extent {
                width: last.0,
                height: last.1
            })
        );
        gpu.assert_clean();
        Flow::Done
    });
}

#[test]
fn at_scale_one_and_a_half_the_buffer_is_physical_and_lines_stay_one_device_pixel() {
    let Some(compositor) = support::compositor() else {
        return;
    };
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    compositor
        .command("output HEADLESS-1 scale 1.5")
        .expect("sway scales the output");
    let app = "com.aqua.test.vulkan.fractional";
    let window = support::window(&compositor, app, (200.0, 120.0), (0, 0));
    let surface = gpu.attach(window, Alpha::Opaque);
    let background = srgb(250, 250, 250);
    let ink = srgb(10, 10, 10);
    let lined = move |extent: Extent| {
        painted(
            extent,
            background,
            &[
                quad(
                    Rect {
                        x: 21.0,
                        y: 0.0,
                        w: 1.0,
                        h: extent.height as f32,
                    },
                    ink,
                    0.0,
                ),
                quad(
                    Rect {
                        x: 0.0,
                        y: 13.0,
                        w: extent.width as f32,
                        h: 1.0,
                    },
                    ink,
                    0.0,
                ),
            ],
        )
    };
    gpu.frame(surface, lined(extent_of(window)));
    let probe = compositor.clone();
    drive(Duration::from_secs(15), move |context| {
        let scaled = context.count(|event| *event == ShellEvent::ScaleChanged { scale: 1.5 }) > 0;
        let Some(placed) = mapped(&probe, app).filter(|_| scaled) else {
            return Flow::Wait(Duration::from_millis(10));
        };
        let extent = extent_of(window);
        assert_eq!(
            (extent.width, extent.height),
            (300, 180),
            "the buffer is physical: logical 200x120 at 1.5"
        );
        if (placed.x, placed.y, placed.width, placed.height) != (0, 0, 200, 120) {
            return Flow::Wait(Duration::from_millis(10));
        }
        let before = shown(&gpu, surface);
        let plan = lined(extent);
        gpu.run(move |backend| {
            backend
                .resize_surface(surface, extent)
                .expect("the swapchain follows the scale");
            frame(backend, surface, &plan, PresentHint::Vsync, None);
        });
        if shown(&gpu, surface) <= before {
            return Flow::Wait(Duration::from_millis(10));
        }
        let capture = capture(&probe, false).expect("screencopy");
        assert_eq!((capture.width, capture.height), (1280, 800));
        let dark = [10, 10, 10];
        let light = [250, 250, 250];
        assert!(close(bgr(&capture, 21, 60), dark, 1));
        assert!(close(bgr(&capture, 20, 60), light, 1));
        assert!(close(bgr(&capture, 22, 60), light, 1));
        assert!(close(bgr(&capture, 100, 13), dark, 1));
        assert!(close(bgr(&capture, 100, 12), light, 1));
        assert!(close(bgr(&capture, 100, 14), light, 1));
        assert!(
            close(bgr(&capture, 299, 179), light, 1) && !close(bgr(&capture, 301, 181), light, 1),
            "the window covers exactly 300x180 device pixels"
        );
        gpu.assert_clean();
        Flow::Done
    });
}

#[test]
fn an_odd_sized_buffer_that_lags_an_integer_scale_change_never_breaks_the_connection() {
    let Some(compositor) = support::compositor_with(Options {
        hidden_globals: vec!["wp_fractional_scale_manager_v1".into()],
        ..Options::default()
    }) else {
        return;
    };
    let Some(gpu) = RenderThread::start() else {
        return;
    };
    let app = "com.aqua.test.vulkan.lagging";
    let window: Window = support::window(&compositor, app, (201.0, 121.0), (20, 20));
    let surface = gpu.attach(window, Alpha::Opaque);
    let stale = extent_of(window);
    gpu.frame(surface, support::solid(stale, srgb(30, 30, 30)));
    let probe = compositor.clone();
    let mut asked = false;
    drive(Duration::from_secs(15), move |context| {
        if mapped(&probe, app).is_none() {
            return Flow::Wait(Duration::from_millis(10));
        }
        if !asked {
            probe
                .command("output HEADLESS-1 scale 2")
                .expect("sway scales the output");
            asked = true;
        }
        if context.count(|event| *event == ShellEvent::ScaleChanged { scale: 2.0 }) == 0 {
            return Flow::Wait(Duration::from_millis(10));
        }
        for _ in 0..3 {
            gpu.frame(surface, support::solid(stale, srgb(30, 30, 30)));
        }
        let fresh = render_at(&gpu, surface, extent_of(window));
        assert_eq!(fresh, extent_of(window));
        assert!(
            window.is_visible(),
            "the connection survived the stale frames"
        );
        gpu.assert_clean();
        Flow::Done
    });
}
