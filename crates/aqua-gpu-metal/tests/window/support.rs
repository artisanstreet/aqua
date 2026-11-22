#![allow(dead_code)]

use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_gpu_metal::scene::{OffscreenOptions, SurfaceFormat};
use aqua_gpu_metal::window::pacing::SurfacePacer;
use aqua_gpu_metal::window::{Alpha, LayerTarget, WindowBackend, WindowOptions};
use aqua_graph::{plan_paint, Extent, FramePlan, PaintCommand, Quad};
use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, PresentHint, SurfaceId};
use aqua_shell_appkit::testing::{available, Facility};
use aqua_shell_appkit::{from_media_time, LayerLease, Window, WindowOptions as ShellOptions};
use image::RgbaImage;

#[path = "../support/mod.rs"]
mod offscreen;

pub use aqua_shell_appkit::testing::cases::{case, main, Case};
pub use aqua_shell_appkit::testing::script::{drive, Flow};

pub const LIMIT: Duration = Duration::from_secs(25);
pub const CAPTURES: u32 = 8;

pub fn appkit() -> bool {
    available(Facility::WindowServer)
}

pub fn compiling() -> bool {
    offscreen::open(SurfaceFormat::Bgra8Unorm)
        .and_then(offscreen::compiling)
        .is_some()
}

pub fn options(title: &str, logical: (f64, f64), at: (i32, i32)) -> ShellOptions {
    ShellOptions {
        title: title.into(),
        logical_size: logical,
        position: Some(at),
        ..ShellOptions::default()
    }
}

pub fn window(title: &str, logical: (f64, f64), at: (i32, i32)) -> Window {
    Window::create(&options(title, logical, at)).expect("the window is created")
}

pub fn extent_of(window: Window) -> Extent {
    let (width, height) = window
        .layer_lease()
        .expect("a live window")
        .geometry()
        .physical;
    Extent { width, height }
}

pub fn srgb(r: u8, g: u8, b: u8) -> Oklch {
    Oklch::from_srgb8(r, g, b, 1.0)
}

pub fn solid(extent: Extent, color: Oklch) -> FramePlan {
    plan_paint(extent, color, &[]).expect("a clear-only plan is valid")
}

pub fn quad(bounds: Rect, fill: Oklch, radius: f32) -> PaintCommand {
    PaintCommand::Quad(Quad {
        bounds,
        radius,
        border_width: 0.0,
        fill,
        border_color: fill,
    })
}

pub fn painted(extent: Extent, clear: Oklch, commands: &[PaintCommand]) -> FramePlan {
    plan_paint(extent, clear, commands).expect("the test plan is valid")
}

pub fn offscreen(plan: &FramePlan) -> Vec<u8> {
    let mut backend = offscreen::open_with(OffscreenOptions {
        format: SurfaceFormat::Bgra8Unorm,
        ..OffscreenOptions::default()
    })
    .expect("the offscreen backend starts where the window one does");
    let surface = backend
        .create_surface(aqua_protocol::SurfaceTarget::Offscreen(plan.extent))
        .expect("surface");
    backend
        .execute(plan, &FrameInputs::new(surface))
        .expect("execute");
    backend.read_surface(surface).expect("readback").rgba8
}

pub struct LeasePacer(pub LayerLease);

impl SurfacePacer for LeasePacer {
    fn refresh(&self) -> Option<Duration> {
        self.0.geometry().refresh
    }

    fn covered(&self) -> bool {
        self.0.geometry().covered
    }

    fn presented_at(&self, media_seconds: f64) -> aqua_protocol::Instant {
        from_media_time(media_seconds)
    }
}

type Job = Box<dyn FnOnce(&mut WindowBackend) + Send>;

pub struct RenderThread {
    jobs: Option<Sender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn required() -> bool {
    std::env::var_os("AQUA_REQUIRE_GPU").is_some_and(|value| value == "1")
}

impl RenderThread {
    pub fn start() -> Option<RenderThread> {
        offscreen::open(SurfaceFormat::Bgra8Unorm)?;
        let (jobs, inbox) = channel::<Job>();
        let (ready, started) = channel();
        let thread = std::thread::spawn(move || {
            let mut backend = WindowBackend::new(WindowOptions::from_env())
                .expect("the window backend starts where the offscreen one does");
            ready
                .send(())
                .expect("the test waits for the render thread");
            while let Ok(job) = inbox.recv() {
                job(&mut backend);
            }
        });
        started.recv().expect("the render thread starts");
        Some(RenderThread {
            jobs: Some(jobs),
            thread: Some(thread),
        })
    }

