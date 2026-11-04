use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant as WallInstant};

use aqua_graph::Extent;
use aqua_protocol::{
    DeviceStatus, Feedback, FrameId, FrameInputs, GpuBackend, GpuError, Instant, PresentHint,
    SurfaceId,
};
use aqua_runtime::RenderSide;
use aqua_transport::RenderHost;

use super::link::ExternalUpdate;
use super::lower::Lowering;

pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

pub trait FrameTarget {
    fn paced(&self) -> bool {
        false
    }
    fn wait_frame(&mut self);
    fn present(&mut self, side: &mut RenderSide, frame: FrameId) -> Vec<Feedback>;
}

#[derive(Debug, Default)]
pub struct RenderStats {
    pub frames: AtomicU64,
    pub lower_ns: AtomicU64,
    pub lower_max_ns: AtomicU64,
    pub execute_ns: AtomicU64,
    pub execute_max_ns: AtomicU64,
    pub present_ns: AtomicU64,
    pub recoveries: AtomicU64,
    pub failures: AtomicU64,
    pub skipped_items: AtomicU64,
    pub commands: AtomicU64,
    pub glyphs: AtomicU64,
    pub immediate: std::sync::atomic::AtomicBool,
    pub waits: AtomicU64,
}

impl RenderStats {
    fn add(counter: &AtomicU64, max: Option<&AtomicU64>, elapsed: Duration) {
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        counter.fetch_add(nanos, Ordering::Relaxed);
        if let Some(max) = max {
            max.fetch_max(nanos, Ordering::Relaxed);
        }
    }
}

pub struct GpuFrames<B: GpuBackend<Error = GpuError>> {
    backend: B,
    surface: SurfaceId,
    extent: Extent,
    lowering: Lowering,
    externals: Receiver<ExternalUpdate>,
    stats: Arc<RenderStats>,
    hint: PresentHint,
}

impl<B: GpuBackend<Error = GpuError>> GpuFrames<B> {
    pub fn new(
        backend: B,
        surface: SurfaceId,
        extent: Extent,
        lowering: Lowering,
        externals: Receiver<ExternalUpdate>,
        stats: Arc<RenderStats>,
    ) -> Self {
        Self {
            backend,
            surface,
            extent,
            lowering,
            externals,
            stats,
            hint: PresentHint::Vsync,
        }
    }

    pub fn with_hint(mut self, hint: PresentHint) -> Self {
        self.hint = hint;
        self
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn surface(&self) -> SurfaceId {
        self.surface
    }

    fn apply_externals(&mut self) {
        while let Ok(update) = self.externals.try_recv() {
            match update {
                ExternalUpdate::Image {
                    layer,
                    width,
                    height,
                    rgba,
                } => self.lowering.register_image(layer, width, height, &rgba),
                ExternalUpdate::Release { layer } => self.lowering.release_image(layer),
            }
        }
    }

    fn recover(&mut self) -> Vec<Feedback> {
        self.stats.recoveries.fetch_add(1, Ordering::Relaxed);
        match self.backend.recover() {
            Ok(_) => {
                self.lowering.reset();
                vec![Feedback::DeviceLost, Feedback::DeviceRecovered]
            }
            Err(error) => {
                eprintln!("aqua: device recovery failed: {error}");
                vec![Feedback::DeviceLost]
            }
        }
    }

    fn fail(&mut self, error: &GpuError) -> Vec<Feedback> {
        self.stats.failures.fetch_add(1, Ordering::Relaxed);
        eprintln!("aqua: render failed: {error}");
        match error {
            GpuError::DeviceFailure(_) => self.recover(),
            _ => Vec::new(),
        }
    }
}

fn physical(size: aqua_core::Size, scale: f32) -> Extent {
    Extent {
        width: (size.w * scale).round().max(1.0) as u32,
        height: (size.h * scale).round().max(1.0) as u32,
    }
}

impl<B: GpuBackend<Error = GpuError>> FrameTarget for GpuFrames<B> {
    fn paced(&self) -> bool {
        !self.backend.capabilities().features.window_surfaces
    }

    fn wait_frame(&mut self) {
        let _ = self.backend.wait_for_frame_slot(self.surface);
    }

