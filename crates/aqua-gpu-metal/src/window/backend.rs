use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aqua_core::geometry::Rect;
use aqua_graph::{Extent, FramePlan};
use aqua_protocol::{
    DeviceStatus, FrameInputs, FrameReport, GpuBackend, GpuCapabilities, GpuError, GpuReadback,
    PresentHint, PresentTimes, RecoveryReport, SurfaceId, SurfacePixels, SurfaceTarget,
    UploadBatch, WindowId,
};
use objc2::rc::Retained;
use objc2_metal::MTLCommandBuffer;
use objc2_quartz_core::CAMetalLayer;

use super::layer::{Alpha, Drawable, Layer};
use super::pacing::{presentation, FrameSlot, PaceReport, Pacer, Slot, SurfacePacer, Wait};
use crate::hal::budget::{pressure, trim_for, MemoryBudget, Pressure, Trim, Visibility};
use crate::hal::{Device, DeviceOptions};
use crate::scene::{
    capabilities, staging_bytes, Error as SceneError, Frames, PersistentStats, PersistentTextures,
    PipelineStats, SceneOptions, SceneRenderer, SurfaceFormat,
};

const PRESENT_LABEL: &str = "aqua present";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowOptions {
    pub device: DeviceOptions,
    pub scene: SceneOptions,
}

impl WindowOptions {
    pub fn from_env() -> Self {
        Self {
            device: DeviceOptions::from_env(),
            scene: SceneOptions::default(),
        }
    }
}