    pub fn run<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut WindowBackend) -> T + Send + 'static,
    ) -> T {
        let (reply, answer) = channel();
        self.jobs
            .as_ref()
            .expect("the render thread is running")
            .send(Box::new(move |backend: &mut WindowBackend| {
                let _ = reply.send(job(backend));
            }))
            .expect("the render thread accepts jobs");
        answer.recv().expect("the render thread answers every job")
    }

    pub fn attach(&self, window: Window, alpha: Alpha) -> SurfaceId {
        let lease = window.layer_lease().expect("a live window has a lease");
        let extent = extent_of(window);
        self.run(move |backend| {
            let target = LayerTarget {
                window: lease.window(),
                layer: lease.layer(),
                extent,
            };
            backend
                .create_layer_surface(target, alpha, Box::new(LeasePacer(lease)))
                .expect("the layer becomes a surface")
        })
    }

    pub fn frame(&self, surface: SurfaceId, plan: FramePlan) -> Duration {
        self.run(move |backend| frame(backend, surface, &plan, PresentHint::Vsync))
    }

    pub fn assert_clean(&self) {
        let (validating, classes, errors) = self.run(|backend| {
            (
                backend.validating(),
                backend.metal_object_classes(),
                backend.validation_errors(),
            )
        });
        assert!(
            validating || !required(),
            "AQUA_REQUIRE_GPU=1 but Metal API validation is off ({classes})"
        );
        assert!(
            errors.is_empty(),
            "{} Metal errors, first: {}",
            errors.len(),
            errors
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

impl Drop for RenderThread {
    fn drop(&mut self) {
        drop(self.jobs.take());
        if let Some(thread) = self.thread.take() {
            let joined = thread.join();
            if !std::thread::panicking() {
                joined.expect("the render thread exits cleanly");
            }
        }
    }
}

pub fn frame(
    backend: &mut WindowBackend,
    surface: SurfaceId,
    plan: &FramePlan,
    hint: PresentHint,
) -> Duration {
    let started = std::time::Instant::now();
    backend
        .wait_for_frame_slot_with_hint(surface, hint)
        .expect("a frame slot");
    backend
        .execute(plan, &FrameInputs::new(surface))
        .expect("execute");
    backend.present(surface, hint).expect("present");
    started.elapsed()
}

pub fn shown(gpu: &RenderThread, surface: SurfaceId) -> u64 {
    let report = gpu.run(move |backend| backend.pacing(surface).expect("a live surface"));
    report.frames + report.discarded
}

pub fn content(capture: &RgbaImage, extent: Extent) -> (u32, u32) {
    assert!(
        capture.width() >= extent.width && capture.height() >= extent.height,
        "the {}x{} capture holds the {}x{} content",
        capture.width(),
        capture.height(),
        extent.width,
        extent.height
    );
    (
        (capture.width() - extent.width) / 2,
        capture.height() - extent.height,
    )
}

pub fn pixel(capture: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let [r, g, b, _] = capture.get_pixel(x, y).0;
    [r, g, b]
}

pub fn close(left: [u8; 3], right: [u8; 3], tolerance: u8) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| left.abs_diff(right) <= tolerance)
}

const WINDOW_CORNER: u32 = 12;

fn under_window_chrome(x: u32, y: u32, extent: Extent) -> bool {
    let bottom_corner = y + WINDOW_CORNER >= extent.height
        && (x < WINDOW_CORNER || x + WINDOW_CORNER >= extent.width);
    y == 0 || bottom_corner
}

pub fn mismatches(
    capture: &RgbaImage,
    expected: &[u8],
    extent: Extent,
    tolerance: u8,
) -> Vec<(u32, u32)> {
    let (left, top) = content(capture, extent);
    (0..extent.height)
        .flat_map(|y| (0..extent.width).map(move |x| (x, y)))
        .filter(|(x, y)| !under_window_chrome(*x, *y, extent))
        .filter(|(x, y)| {
            let offset = ((y * extent.width + x) * 4) as usize;
            let wanted = [expected[offset], expected[offset + 1], expected[offset + 2]];
            !close(pixel(capture, left + x, top + y), wanted, tolerance)
        })
        .collect()
}
