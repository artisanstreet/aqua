use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant as WallInstant};

use aqua_graph::Extent;
use aqua_protocol::{Feedback, FrameId, GpuBackend, GpuError, Instant, PresentHint, SurfaceId};
use aqua_runtime::RenderSide;
use aqua_transport::{
    GpuRenderHost, HostConfig, HostCounters, Inspector, MonotonicClock, RenderHost,
};

use super::link::ExternalUpdate;
use super::lower::Lowering;

const MAINTENANCE_FRAMES: u64 = 900;

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
    pub trim: std::sync::atomic::AtomicBool,
    pub gpu_ns: AtomicU64,
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

type Maintenance<B> = Box<dyn FnMut(&mut B)>;

pub struct GpuFrames<B: GpuBackend<Error = GpuError> + 'static> {
    host: GpuRenderHost<B, Lowering, MonotonicClock>,
    surface: SurfaceId,
    stats: Arc<RenderStats>,
    hint: PresentHint,
    maintenance: Option<Maintenance<B>>,
    since_maintenance: u64,
    counted: HostCounters,
}

impl<B: GpuBackend<Error = GpuError> + 'static> GpuFrames<B> {
    pub fn new(
        backend: B,
        surface: SurfaceId,
        extent: Extent,
        lowering: Lowering,
        externals: Receiver<ExternalUpdate>,
        stats: Arc<RenderStats>,
    ) -> Self {
        let clear = lowering.clear_color();
        let lowering = lowering.with_externals(externals).with_stats(stats.clone());
        let host = GpuRenderHost::new(
            backend,
            lowering,
            MonotonicClock::from_origin(WallInstant::now()),
            HostConfig::offscreen(clear),
        )
        .adopting(surface, extent);
        Self {
            host,
            surface,
            stats,
            hint: PresentHint::Vsync,
            maintenance: None,
            since_maintenance: 0,
            counted: HostCounters::default(),
        }
    }

    pub fn with_hint(mut self, hint: PresentHint) -> Self {
        self.hint = hint;
        self
    }

    pub fn with_inspector(mut self, inspector: Inspector) -> Self {
        self.host.set_inspector(inspector);
        self
    }

    pub fn with_maintenance(mut self, maintenance: impl FnMut(&mut B) + 'static) -> Self {
        self.maintenance = Some(Box::new(maintenance));
        self
    }

    pub fn backend(&self) -> &B {
        self.host.backend()
    }

    pub fn backend_mut(&mut self) -> &mut B {
        self.host.backend_mut()
    }

    pub fn surface(&self) -> SurfaceId {
        self.surface
    }

    pub fn host(&self) -> &GpuRenderHost<B, Lowering, MonotonicClock> {
        &self.host
    }

    fn maintain(&mut self) {
        if let Some(maintenance) = self.maintenance.as_mut() {
            maintenance(self.host.backend_mut());
        }
    }

    fn account(&mut self) {
        let counters = self.host.counters();
        let stats = &self.stats;
        stats.recoveries.fetch_add(
            u64::from(counters.losses - self.counted.losses),
            Ordering::Relaxed,
        );
        if counters.failed > self.counted.failed {
            stats
                .failures
                .fetch_add(counters.failed - self.counted.failed, Ordering::Relaxed);
            if let Some(error) = self.host.last_error() {
                eprintln!("aqua: render failed: {error}");
            }
        }
        stats.frames.fetch_add(
            counters.presented - self.counted.presented,
            Ordering::Relaxed,
        );
        self.counted = counters;
        if let Some(record) = self.host.last_record() {
            RenderStats::add(
                &stats.execute_ns,
                Some(&stats.execute_max_ns),
                Duration::from_micros(record.cpu_micros),
            );
            RenderStats::add(
                &stats.present_ns,
                None,
                Duration::from_micros(record.present_queued_micros),
            );
            if let Some(gpu) = record.gpu_micros {
                stats
                    .gpu_ns
                    .store(gpu.saturating_mul(1000), Ordering::Relaxed);
            }
        }
    }
}

impl<B: GpuBackend<Error = GpuError> + 'static> Drop for GpuFrames<B> {
    fn drop(&mut self) {
        if let Some(mut maintenance) = self.maintenance.take() {
            maintenance(self.host.backend_mut());
        }
    }
}

impl<B: GpuBackend<Error = GpuError> + 'static> FrameTarget for GpuFrames<B> {
    fn paced(&self) -> bool {
        !self.host.backend().capabilities().features.window_surfaces
    }

    fn wait_frame(&mut self) {
        self.host.wait_frame();
    }

    fn present(&mut self, side: &mut RenderSide, frame: FrameId) -> Vec<Feedback> {
        let hint = match self.stats.immediate.load(Ordering::Relaxed) {
            true => PresentHint::Immediate,
            false => self.hint,
        };
        self.host.set_present(hint);
        if self.stats.trim.swap(false, Ordering::AcqRel) {
            self.host.trim_handle().request();
        }
        let feedback = self.host.execute(side, frame);
        self.account();
        self.since_maintenance += 1;
        if self.since_maintenance >= MAINTENANCE_FRAMES {
            self.since_maintenance = 0;
            self.maintain();
        }
        feedback
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
