use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aqua_core::{Oklch, Size};
use aqua_graph::{plan_paint_with, Extent};
use aqua_protocol::{
    DeviceStatus, Feedback, FrameId, FrameInputs, FrameReport, GpuBackend, GpuError, Instant,
    PresentHint, SurfaceId, SurfaceTarget,
};
use aqua_runtime::RenderSide;

use crate::inspector::{FrameRecord, Inspector};
use crate::lower::{LowerContext, Lowering, TrimRequest};
use crate::thread::RenderHost;
use crate::trim::{TrimAction, TrimInput, TrimPolicy};

pub trait Clock: 'static {
    fn now(&mut self) -> Instant;
}

#[derive(Clone, Copy, Debug)]
pub struct MonotonicClock {
    origin: std::time::Instant,
}

impl MonotonicClock {
    #[must_use]
    pub fn from_origin(origin: std::time::Instant) -> Self {
        Self { origin }
    }
}

impl Clock for MonotonicClock {
    fn now(&mut self) -> Instant {
        Instant::from_nanos(u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX))
    }
}

#[derive(Clone, Debug, Default)]
pub struct SharedClock {
    nanos: Arc<AtomicU64>,
}

impl SharedClock {
    pub fn set(&self, now: Instant) {
        self.nanos.store(now.as_nanos(), Ordering::SeqCst);
    }

    pub fn advance(&self, by: Duration) {
        self.nanos.fetch_add(
            u64::try_from(by.as_nanos()).unwrap_or(u64::MAX),
            Ordering::SeqCst,
        );
    }
}

