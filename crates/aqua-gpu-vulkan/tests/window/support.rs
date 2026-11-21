#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{channel, Sender};
use std::time::Duration;

use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_gpu_vulkan::hal::Validation;
use aqua_gpu_vulkan::scene::{AdapterPreference, DeviceOptions, SurfaceFormat};
use aqua_gpu_vulkan::window::pacing::{FrameWait, PacerError, Presentation, SurfacePacer};
use aqua_gpu_vulkan::window::{Alpha, WindowBackend, WindowOptions};
use aqua_graph::{plan_paint, Extent, FramePlan, PaintCommand, Quad};
use aqua_protocol::{FrameInputs, GpuBackend, PresentHint, SurfaceId, WaylandSurface, WindowId};
use aqua_shell::AppId;
use aqua_shell_wayland::testing::{Capture, Compositor, Options};
use aqua_shell_wayland::{
    now, quit, run, Handler, ShellEvent, SurfaceLease, Window, WindowOptions as ShellOptions,
};

#[path = "../support/mod.rs"]
mod offscreen;

pub use aqua_shell_wayland::testing::Rect as ScreenRect;

pub fn compositor() -> Option<Rc<Compositor>> {
    compositor_with(Options::default())
}

pub fn compositor_with(options: Options) -> Option<Rc<Compositor>> {
    let compositor = Compositor::start_with(options)?;
    compositor
        .attach()
        .expect("the shell connects to the headless compositor");
    Some(Rc::new(compositor))
}

pub fn floating(compositor: &Compositor, app_id: &str) {
    compositor
        .command(&format!(
            "for_window [app_id=\"^{app_id}$\"] floating enable"
        ))
        .expect("sway accepts the floating rule");
}

pub fn options(app_id: &str, logical: (f64, f64)) -> ShellOptions {
    ShellOptions {
        title: app_id.into(),
        logical_size: logical,
        app_id: Some(AppId::new(app_id).expect("test app ids are valid")),
        ..ShellOptions::default()
    }
}

struct Placement {
    compositor: Rc<Compositor>,
    app_id: String,
    at: (i32, i32),
}

thread_local! {
    static PLACEMENTS: RefCell<Vec<Placement>> = const { RefCell::new(Vec::new()) };
}