pub struct LayerTarget {
    pub window: WindowId,
    pub layer: Retained<CAMetalLayer>,
    pub extent: Extent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureRecovery {
    HostMustReupload,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryNotice {
    pub generation: u32,
    pub software: bool,
    pub textures: TextureRecovery,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowStats {
    pub frames: u64,
    pub presents: u64,
    pub unavailable: u64,
    pub recoveries: u32,
    pub last_repaint_area: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhaseTimes {
    pub validate: Duration,
    pub slot_wait: Duration,
    pub acquire: Duration,
    pub record: Duration,
    pub present: Duration,
    pub resize: Duration,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryBreakdown {
    pub drawable_bytes: u64,
    pub transient_heap_bytes: u64,
    pub frame_upload_bytes: u64,
    pub atlas_resident_bytes: u64,
    pub local_usage: u64,
    pub local_budget: u64,
}

impl MemoryBreakdown {
    pub fn attributed(&self) -> u64 {
        self.drawable_bytes
            + self.transient_heap_bytes
            + self.frame_upload_bytes
            + self.atlas_resident_bytes
    }

    pub fn unattributed(&self) -> u64 {
        self.local_usage.saturating_sub(self.attributed())
    }
}

enum FrameState {
    Idle,
    Acquired(Drawable),
    Rendered(Drawable),
    Unavailable,
}

struct WindowSurface {
    window: WindowId,
    layer: Layer,
    frame: FrameState,
    slot: FrameSlot,
    pacing: Pacer,
    pacer: Box<dyn SurfacePacer>,
}

struct Gpu {
    device: Arc<Device>,
    renderer: SceneRenderer,
    textures: PersistentTextures,
    frames: Frames,
}

pub struct WindowBackend {
    surfaces: HashMap<SurfaceId, WindowSurface>,
    gpu: Gpu,
    options: WindowOptions,
    capabilities: GpuCapabilities,
    pending: Vec<UploadBatch>,
    next_surface: u32,
    generation: u32,
    visibility: Visibility,
    stats: WindowStats,
    phases: PhaseTimes,
    listener: Option<Box<dyn FnMut(RecoveryNotice)>>,
}

fn device_error(error: crate::hal::Error) -> GpuError {
    SceneError::from(error).into()
}

fn describe(device: &Device) -> GpuCapabilities {
    let mut described = capabilities(device.info());
    described.features.window_surfaces = true;
    described
}

fn whole(extent: Extent) -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        w: extent.width as f32,
        h: extent.height as f32,
    }
}

impl Gpu {
    fn new(options: &WindowOptions, cache: Option<&[u8]>) -> Result<Self, GpuError> {
        let device = Device::create(&options.device).map_err(device_error)?;
        let mut renderer = SceneRenderer::new(&device, cache, options.scene)?;
        renderer.warm(super::layer::FORMAT)?;
        Ok(Self {
            textures: PersistentTextures::new(&device),
            frames: Frames::new(&device),
            renderer,
            device,
        })
    }
}

impl WindowSurface {
    fn interval(&self) -> Duration {
        self.pacing.interval(self.pacer.refresh())
    }

    fn observe_presentations(&mut self) {
        let pacer = &*self.pacer;
        self.layer
            .feedback()
            .take_shown()
            .into_iter()
            .for_each(|shown| self.pacing.observe(presentation(shown, pacer)));
    }

    fn poll(&mut self) {
        let interval = self.interval();
        let outcome = self.layer.feedback().wait(Duration::ZERO);
        self.slot.resolve(Instant::now(), outcome, interval);
        self.observe_presentations();
    }

    fn drawable_free(&self) -> bool {
        self.layer.feedback().outstanding() == 0
            || (self.slot.slot() == Slot::Starved && !self.pacer.covered())
    }

    fn acquire(&mut self) {
        if !matches!(self.frame, FrameState::Idle) {
            return;
        }
        self.frame = match self.drawable_free() {
            true => self
                .layer
                .next_drawable()
                .map_or(FrameState::Unavailable, FrameState::Acquired),
            false => FrameState::Unavailable,
        };
    }

    fn settle(&mut self) {
        self.frame = FrameState::Idle;
    }
}

impl WindowBackend {
    pub fn new(options: WindowOptions) -> Result<Self, GpuError> {
        Self::with_pipeline_cache(options, None)
    }

    pub fn with_pipeline_cache(
        options: WindowOptions,
        cache: Option<&[u8]>,
    ) -> Result<Self, GpuError> {
        let gpu = Gpu::new(&options, cache)?;
        Ok(Self {
            capabilities: describe(&gpu.device),
            surfaces: HashMap::new(),
            gpu,
            options,
            pending: Vec::new(),
            next_surface: 0,
            generation: 0,
            visibility: Visibility::Visible,
            stats: WindowStats::default(),
            phases: PhaseTimes::default(),
            listener: None,
        })
    }

    pub fn create_layer_surface(
        &mut self,
        target: LayerTarget,
        alpha: Alpha,
        pacer: Box<dyn SurfacePacer>,
    ) -> Result<SurfaceId, GpuError> {
        let LayerTarget {
            window,
            layer,
            extent,
        } = target;
        if extent.width == 0 || extent.height == 0 {
            return Err(GpuError::DeviceFailure(format!(
                "window {} has an empty {}x{} layer",
                window.raw(),
                extent.width,
                extent.height
            )));
        }
        let id = SurfaceId(self.next_surface);
        self.next_surface += 1;
        self.surfaces.insert(
            id,
            WindowSurface {
                window,
                layer: Layer::new(layer, &self.gpu.device.raw, extent, alpha),
                frame: FrameState::Idle,
                slot: FrameSlot::default(),
                pacing: Pacer::default(),
                pacer,
            },
        );
        Ok(id)
    }

    pub fn set_recovery_listener(&mut self, listener: Box<dyn FnMut(RecoveryNotice)>) {
        self.listener = Some(listener);
    }

    pub fn finish_startup(&mut self) {
        self.gpu.renderer.close_startup();
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn device_status(&self) -> DeviceStatus {
        match self.gpu.device.is_lost() {
            true => DeviceStatus::Lost,
            false => DeviceStatus::Healthy,
        }
    }

    pub fn inject_device_removal(&self) {
        self.gpu.device.inject_loss();
    }

    pub fn validating(&self) -> bool {
        self.gpu.device.validating()
    }

    pub fn metal_object_classes(&self) -> String {
        self.gpu.device.object_classes()
    }

    pub fn validation_errors(&self) -> Vec<String> {
        self.gpu.device.validation_errors()
    }

    pub fn incremental_present(&self) -> bool {
        false
    }

    pub fn stats(&self) -> WindowStats {
        self.stats
    }

    pub fn phases(&self) -> PhaseTimes {
        self.phases
    }

    pub fn ledger(&self) -> crate::ledger::Snapshot {
        crate::ledger::snapshot()
    }

    pub fn gpu_frame_time(&self) -> Option<Duration> {
        self.gpu.frames.gpu_time()
    }

    pub fn pipeline_stats(&self) -> PipelineStats {
        self.gpu.renderer.pipeline_stats()
    }

    pub fn late_pipelines(&self) -> Vec<String> {
        self.gpu
            .renderer
            .late_pipelines()
            .iter()
            .map(|kind| format!("{kind:?}"))
            .collect()
    }

    pub fn pipeline_cache(&self) -> Option<Vec<u8>> {
        Some(self.gpu.renderer.pipeline_cache())
    }

    pub fn persistent_stats(&self) -> PersistentStats {
        self.gpu.textures.stats()
    }

    pub fn memory_budget(&self) -> Option<MemoryBudget> {
        Some(self.gpu.device.memory_budget())
    }

    pub fn memory_breakdown(&self) -> MemoryBreakdown {
        let textures = self.gpu.textures.stats();
        let local = self.gpu.device.memory_budget();
        MemoryBreakdown {
            drawable_bytes: self
                .surfaces
                .values()
                .map(|entry| entry.layer.bytes())
                .sum(),
            transient_heap_bytes: self.gpu.renderer.transient_bytes(),
            frame_upload_bytes: self.gpu.frames.ring_bytes(),
            atlas_resident_bytes: textures.resident_bytes + textures.pooled_bytes,
            local_usage: local.usage,
            local_budget: local.budget,
        }
    }

    pub fn trim(&mut self, trim: Trim) -> u64 {
        let gpu = &mut self.gpu;
        let mut freed = gpu.renderer.trim(&trim);
        if trim.shrink_upload_ring {
            freed += gpu.frames.ring_bytes();
            gpu.frames.shrink();
        }
        if trim.evict_idle_resources {
            freed += gpu.textures.drain_pool();
        }
        freed
    }

    pub fn set_visibility(&mut self, visibility: Visibility) -> Trim {
        self.visibility = visibility;
        let trim = trim_for(self.pressure(), visibility);
        self.trim(trim);
        trim
    }

    pub fn service_budget(&mut self) -> u64 {
        let trim = trim_for(self.pressure(), self.visibility);
        self.trim(trim)
    }

    fn pressure(&self) -> Pressure {
        pressure(self.gpu.device.memory_budget())
    }

    pub fn surface_extent(&self, surface: SurfaceId) -> Option<Extent> {
        self.surfaces
            .get(&surface)
            .map(|entry| entry.layer.extent())
    }

    pub fn surface_window(&self, surface: SurfaceId) -> Option<WindowId> {
        self.surfaces.get(&surface).map(|entry| entry.window)
    }

    pub fn surface_alpha(&self, surface: SurfaceId) -> Option<Alpha> {
        self.surfaces.get(&surface).map(|entry| entry.layer.alpha())
    }

    pub fn frame_slot(&self, surface: SurfaceId) -> Option<Slot> {
        self.surfaces.get(&surface).map(|entry| entry.slot.slot())
    }

    pub fn pacing(&mut self, surface: SurfaceId) -> Option<PaceReport> {
        let entry = self.surfaces.get_mut(&surface)?;
        entry.poll();
        Some(entry.pacing.report())
    }

    pub fn refresh_interval(&self, surface: SurfaceId) -> Option<Duration> {
        self.surfaces.get(&surface).map(WindowSurface::interval)
    }

    pub fn repaint_bounds(&self, surface: SurfaceId, _damage: &[Rect]) -> Option<Rect> {
        self.surface_extent(surface).map(whole)
    }

    pub fn execute_damaged(
        &mut self,
        plan: &FramePlan,
        inputs: &FrameInputs,
        _damage: &[Rect],
    ) -> Result<FrameReport, GpuError> {
        self.execute_frame(plan, inputs.surface)
    }

    fn execute_frame(
        &mut self,
        plan: &FramePlan,
        surface: SurfaceId,
    ) -> Result<FrameReport, GpuError> {
        let started = Instant::now();
        aqua_graph::validate(plan)?;
        let validated = started.elapsed();
        let entry = self
            .surfaces
            .get_mut(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        let extent = entry.layer.extent();
        if plan.extent != extent {
            return Err(GpuError::ExtentMismatch {
                plan: plan.extent,
                surface: extent,
            });
        }
        let gpu = &mut self.gpu;
        gpu.device.check("frame execute").map_err(device_error)?;
        let acquire_started = Instant::now();
        entry.acquire();
        let acquired_in = acquire_started.elapsed();
        let drawable = match std::mem::replace(&mut entry.frame, FrameState::Idle) {
            FrameState::Acquired(drawable) | FrameState::Rendered(drawable) => drawable,
            FrameState::Unavailable => {
                entry.frame = FrameState::Unavailable;
                let ring = staging_bytes(&self.pending);
                let mut frame = gpu.frames.begin(ring).map_err(device_error)?;
                let pending = std::mem::take(&mut self.pending);
                let uploaded = gpu.textures.apply(&mut frame, &pending);
                frame.submit();
                uploaded?;
                self.stats.unavailable += 1;
                return Ok(FrameReport {
                    passes: 0,
                    instances: 0,
                    cpu_time: started.elapsed(),
                    device: DeviceStatus::Healthy,
                    barriers: 0,
                    transient_bytes: gpu.renderer.transient_bytes(),
                    gpu_time: gpu.frames.gpu_time(),
                });
            }
            FrameState::Idle => unreachable!("acquire leaves a surface with a drawable or none"),
        };
        let record_started = Instant::now();
        let ring = gpu.renderer.prepare(plan) + staging_bytes(&self.pending);
        let mut frame = gpu.frames.begin(ring).map_err(device_error)?;
        let pending = std::mem::take(&mut self.pending);
        let recorded = gpu.textures.apply(&mut frame, &pending).and_then(|_| {
            gpu.renderer
                .record(&mut frame, plan, drawable.view(), &gpu.textures)
        });
        frame.submit();
        entry.frame = FrameState::Rendered(drawable);
        let stats = recorded?;
        self.phases = PhaseTimes {
            validate: validated,
            acquire: acquired_in,
            record: record_started.elapsed(),
            ..self.phases
        };
        self.stats.frames += 1;
        self.stats.last_repaint_area = i64::from(extent.width) * i64::from(extent.height);
        Ok(FrameReport {
            passes: stats.passes,
            instances: stats.instances,
            cpu_time: started.elapsed(),
            device: DeviceStatus::Healthy,
            barriers: 0,
            transient_bytes: gpu.renderer.transient_bytes(),
            gpu_time: gpu.frames.gpu_time(),
        })
    }

    fn present_frame(&mut self, surface: SurfaceId) -> Result<PresentTimes, GpuError> {
        let started = Instant::now();
        self.gpu.device.check("present").map_err(device_error)?;
        let entry = self
            .surfaces
            .get_mut(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        match std::mem::replace(&mut entry.frame, FrameState::Idle) {
            FrameState::Rendered(drawable) => {
                let command = self
                    .gpu
                    .device
                    .command_buffer(PRESENT_LABEL)
                    .map_err(device_error)?;
                entry.layer.watch(&drawable);
                command.presentDrawable(drawable.as_drawable());
                command.commit();
                entry.slot.presented(Instant::now());
                self.stats.presents += 1;
                self.phases.present = started.elapsed();
            }
            FrameState::Unavailable => {}
            unexecuted @ (FrameState::Idle | FrameState::Acquired(_)) => {
                entry.frame = unexecuted;
                return Err(GpuError::DeviceFailure(
                    "surface has no executed frame".into(),
                ));
            }
        }
        Ok(PresentTimes {
            queued: started.elapsed(),
        })
    }

    fn recover_device(&mut self) -> Result<RecoveryReport, GpuError> {
        let cache = self.pipeline_cache();
        let gpu = Gpu::new(&self.options, cache.as_deref())?;
        self.pending.clear();
        self.surfaces.values_mut().for_each(|entry| {
            entry.settle();
            entry.layer.attach(&gpu.device.raw);
            entry.pacing.restart();
        });
        let count = self.surfaces.len() as u32;
        self.capabilities = describe(&gpu.device);
        self.gpu = gpu;
        self.generation += 1;
        self.stats.recoveries += 1;
        let notice = RecoveryNotice {
            generation: self.generation,
            software: false,
            textures: TextureRecovery::HostMustReupload,
        };
        if let Some(listener) = self.listener.as_mut() {
            listener(notice);
        }
        Ok(RecoveryReport {
            rebuilt_surfaces: count,
        })
    }

    fn read_drawable(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        let entry = self
            .surfaces
            .get(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        let FrameState::Rendered(drawable) = &entry.frame else {
            return Err(GpuError::DeviceFailure(
                "surface has no unpresented frame".into(),
            ));
        };
        let extent = drawable.extent();
        let format = SurfaceFormat::Bgra8Unorm;
        let bytes = self
            .gpu
            .frames
            .read_texture(drawable.view(), format.texel_bytes())
            .map_err(device_error)?;
        Ok(SurfacePixels {
            extent,
            rgba8: format.to_rgba8(&bytes, extent.width as usize, extent.height as usize),
        })
    }
}

impl Drop for WindowBackend {
    fn drop(&mut self) {
        self.surfaces.values_mut().for_each(WindowSurface::settle);
        self.gpu.frames.wait_idle();
    }
}

impl GpuBackend for WindowBackend {
    type Error = GpuError;

    fn capabilities(&self) -> &GpuCapabilities {
        &self.capabilities
    }

    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
        match target {
            SurfaceTarget::Metal(_) => Err(GpuError::Unsupported(
                "a Metal layer without its pacer: use WindowBackend::create_layer_surface",
            )),
            SurfaceTarget::Native(_) | SurfaceTarget::Window(_) | SurfaceTarget::Wayland(_) => Err(
                GpuError::Unsupported("non-Metal window surfaces on the Metal backend"),
            ),
            SurfaceTarget::Offscreen(_) => Err(GpuError::Unsupported(
                "offscreen surfaces on the window backend",
            )),
        }
    }

    fn upload(&mut self, batch: UploadBatch) -> Result<(), GpuError> {
        if !batch.textures.is_empty() {
            self.pending.push(batch);
        }
        Ok(())
    }

    fn execute(&mut self, plan: &FramePlan, inputs: &FrameInputs) -> Result<FrameReport, GpuError> {
        self.execute_frame(plan, inputs.surface)
    }

    fn present(
        &mut self,
        surface: SurfaceId,
        _hint: PresentHint,
    ) -> Result<PresentTimes, GpuError> {
        self.present_frame(surface)
    }

    fn present_with_damage(
        &mut self,
        surface: SurfaceId,
        _hint: PresentHint,
        _damage: &[Rect],
    ) -> Result<PresentTimes, GpuError> {
        self.present_frame(surface)
    }

    fn recover(&mut self) -> Result<RecoveryReport, GpuError> {
        self.recover_device()
    }

    fn resize_surface(&mut self, surface: SurfaceId, extent: Extent) -> Result<(), GpuError> {
        let entry = self
            .surfaces
            .get_mut(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        if entry.layer.extent() == extent {
            return Ok(());
        }
        let started = Instant::now();
        entry.settle();
        entry.layer.resize(extent);
        self.phases.resize = started.elapsed();
        Ok(())
    }

    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<(), GpuError> {
        let mut entry = self
            .surfaces
            .remove(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        entry.settle();
        self.gpu.frames.wait_idle();
        Ok(())
    }

    fn wait_for_frame_slot(&mut self, surface: SurfaceId) -> Result<(), GpuError> {
        self.wait_for_frame_slot_with_hint(surface, PresentHint::Vsync)
    }

    fn wait_for_frame_slot_with_hint(
        &mut self,
        surface: SurfaceId,
        hint: PresentHint,
    ) -> Result<(), GpuError> {
        let started = Instant::now();
        let entry = self
            .surfaces
            .get_mut(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        entry.observe_presentations();
        let interval = entry.interval();
        let waited = match entry.slot.plan(Instant::now(), hint, interval) {
            Wait::None => None,
            Wait::Poll => Some(entry.layer.feedback().wait(Duration::ZERO)),
            Wait::For(timeout) => Some(entry.layer.feedback().wait(timeout)),
        };
        if let Some(outcome) = waited {
            entry.slot.resolve(Instant::now(), outcome, interval);
        }
        entry.observe_presentations();
        entry.acquire();
        self.phases.slot_wait = started.elapsed();
        Ok(())
    }
}

impl GpuReadback for WindowBackend {
    fn read_surface(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        self.read_drawable(surface)
    }
}