impl Clock for SharedClock {
    fn now(&mut self) -> Instant {
        Instant::from_nanos(self.nanos.load(Ordering::SeqCst))
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SurfaceKind {
    Offscreen,
    Native {
        window: aqua_protocol::WindowId,
        hwnd: u64,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct HostConfig {
    pub surface: SurfaceKind,
    pub clear: Oklch,
    pub present: PresentHint,
    pub frame_interval: Duration,
}

impl HostConfig {
    #[must_use]
    pub fn offscreen(clear: Oklch) -> Self {
        Self {
            surface: SurfaceKind::Offscreen,
            clear,
            present: PresentHint::Vsync,
            frame_interval: Duration::from_nanos(16_666_667),
        }
    }

    #[must_use]
    pub fn native(window: aqua_protocol::WindowId, hwnd: u64, clear: Oklch) -> Self {
        Self {
            surface: SurfaceKind::Native { window, hwnd },
            ..Self::offscreen(clear)
        }
    }

    fn target(&self, extent: Extent) -> SurfaceTarget {
        match self.surface {
            SurfaceKind::Offscreen => SurfaceTarget::Offscreen(extent),
            SurfaceKind::Native { window, hwnd } => {
                SurfaceTarget::Native(aqua_protocol::NativeSurface {
                    window,
                    hwnd,
                    extent,
                })
            }
        }
    }
}

struct Live {
    surface: SurfaceId,
    extent: Extent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostCounters {
    pub presented: u64,
    pub failed: u64,
    pub losses: u32,
    pub trimmed: u64,
}

#[derive(Clone, Debug, Default)]
pub struct TrimHandle {
    requested: Arc<AtomicBool>,
}

impl TrimHandle {
    pub fn request(&self) {
        self.requested.store(true, Ordering::Release);
    }

    fn take(&self) -> bool {
        self.requested.swap(false, Ordering::AcqRel)
    }
}

pub struct GpuRenderHost<B: GpuBackend<Error = GpuError>, L: Lowering, C: Clock> {
    backend: B,
    lowering: L,
    clock: C,
    config: HostConfig,
    inspector: Option<Inspector>,
    live: Option<Live>,
    slot_acquired: bool,
    lost: bool,
    presented: u64,
    failed: u64,
    losses: u32,
    trimmed: u64,
    trim: TrimPolicy,
    trim_handle: TrimHandle,
    last: Option<FrameRecord>,
    last_error: Option<String>,
}

fn extent_of(size: Size, scale: f32) -> Extent {
    let px = |logical: f32| (logical * scale).round().max(1.0) as u32;
    Extent {
        width: px(size.w),
        height: px(size.h),
    }
}

impl<B, L, C> GpuRenderHost<B, L, C>
where
    B: GpuBackend<Error = GpuError> + 'static,
    L: Lowering,
    C: Clock,
{
    pub fn new(backend: B, lowering: L, clock: C, config: HostConfig) -> Self {
        Self {
            backend,
            lowering,
            clock,
            config,
            inspector: None,
            live: None,
            slot_acquired: false,
            lost: false,
            presented: 0,
            failed: 0,
            losses: 0,
            trimmed: 0,
            trim: TrimPolicy::new(),
            trim_handle: TrimHandle::default(),
            last: None,
            last_error: None,
        }
    }

    #[must_use]
    pub fn adopting(mut self, surface: SurfaceId, extent: Extent) -> Self {
        self.live = Some(Live { surface, extent });
        self
    }

    #[must_use]
    pub fn trim_handle(&self) -> TrimHandle {
        self.trim_handle.clone()
    }

    #[must_use]
    pub fn counters(&self) -> HostCounters {
        HostCounters {
            presented: self.presented,
            failed: self.failed,
            losses: self.losses,
            trimmed: self.trimmed,
        }
    }

    #[must_use]
    pub fn last_record(&self) -> Option<&FrameRecord> {
        self.last.as_ref()
    }

    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn set_present(&mut self, hint: PresentHint) {
        self.config.present = hint;
    }

    #[must_use]
    pub fn lowering(&self) -> &L {
        &self.lowering
    }

    pub fn lowering_mut(&mut self) -> &mut L {
        &mut self.lowering
    }

    #[must_use]
    pub fn with_inspector(mut self, inspector: Inspector) -> Self {
        self.inspector = Some(inspector);
        self
    }

    pub fn set_inspector(&mut self, inspector: Inspector) {
        self.inspector = Some(inspector);
    }

    #[must_use]
    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    #[must_use]
    pub fn surface(&self) -> Option<SurfaceId> {
        self.live.as_ref().map(|live| live.surface)
    }

    fn ensure_surface(&mut self, extent: Extent) -> Result<SurfaceId, GpuError> {
        match &mut self.live {
            Some(live) if live.extent == extent => Ok(live.surface),
            Some(live) => {
                match self.backend.resize_surface(live.surface, extent) {
                    Ok(()) => {}
                    Err(_) => {
                        let _ = self.backend.destroy_surface(live.surface);
                        live.surface = self.backend.create_surface(self.config.target(extent))?;
                    }
                }
                live.extent = extent;
                Ok(live.surface)
            }
            None => {
                let surface = self.backend.create_surface(self.config.target(extent))?;
                self.live = Some(Live { surface, extent });
                Ok(surface)
            }
        }
    }

    fn acquire_slot(&mut self, surface: SurfaceId) -> Result<(), GpuError> {
        if self.slot_acquired {
            return Ok(());
        }
        self.backend.wait_for_frame_slot(surface)?;
        self.slot_acquired = true;
        Ok(())
    }

    fn recover(&mut self) -> Option<Feedback> {
        match self.backend.recover() {
            Ok(report) => {
                self.lost = false;
                self.slot_acquired = false;
                if report.rebuilt_surfaces == 0 {
                    self.live = None;
                }
                self.lowering.device_recovered();
                Some(Feedback::DeviceRecovered)
            }
            Err(_) => None,
        }
    }

    fn render(&mut self, side: &mut RenderSide) -> Result<(FrameReport, FrameRecord), GpuError> {
        let (size, scale) = side
            .viewport()
            .ok_or(GpuError::Unsupported("frame before viewport"))?;
        let extent = extent_of(size, scale);
        let surface = self.ensure_surface(extent)?;
        self.lowering.sync();
        let context = LowerContext {
            size,
            scale,
            now: self.clock.now(),
        };
        let lowered = self.lowering.lower(side.scene(), &context);
        let uploaded = !lowered.uploads.textures.is_empty();
        if uploaded {
            self.backend.upload(lowered.uploads.clone())?;
        }
        let plan = plan_paint_with(
            extent,
            self.config.clear,
            &lowered.commands,
            self.backend.capabilities().plan_limits(),
        )?;
        self.acquire_slot(surface)?;
        let report = self.backend.execute(&plan, &FrameInputs::new(surface))?;
        let times = self.backend.present(surface, self.config.present)?;
        self.slot_acquired = false;
        self.lowering.frame_reported(&report);
        let trimmed = self.trim_after(&report, uploaded);
        let record = FrameRecord {
            frame: 0,
            extent: (extent.width, extent.height),
            passes: report.passes,
            instances: report.instances,
            barriers: report.barriers,
            transient_bytes: report.transient_bytes,
            commands: u32::try_from(lowered.commands.len()).unwrap_or(u32::MAX),
            unsupported_items: lowered.unsupported,
            approximated_items: lowered.approximated,
            glyphs: lowered.glyphs,
            trimmed_entries: trimmed,
            cpu_micros: u64::try_from(report.cpu_time.as_micros()).unwrap_or(u64::MAX),
            gpu_micros: report
                .gpu_time
                .map(|t| u64::try_from(t.as_micros()).unwrap_or(u64::MAX)),
            present_queued_micros: u64::try_from(times.queued.as_micros()).unwrap_or(u64::MAX),
        };
        Ok((report, record))
    }

    fn trim_after(&mut self, report: &FrameReport, uploaded: bool) -> u64 {
        let action = self.trim.observe(TrimInput {
            uploaded,
            cpu_time: report.cpu_time,
            gpu_time: report.gpu_time,
            budget: self.config.frame_interval,
            requested: self.trim_handle.take(),
        });
        let freed = match action {
            TrimAction::Keep => 0,
            TrimAction::Unused(frames) => self.lowering.trim(TrimRequest::IdleFrames(frames)),
            TrimAction::Everything => self.lowering.trim(TrimRequest::Everything),
        };
        let freed = u64::try_from(freed).unwrap_or(u64::MAX);
        self.trimmed += freed;
        freed
    }

    fn publish(&self, side: &RenderSide, record: Option<FrameRecord>, error: Option<String>) {
        let Some(inspector) = &self.inspector else {
            return;
        };
        let (presented, failed, losses) = (self.presented, self.failed, self.losses);
        let (items, tracks) = (side.scene().len(), side.tracks().len());
        inspector.update(|snapshot| {
            snapshot.frames_presented = presented;
            snapshot.frames_failed = failed;
            snapshot.device_losses = losses;
            snapshot.scene_items = items;
            snapshot.tracks = tracks;
            if let Some(record) = record {
                snapshot.last = Some(record);
            }
            if error.is_some() {
                snapshot.last_error = error;
            }
        });
    }
}

impl<B, L, C> RenderHost for GpuRenderHost<B, L, C>
where
    B: GpuBackend<Error = GpuError> + 'static,
    L: Lowering,
    C: Clock,
{
    fn present_time(&mut self) -> Instant {
        self.clock.now() + self.config.frame_interval
    }

    fn wait_frame(&mut self) {
        if self.lost {
            return;
        }
        if let Some(surface) = self.surface() {
            if self.acquire_slot(surface).is_err() {
                self.slot_acquired = false;
            }
        }
    }

    fn execute(&mut self, side: &mut RenderSide, frame: FrameId) -> Vec<Feedback> {
        let mut feedback = Vec::new();
        if self.lost {
            match self.recover() {
                Some(recovered) => feedback.push(recovered),
                None => return feedback,
            }
        }
        let outcome = self.render(side);
        side.scene_mut().drain_changes();
        side.scene_mut().drain_removed();
        side.scene_mut().take_structure_changed();
        match outcome {
            Ok((report, mut record)) => {
                record.frame = frame;
                self.last = Some(record.clone());
                self.presented += 1;
                feedback.push(Feedback::Presented(frame));
                if report.device == DeviceStatus::Lost {
                    self.lost = true;
                    self.losses += 1;
                    feedback.push(Feedback::DeviceLost);
                }
                self.publish(side, Some(record), None);
            }
            Err(GpuError::DeviceFailure(detail)) => {
                self.failed += 1;
                self.lost = true;
                self.losses += 1;
                self.slot_acquired = false;
                feedback.push(Feedback::DeviceLost);
                self.last_error = Some(detail.clone());
                self.publish(side, None, Some(detail));
            }
            Err(other) => {
                self.failed += 1;
                self.slot_acquired = false;
                let detail = other.to_string();
                self.last_error = Some(detail.clone());
                self.publish(side, None, Some(detail));
            }
        }
        feedback
    }
}