    fn present(&mut self, side: &mut RenderSide, frame: FrameId) -> Vec<Feedback> {
        self.apply_externals();
        let Some((size, scale)) = side.viewport() else {
            return Vec::new();
        };
        let extent = physical(size, scale);
        if extent != self.extent {
            match self.backend.resize_surface(self.surface, extent) {
                Ok(()) => self.extent = extent,
                Err(error) => return self.fail(&error),
            }
        }
        let limits = self.backend.capabilities().plan_limits();
        let started = WallInstant::now();
        let lowered = match self.lowering.lower(side.scene(), size, scale, limits) {
            Ok(lowered) => lowered,
            Err(error) => return self.fail(&GpuError::PlanRejected(error)),
        };
        RenderStats::add(
            &self.stats.lower_ns,
            Some(&self.stats.lower_max_ns),
            started.elapsed(),
        );
        let skipped: u32 = lowered.report.skipped.values().sum();
        self.stats
            .skipped_items
            .fetch_add(u64::from(skipped), Ordering::Relaxed);
        self.stats
            .commands
            .fetch_add(u64::from(lowered.report.commands), Ordering::Relaxed);
        self.stats
            .glyphs
            .fetch_add(u64::from(lowered.report.glyphs), Ordering::Relaxed);
        if let Err(error) = self.backend.upload(lowered.uploads) {
            return self.fail(&error);
        }
        let executed = WallInstant::now();
        let report = match self
            .backend
            .execute(&lowered.plan, &FrameInputs::new(self.surface))
        {
            Ok(report) => report,
            Err(error) => return self.fail(&error),
        };
        RenderStats::add(
            &self.stats.execute_ns,
            Some(&self.stats.execute_max_ns),
            executed.elapsed(),
        );
        if report.device == DeviceStatus::Lost {
            return self.recover();
        }
        let presented = WallInstant::now();
        let hint = match self.stats.immediate.load(Ordering::Relaxed) {
            true => PresentHint::Immediate,
            false => self.hint,
        };
        match self.backend.present(self.surface, hint) {
            Ok(_) => {
                RenderStats::add(&self.stats.present_ns, None, presented.elapsed());
                self.stats.frames.fetch_add(1, Ordering::Relaxed);
                vec![Feedback::Presented(frame)]
            }
            Err(error) => self.fail(&error),
        }
    }
}

pub type TargetInit = Box<dyn FnOnce() -> Result<Box<dyn FrameTarget>, String> + Send>;

thread_local! {
    static TARGET: RefCell<Option<Box<dyn FrameTarget>>> = const { RefCell::new(None) };
}

enum Stage {
    Pending(TargetInit),
    Running,
    Dead,
}

pub struct DeferredHost {
    stats: Option<Arc<RenderStats>>,
    stage: Stage,
    clock: Clock,
    interval: Duration,
    deadline: Option<WallInstant>,
    paced: bool,
    presented_last: bool,
}

impl DeferredHost {
    pub fn new(
        init: TargetInit,
        clock: Clock,
        interval: Duration,
        stats: Option<Arc<RenderStats>>,
    ) -> Self {
        Self {
            stats,
            stage: Stage::Pending(init),
            clock,
            interval,
            deadline: None,
            paced: false,
            presented_last: false,
        }
    }

    fn start(&mut self) -> bool {
        let stage = std::mem::replace(&mut self.stage, Stage::Dead);
        match stage {
            Stage::Pending(init) => match init() {
                Ok(target) => {
                    self.paced = target.paced();
                    TARGET.with(|slot| *slot.borrow_mut() = Some(target));
                    self.stage = Stage::Running;
                    true
                }
                Err(message) => {
                    eprintln!("aqua: render target failed: {message}");
                    false
                }
            },
            Stage::Running => {
                self.stage = Stage::Running;
                true
            }
            Stage::Dead => false,
        }
    }

    fn pace(&mut self) {
        let now = WallInstant::now();
        let target = match self.deadline {
            Some(deadline) if deadline > now => deadline,
            _ => now,
        };
        if target > now {
            std::thread::sleep(target - now);
        }
        self.deadline = Some(target + self.interval);
    }
}

impl RenderHost for DeferredHost {
    fn present_time(&mut self) -> Instant {
        (self.clock)() + self.interval
    }

    fn wait_frame(&mut self) {
        if let Some(stats) = self.stats.as_ref() {
            stats.waits.fetch_add(1, Ordering::Relaxed);
        }
        match self.start() {
            true => {
                TARGET.with(|slot| {
                    if let Some(target) = slot.borrow_mut().as_mut() {
                        target.wait_frame();
                    }
                });
                if self.paced || !self.presented_last {
                    self.pace();
                }
                self.presented_last = false;
            }
            false => std::thread::sleep(self.interval),
        }
    }

    fn execute(&mut self, side: &mut RenderSide, frame: FrameId) -> Vec<Feedback> {
        let feedback = match self.start() {
            true => TARGET.with(|slot| match slot.borrow_mut().as_mut() {
                Some(target) => target.present(side, frame),
                None => Vec::new(),
            }),
            false => vec![Feedback::DeviceLost],
        };
        self.presented_last = feedback
            .iter()
            .any(|item| matches!(item, Feedback::Presented(_)));
        feedback
    }
}