pub fn window(
    compositor: &Rc<Compositor>,
    app_id: &str,
    logical: (f64, f64),
    at: (i32, i32),
) -> Window {
    floating(compositor, app_id);
    PLACEMENTS.with_borrow_mut(|placements| {
        placements.push(Placement {
            compositor: compositor.clone(),
            app_id: app_id.into(),
            at,
        })
    });
    Window::create(&options(app_id, logical)).expect("the window is created")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Placing {
    Mapping,
    Moving,
    Placed,
}

fn place(placements: &[Placement], placing: Placing) -> Placing {
    let rects: Vec<Option<ScreenRect>> = placements
        .iter()
        .map(|placement| mapped(&placement.compositor, &placement.app_id))
        .collect();
    match placing {
        Placing::Mapping if rects.iter().all(Option::is_some) => {
            placements.iter().for_each(|placement| {
                placement
                    .compositor
                    .command(&format!(
                        "[app_id=\"^{}$\"] move position {} {}",
                        placement.app_id, placement.at.0, placement.at.1
                    ))
                    .expect("sway moves the floating window");
            });
            Placing::Moving
        }
        Placing::Moving
            if placements.iter().zip(&rects).all(|(placement, rect)| {
                rect.is_some_and(|rect| (rect.x, rect.y) == placement.at)
            }) =>
        {
            Placing::Placed
        }
        unchanged => unchanged,
    }
}

pub fn mapped(compositor: &Compositor, app_id: &str) -> Option<ScreenRect> {
    compositor
        .view_rect(app_id)
        .filter(|rect| rect.width > 0 && rect.height > 0)
}

pub fn extent_of(window: Window) -> Extent {
    let (width, height) = window.inner_size();
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

pub struct LeasePacer(pub SurfaceLease);

impl SurfacePacer for LeasePacer {
    fn before_present(&mut self, buffer: Extent) {
        self.0.before_present((buffer.width, buffer.height));
    }

    fn wait_ready(&mut self, timeout: Duration) -> Result<FrameWait, PacerError> {
        match self.0.wait_ready(timeout) {
            Ok(aqua_shell_wayland::FrameWait::Ready) => Ok(FrameWait::Ready),
            Ok(aqua_shell_wayland::FrameWait::TimedOut) => Ok(FrameWait::TimedOut),
            Err(error) => Err(PacerError {
                reason: error.to_string(),
            }),
        }
    }

    fn take_presented(&mut self) -> Vec<Presentation> {
        self.0
            .take_presented()
            .into_iter()
            .map(|presented| match presented {
                aqua_shell_wayland::Presented::Shown { at, refresh, .. } => {
                    Presentation::Shown { at, refresh }
                }
                aqua_shell_wayland::Presented::Discarded => Presentation::Discarded,
            })
            .collect()
    }

    fn refresh(&self) -> Option<Duration> {
        self.0.geometry().refresh
    }
}

type Job = Box<dyn FnOnce(&mut WindowBackend) + Send>;

pub struct RenderThread {
    jobs: Option<Sender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

pub fn device_options() -> DeviceOptions {
    DeviceOptions {
        preference: AdapterPreference::Software,
        validation: Validation::On,
        ..DeviceOptions::default()
    }
}

impl RenderThread {
    pub fn start() -> Option<RenderThread> {
        offscreen::open(AdapterPreference::Software, SurfaceFormat::Bgra8Unorm)?;
        let (jobs, inbox) = channel::<Job>();
        let (ready, started) = channel();
        let thread = std::thread::spawn(move || {
            let mut backend = WindowBackend::new(WindowOptions {
                device: device_options(),
                ..WindowOptions::default()
            })
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
        let lease = window.surface_lease().expect("a live window has a lease");
        let target = WaylandSurface {
            window: window.id(),
            display: lease.display_handle(),
            surface: lease.surface_handle(),
            extent: extent_of(window),
        };
        self.run(move |backend| {
            backend
                .create_wayland_surface(target, alpha, Box::new(LeasePacer(lease)))
                .expect("the Wayland surface gets a swapchain")
        })
    }

    pub fn frame(&self, surface: SurfaceId, plan: FramePlan) -> Duration {
        self.run(move |backend| frame(backend, surface, &plan, PresentHint::Vsync, None))
    }

    pub fn damaged(&self, surface: SurfaceId, plan: FramePlan, damage: Vec<Rect>) -> Duration {
        self.run(move |backend| frame(backend, surface, &plan, PresentHint::Vsync, Some(&damage)))
    }

    pub fn assert_clean(&self) {
        let (validating, errors) =
            self.run(|backend| (backend.validating(), backend.validation_errors()));
        assert!(validating, "the validation layer is active");
        assert!(
            errors.is_empty(),
            "{} validation errors, first: {}",
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
    damage: Option<&[Rect]>,
) -> Duration {
    let started = std::time::Instant::now();
    backend
        .wait_for_frame_slot_with_hint(surface, hint)
        .expect("a frame slot");
    let inputs = FrameInputs::new(surface);
    match damage {
        Some(rects) => {
            backend
                .execute_damaged(plan, &inputs, rects)
                .expect("execute");
            backend
                .present_with_damage(surface, hint, rects)
                .expect("present");
        }
        None => {
            backend.execute(plan, &inputs).expect("execute");
            backend.present(surface, hint).expect("present");
        }
    }
    started.elapsed()
}

pub fn shown(gpu: &RenderThread, surface: SurfaceId) -> u64 {
    gpu.run(move |backend| backend.pacing(surface).expect("a live surface").frames)
}

pub enum Flow {
    Wait(Duration),
    Done,
}

pub type Entry = (WindowId, ShellEvent);

pub struct Context<'a> {
    pub log: &'a [Entry],
}

impl Context<'_> {
    pub fn count(&self, wanted: impl Fn(&ShellEvent) -> bool) -> usize {
        self.log.iter().filter(|(_, event)| wanted(event)).count()
    }
}

struct Driver<F> {
    placements: Vec<Placement>,
    placing: Placing,
    log: Rc<RefCell<Vec<Entry>>>,
    step: F,
    limit: aqua_protocol::Instant,
    timed_out: Rc<RefCell<bool>>,
}

impl<F: FnMut(&Context<'_>) -> Flow + 'static> Handler for Driver<F> {
    fn window_event(&mut self, id: WindowId, event: ShellEvent) {
        self.log.borrow_mut().push((id, event));
    }

    fn tick(&mut self, at: aqua_protocol::Instant) -> Option<aqua_protocol::Instant> {
        if at >= self.limit {
            *self.timed_out.borrow_mut() = true;
            quit();
            return None;
        }
        if self.placing != Placing::Placed {
            self.placing = place(&self.placements, self.placing);
            return Some(at + Duration::from_millis(10));
        }
        let log = self.log.borrow().clone();
        match (self.step)(&Context { log: &log }) {
            Flow::Wait(delay) => Some(at + delay),
            Flow::Done => {
                quit();
                None
            }
        }
    }
}

pub fn drive(limit: Duration, step: impl FnMut(&Context<'_>) -> Flow + 'static) -> Vec<Entry> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let timed_out = Rc::new(RefCell::new(false));
    run(Driver {
        placements: PLACEMENTS.with_borrow_mut(std::mem::take),
        placing: Placing::Mapping,
        log: log.clone(),
        step,
        limit: now() + limit,
        timed_out: timed_out.clone(),
    })
    .expect("the loop runs until the script quits");
    let events = log.borrow().clone();
    assert!(
        !*timed_out.borrow(),
        "the script did not finish within {limit:?}; events: {events:#?}"
    );
    events
}

pub fn bgr(capture: &Capture, x: i32, y: i32) -> [u8; 3] {
    let [b, g, r, _] = capture.bgra_at(x as u32, y as u32).unwrap_or_else(|| {
        panic!(
            "({x}, {y}) lies inside the {}x{} capture",
            capture.width, capture.height
        )
    });
    [r, g, b]
}

pub fn close(left: [u8; 3], right: [u8; 3], tolerance: u8) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| left.abs_diff(right) <= tolerance)
}
