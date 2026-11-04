use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant as WallInstant};

use aqua_core::Rect;
use aqua_graph::Extent;
use aqua_protocol::{Feedback, FrameId, GpuBackend, GpuError, Instant, PresentHint, SurfaceId};
use aqua_runtime::RenderSide;
use aqua_text::SharedRuns;
use aqua_transport::{
    GpuRenderHost, HostConfig, HostCounters, Inspector, MonotonicClock, RenderHost,
};

use super::damage::{DamagePlanner, Plan};
use super::link::ExternalUpdate;
use super::lower::Lowering;

const MAINTENANCE_FRAMES: u64 = 900;
const PENDING_POLL: Duration = Duration::from_millis(33);

pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repaint {
    #[default]
    Settled,
    NextFrame,
    Poll(Duration),
}

pub type DamageSlot = Rc<RefCell<Option<Vec<Rect>>>>;

pub trait FrameTarget {
    fn paced(&self) -> bool {
        false
    }
    fn wait_frame(&mut self);
    fn present(&mut self, side: &mut RenderSide, frame: FrameId) -> Vec<Feedback>;
    fn invalidate(&mut self) {}
    fn repaint(&self) -> Repaint {
        Repaint::Settled
    }
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
    pub arrivals: AtomicU64,
    pub forced: AtomicU64,
    pub partial_frames: AtomicU64,
    pub full_frames: AtomicU64,
    pub damaged_pixels: AtomicU64,
    pub trims: AtomicU64,
    pub attached: AtomicU64,
    pub service_thread: AtomicU64,
    pub device_sharers: AtomicU64,
    pub inject_loss: std::sync::atomic::AtomicBool,
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

struct DamageStage {
    planner: DamagePlanner,
    slot: DamageSlot,
    runs: SharedRuns,
}

pub struct GpuFrames<B: GpuBackend<Error = GpuError> + 'static> {
    host: GpuRenderHost<B, Lowering, MonotonicClock>,
    surface: SurfaceId,
    stats: Arc<RenderStats>,
    hint: PresentHint,
    maintenance: Option<Maintenance<B>>,
    since_maintenance: u64,
    counted: HostCounters,
    damage: Option<DamageStage>,
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
            damage: None,
        }
    }

    pub fn with_damage(mut self, slot: DamageSlot, runs: SharedRuns) -> Self {
        self.damage = Some(DamageStage {
            planner: DamagePlanner::new(),
            slot,
            runs,
        });
        self
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

    fn stage_damage(&mut self, side: &RenderSide) {
        let Some(stage) = self.damage.as_mut() else {
            return;
        };
        let plan = match side.viewport() {
            Some((size, scale)) => stage.planner.plan(side.scene(), &stage.runs, size, scale),
            None => Plan::Full,
        };
        let stats = &self.stats;
        match &plan {
            Plan::Full => {
                stats.full_frames.fetch_add(1, Ordering::Relaxed);
            }
            Plan::Regions(rects) => {
                stats.partial_frames.fetch_add(1, Ordering::Relaxed);
                let pixels: f64 = rects.iter().map(|r| f64::from(r.w) * f64::from(r.h)).sum();
                stats
                    .damaged_pixels
                    .fetch_add(pixels as u64, Ordering::Relaxed);
            }
        }
        *stage.slot.borrow_mut() = match plan {
            Plan::Full => None,
            Plan::Regions(rects) => Some(rects),
        };
    }

    fn settle_damage(&mut self, feedback: &[Feedback]) {
        let Some(stage) = self.damage.as_mut() else {
            return;
        };
        *stage.slot.borrow_mut() = None;
        let clean = feedback
            .iter()
            .any(|item| matches!(item, Feedback::Presented(_)))
            && !feedback
                .iter()
                .any(|item| matches!(item, Feedback::DeviceLost));
        match clean {
            true => stage.planner.commit(),
            false => stage.planner.invalidate(),
        }
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
            self.stats.trims.fetch_add(1, Ordering::Relaxed);
        }
        self.stage_damage(side);
        let feedback = self.host.execute(side, frame);
        self.settle_damage(&feedback);
        self.account();
        self.since_maintenance += 1;
        if self.since_maintenance >= MAINTENANCE_FRAMES {
            self.since_maintenance = 0;
            self.maintain();
        }
        feedback
    }

    fn invalidate(&mut self) {
        if let Some(stage) = self.damage.as_mut() {
            stage.planner.invalidate();
        }
    }

    fn repaint(&self) -> Repaint {
        let report = self.host.lowering().last_report();
        match (report.pulsing || report.shimmering, report.pending > 0) {
            (true, _) => Repaint::NextFrame,
            (false, true) => Repaint::Poll(PENDING_POLL),
            (false, false) => Repaint::Settled,
        }
    }
}

pub type TargetInit = Box<dyn FnOnce() -> Result<Box<dyn FrameTarget>, String> + Send>;
