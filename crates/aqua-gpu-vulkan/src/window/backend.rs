use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aqua_core::geometry::Rect;
use aqua_graph::{Extent, FramePlan};
use aqua_protocol::{
    DeviceStatus, FrameInputs, FrameReport, GpuBackend, GpuCapabilities, GpuError, GpuReadback,
    PresentHint, PresentTimes, RecoveryReport, SurfaceId, SurfacePixels, SurfaceTarget,
    UploadBatch, WaylandSurface,
};
use ash::vk;

use super::damage::{Damage, DamageTracker, PixelRect};
use super::pacing::{FrameSlot, PaceReport, Pacer, Slot, SurfacePacer, Wait};
use super::swapchain::{Acquire, Alpha, PresentOutcome, Swapchain, SwapchainError};
use crate::hal::budget::{pressure, trim_for, MemoryBudget, Trim, Visibility};
use crate::hal::{Access, Device, DeviceOptions};
use crate::half::bgra8_rows_to_rgba8;
use crate::scene::{
    capabilities, staging_bytes, Error as SceneError, Frames, PassTime, PersistentStats,
    PersistentTextures, PipelineStats, Presentable, RuntimeCompiler, SceneOptions, SceneRenderer,
    ShaderCompiler,
};

const ACQUIRE_LIMIT: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowOptions {
    pub device: DeviceOptions,
    pub scene: SceneOptions,
    pub compiler: ShaderCompiler,
}

impl WindowOptions {
    pub fn from_env() -> Self {
        Self {
            device: DeviceOptions::from_env(),
            scene: SceneOptions::default(),
            compiler: ShaderCompiler::from_env(),
        }
    }
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
    pub incremental_presents: u64,
    pub unavailable: u64,
    pub recoveries: u32,
    pub last_repaint_area: i64,
    pub scissored_frames: u64,
    pub skipped_pixels: u64,
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
    pub swapchain_bytes: u64,
    pub transient_heap_bytes: u64,
    pub frame_upload_bytes: u64,
    pub atlas_resident_bytes: u64,
    pub local_usage: u64,
    pub local_budget: u64,
}

impl MemoryBreakdown {
    pub fn attributed(&self) -> u64 {
        self.swapchain_bytes
            + self.transient_heap_bytes
            + self.frame_upload_bytes
            + self.atlas_resident_bytes
    }

    pub fn unattributed(&self) -> u64 {
        self.local_usage.saturating_sub(self.attributed())
    }
}

#[allow(clippy::large_enum_variant)]
enum FrameState {
    Idle,
    Acquired {
        image: u32,
        acquired: vk::Semaphore,
    },
    Rendered {
        image: u32,
        rendered: vk::Semaphore,
        damage: Option<Damage>,
    },
    Unavailable,
}

struct WindowSurface {
    swapchain: Swapchain,
    target: WaylandSurface,
    damage: DamageTracker,
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
    compiler: RuntimeCompiler,
    capabilities: GpuCapabilities,
    pending: Vec<UploadBatch>,
    next_surface: u32,
    generation: u32,
    visibility: Visibility,
    stats: WindowStats,
    phases: PhaseTimes,
    listener: Option<Box<dyn FnMut(RecoveryNotice)>>,
}

impl From<SwapchainError> for GpuError {
    fn from(error: SwapchainError) -> Self {
        match error {
            SwapchainError::Device(error) => SceneError::from(error).into(),
            SwapchainError::NotPresenting => {
                GpuError::Unsupported("window surfaces on a device made for offscreen work")
            }
            SwapchainError::NoPresentQueue { .. } => {
                GpuError::Unsupported("presenting to Wayland from this device's queue")
            }
            SwapchainError::NoFormat => {
                GpuError::Unsupported("a Wayland surface without an 8-bit UNORM format")
            }
            SwapchainError::NoAlpha {
                alpha: Alpha::Premultiplied,
            } => GpuError::Unsupported("premultiplied alpha on this Wayland surface"),
            SwapchainError::NoAlpha {
                alpha: Alpha::Opaque,
            } => GpuError::Unsupported("opaque composition on this Wayland surface"),
            SwapchainError::NoUsage { .. } => {
                GpuError::Unsupported("rendering into this Wayland surface's images")
            }
        }
    }
}

fn device_error(error: crate::hal::Error) -> GpuError {
    SceneError::from(error).into()
}

fn describe(device: &Device, custom_passes: bool) -> GpuCapabilities {
    let mut described = capabilities(device.info(), custom_passes);
    described.features.window_surfaces = true;
    described
}

fn bounding(damage: &Damage) -> Option<PixelRect> {
    damage
        .present_rects()
        .iter()
        .copied()
        .reduce(PixelRect::union)
}

fn scissor(rect: PixelRect) -> vk::Rect2D {
    vk::Rect2D {
        offset: vk::Offset2D {
            x: rect.left,
            y: rect.top,
        },
        extent: vk::Extent2D {
            width: (rect.right - rect.left).max(0) as u32,
            height: (rect.bottom - rect.top).max(0) as u32,
        },
    }
}

fn layer(rect: &PixelRect) -> vk::RectLayerKHR {
    vk::RectLayerKHR {
        offset: vk::Offset2D {
            x: rect.left,
            y: rect.top,
        },
        extent: vk::Extent2D {
            width: (rect.right - rect.left).max(0) as u32,
            height: (rect.bottom - rect.top).max(0) as u32,
        },
        layer: 0,
    }
}

impl Gpu {
    fn new(
        options: &WindowOptions,
        cache: Option<&[u8]>,
        compiler: &RuntimeCompiler,
    ) -> Result<Self, GpuError> {
        let device = Device::create_presenting(&options.device).map_err(device_error)?;
        let mut renderer = SceneRenderer::new(&device, cache, options.scene, compiler.clone())?;
        renderer.warm(vk::Format::B8G8R8A8_UNORM)?;
        Ok(Self {
            textures: PersistentTextures::new(&device),
            frames: Frames::new(&device, options.scene.timestamps).map_err(device_error)?,
            renderer,
            device,
        })
    }
}

impl WindowSurface {
    fn new(
        device: &Arc<Device>,
        target: WaylandSurface,
        alpha: Alpha,
        pacer: Box<dyn SurfacePacer>,
    ) -> Result<Self, GpuError> {
        let swapchain = Swapchain::new(device, &target, alpha)?;
        Ok(Self {
            damage: DamageTracker::new(swapchain.image_count()),
            swapchain,
            target,
            frame: FrameState::Idle,
            slot: FrameSlot::default(),
            pacing: Pacer::default(),
            pacer,
        })
    }

    fn rebuilt(self, device: &Arc<Device>) -> Result<Self, GpuError> {
        let WindowSurface {
            mut swapchain,
            target,
            frame,
            slot,
            mut pacing,
            pacer,
            ..
        } = self;
        let alpha = swapchain.alpha();
        let extent = swapchain.extent();
        Self::release(&mut swapchain, frame);
        drop(swapchain);
        let swapchain = Swapchain::new(device, &WaylandSurface { extent, ..target }, alpha)?;
        pacing.restart();
        Ok(Self {
            damage: DamageTracker::new(swapchain.image_count()),
            swapchain,
            target,
            frame: FrameState::Idle,
            slot,
            pacing,
            pacer,
        })
    }

    fn release(swapchain: &mut Swapchain, frame: FrameState) {
        let held = match frame {
            FrameState::Acquired { acquired, .. } => Some(acquired),
            FrameState::Rendered { rendered, .. } => Some(rendered),
            FrameState::Idle | FrameState::Unavailable => None,
        };
        if let Some(semaphore) = held {
            let _ = swapchain.release(semaphore);
        }
    }

    fn settle(&mut self) {
        let frame = std::mem::replace(&mut self.frame, FrameState::Idle);
        Self::release(&mut self.swapchain, frame);
    }

    fn observe_presentations(&mut self) {
        self.pacer
            .take_presented()
            .into_iter()
            .for_each(|presentation| self.pacing.observe(presentation));
    }

    fn poll(&mut self) {
        let interval = self.interval();
        match self.pacer.wait_ready(Duration::ZERO) {
            Ok(outcome) => self.slot.resolve(Instant::now(), outcome, interval),
            Err(error) => self.pacer_failed(&error),
        }
        self.observe_presentations();
    }

    fn pacer_failed(&mut self, error: &super::pacing::PacerError) {
        eprintln!("aqua: {error}; the window is treated as covered");
        self.slot.starve();
    }

    fn interval(&self) -> Duration {
        self.pacing.interval(self.pacer.refresh())
    }

    fn acquire(&mut self) -> Result<(), GpuError> {
        if !matches!(self.frame, FrameState::Idle) {
            return Ok(());
        }
        let limit = match self.slot.slot() {
            Slot::Starved => Duration::ZERO,
            Slot::Free | Slot::Pending { .. } => ACQUIRE_LIMIT,
        };
        let acquired = match self.swapchain.acquire(limit).map_err(device_error)? {
            Acquire::Stale => {
                let extent = self.swapchain.extent();
                self.swapchain.resize(extent)?;
                self.damage = DamageTracker::new(self.swapchain.image_count());
                self.swapchain.acquire(limit).map_err(device_error)?
            }
            acquired => acquired,
        };
        self.frame = match acquired {
            Acquire::Image { index, semaphore } => FrameState::Acquired {
                image: index,
                acquired: semaphore,
            },
            Acquire::Unavailable | Acquire::Stale => FrameState::Unavailable,
        };
        Ok(())
    }

    fn current_image(&self) -> Option<u32> {
        match self.frame {
            FrameState::Acquired { image, .. } | FrameState::Rendered { image, .. } => Some(image),
            FrameState::Idle | FrameState::Unavailable => None,
        }
    }

    fn repaint_region(&self, current: &Damage) -> Damage {
        match self.current_image() {
            Some(image) => self.damage.repaint(image as usize, current),
            None => (0..self.swapchain.image_count())
                .map(|image| self.damage.repaint(image, current))
                .fold(*current, |merged, region| merged.merge(&region)),
        }
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
        let compiler = RuntimeCompiler::load(&options.compiler);
        let gpu = Gpu::new(&options, cache, &compiler)?;
        Ok(Self {
            capabilities: describe(&gpu.device, gpu.renderer.custom_passes()),
            surfaces: HashMap::new(),
            gpu,
            options,
            compiler,
            pending: Vec::new(),
            next_surface: 0,
            generation: 0,
            visibility: Visibility::Visible,
            stats: WindowStats::default(),
            phases: PhaseTimes::default(),
            listener: None,
        })
    }

    pub fn create_wayland_surface(
        &mut self,
        target: WaylandSurface,
        alpha: Alpha,
        pacer: Box<dyn SurfacePacer>,
    ) -> Result<SurfaceId, GpuError> {
        let surface = WindowSurface::new(&self.gpu.device, target, alpha, pacer)?;
        let id = SurfaceId(self.next_surface);
        self.next_surface += 1;
        self.surfaces.insert(id, surface);
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

    pub fn validation_errors(&self) -> Vec<String> {
        self.gpu.device.validation_errors()
    }

    pub fn incremental_present(&self) -> bool {
        self.gpu.device.incremental_present()
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

    pub fn gpu_pass_times(&self) -> &[PassTime] {
        self.gpu.frames.pass_times()
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
        self.gpu.renderer.pipeline_cache().ok()
    }

    pub fn persistent_stats(&self) -> PersistentStats {
        self.gpu.textures.stats()
    }

    pub fn memory_budget(&self) -> Option<MemoryBudget> {
        self.gpu.device.memory_budget()
    }

    pub fn memory_breakdown(&self) -> MemoryBreakdown {
        let textures = self.gpu.textures.stats();
        let local = self.memory_budget();
        MemoryBreakdown {
            swapchain_bytes: self
                .surfaces
                .values()
                .map(|entry| entry.swapchain.bytes())
                .sum(),
            transient_heap_bytes: self.gpu.renderer.transient_bytes(),
            frame_upload_bytes: self.gpu.frames.ring_bytes(),
            atlas_resident_bytes: textures.resident_bytes + textures.pooled_bytes,
            local_usage: local.map_or(0, |local| local.usage),
            local_budget: local.map_or(0, |local| local.budget),
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

    fn pressure(&self) -> crate::hal::budget::Pressure {
        pressure(self.memory_budget().unwrap_or(MemoryBudget {
            budget: 0,
            usage: 0,
        }))
    }

    pub fn surface_extent(&self, surface: SurfaceId) -> Option<Extent> {
        self.surfaces
            .get(&surface)
            .map(|entry| entry.swapchain.extent())
    }

    pub fn present_mode(&self, surface: SurfaceId) -> Option<vk::PresentModeKHR> {
        self.surfaces
            .get(&surface)
            .map(|entry| entry.swapchain.present_mode())
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

    pub fn buffer_age(&self, surface: SurfaceId) -> Option<u32> {
        let entry = self.surfaces.get(&surface)?;
        Some(entry.damage.age(entry.current_image()? as usize))
    }

    pub fn repaint_bounds(&self, surface: SurfaceId, damage: &[Rect]) -> Option<Rect> {
        let entry = self.surfaces.get(&surface)?;
        let current = Damage::from_rects(damage, entry.swapchain.extent());
        bounding(&entry.repaint_region(&current)).map(|rect| Rect {
            x: rect.left as f32,
            y: rect.top as f32,
            w: (rect.right - rect.left) as f32,
            h: (rect.bottom - rect.top) as f32,
        })
    }

    pub fn execute_damaged(
        &mut self,
        plan: &FramePlan,
        inputs: &FrameInputs,
        damage: &[Rect],
    ) -> Result<FrameReport, GpuError> {
        self.execute_region(plan, inputs.surface, Some(damage))
    }

    fn execute_region(
        &mut self,
        plan: &FramePlan,
        surface: SurfaceId,
        damage: Option<&[Rect]>,
    ) -> Result<FrameReport, GpuError> {
        let started = Instant::now();
        aqua_graph::validate(plan)?;
        let validated = started.elapsed();
        let entry = self
            .surfaces
            .get_mut(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        let extent = entry.swapchain.extent();
        if plan.extent != extent {
            return Err(GpuError::ExtentMismatch {
                plan: plan.extent,
                surface: extent,
            });
        }
        let gpu = &mut self.gpu;
        gpu.device.check("frame execute").map_err(device_error)?;
        let acquire_started = Instant::now();
        entry.acquire()?;
        let acquired_in = acquire_started.elapsed();
        let (image, wait) = match entry.frame {
            FrameState::Acquired { image, acquired } => (image, acquired),
            FrameState::Rendered {
                image, rendered, ..
            } => (image, rendered),
            FrameState::Unavailable => {
                let ring = staging_bytes(&self.pending);
                let mut frame = gpu.frames.begin(ring).map_err(device_error)?;
                let pending = std::mem::take(&mut self.pending);
                let uploaded = gpu.textures.apply(&mut frame, &pending);
                frame.submit().map_err(device_error)?;
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
            FrameState::Idle => unreachable!("acquire leaves a surface with an image or none"),
        };
        let (repaint, current) = match damage {
            None => (None, None),
            Some(rects) => {
                let mut current = Damage::from_rects(rects, extent);
                let region = entry.damage.repaint(image as usize, &current);
                let repaint = bounding(&region).map(|bounds| {
                    let region = plan.repaint_bounds(Rect::from_edges(
                        bounds.left as f32,
                        bounds.top as f32,
                        bounds.right as f32,
                        bounds.bottom as f32,
                    ));
                    let expanded = PixelRect {
                        left: region.x.floor() as i32,
                        top: region.y.floor() as i32,
                        right: region.right().ceil() as i32,
                        bottom: region.bottom().ceil() as i32,
                    };
                    if expanded != bounds {
                        current = Damage::from_rects(&[region], extent);
                    }
                    expanded
                });
                (repaint, Some(current))
            }
        };
        let rendered = entry.swapchain.signal_semaphore().map_err(device_error)?;
        let record_started = Instant::now();
        let ring = gpu.renderer.prepare(plan) + staging_bytes(&self.pending);
        let mut frame = gpu.frames.begin(ring).map_err(device_error)?;
        let pending = std::mem::take(&mut self.pending);
        let target = entry.swapchain.image(image);
        let recorded = gpu.textures.apply(&mut frame, &pending).and_then(|_| {
            gpu.renderer.record(
                &mut frame,
                plan,
                target,
                &gpu.textures,
                repaint.map(scissor),
            )
        });
        let presentable: Vec<_> = target.transition(Access::Present).into_iter().collect();
        frame.barrier(&presentable);
        let barriers = frame.barriers();
        let serial = frame
            .submit_presentable(Presentable {
                acquired: wait,
                rendered,
            })
            .map_err(device_error)?;
        entry.swapchain.submitted(wait, serial);
        entry.frame = FrameState::Rendered {
            image,
            rendered,
            damage: current,
        };
        let stats = match recorded {
            Ok(stats) => stats,
            Err(error) => {
                entry.damage.reset();
                return Err(error.into());
            }
        };
        self.phases = PhaseTimes {
            validate: validated,
            acquire: acquired_in,
            record: record_started.elapsed(),
            ..self.phases
        };
        self.stats.frames += 1;
        let total = i64::from(extent.width) * i64::from(extent.height);
        match repaint {
            Some(rect) => {
                let area =
                    (rect.right - rect.left).max(0) as i64 * (rect.bottom - rect.top).max(0) as i64;
                self.stats.scissored_frames += 1;
                self.stats.last_repaint_area = area;
                self.stats.skipped_pixels += (total - area).max(0) as u64;
            }
            None => self.stats.last_repaint_area = total,
        }
        Ok(FrameReport {
            passes: stats.passes,
            instances: stats.instances,
            cpu_time: started.elapsed(),
            device: DeviceStatus::Healthy,
            barriers,
            transient_bytes: gpu.renderer.transient_bytes(),
            gpu_time: gpu.frames.gpu_time(),
        })
    }

    fn present_inner(
        &mut self,
        surface: SurfaceId,
        damage: Damage,
    ) -> Result<PresentTimes, GpuError> {
        let started = Instant::now();
        self.gpu.device.check("present").map_err(device_error)?;
        let incremental = self.gpu.device.incremental_present();
        let entry = self
            .surfaces
            .get_mut(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        match std::mem::replace(&mut entry.frame, FrameState::Idle) {
            FrameState::Rendered {
                image,
                rendered,
                damage: planned,
            } => {
                let damage = match entry.damage.has_presented() {
                    true => planned.unwrap_or(damage),
                    false => Damage::Full,
                };
                let rects: Vec<vk::RectLayerKHR> =
                    damage.present_rects().iter().map(layer).collect();
                entry.pacer.before_present(entry.swapchain.extent());
                let presented = entry.swapchain.present(image, rendered, &rects);
                self.phases.present = started.elapsed();
                match presented.map_err(device_error)? {
                    PresentOutcome::Presented => {
                        entry.damage.commit(image as usize, damage);
                        entry.slot.presented(Instant::now());
                        self.stats.presents += 1;
                        if incremental && !rects.is_empty() {
                            self.stats.incremental_presents += 1;
                        }
                    }
                    PresentOutcome::Stale => {
                        let extent = entry.swapchain.extent();
                        entry.swapchain.resize(extent)?;
                        entry.damage = DamageTracker::new(entry.swapchain.image_count());
                    }
                }
            }
            FrameState::Unavailable => entry.damage.reset(),
            unexecuted @ (FrameState::Idle | FrameState::Acquired { .. }) => {
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

    fn recover_inner(&mut self) -> Result<RecoveryReport, GpuError> {
        let cache = self.pipeline_cache();
        let gpu = Gpu::new(&self.options, cache.as_deref(), &self.compiler)?;
        let surfaces: Vec<(SurfaceId, WindowSurface)> = self.surfaces.drain().collect();
        self.pending.clear();
        let rebuilt = surfaces
            .into_iter()
            .map(|(id, surface)| surface.rebuilt(&gpu.device).map(|surface| (id, surface)))
            .collect::<Result<Vec<_>, GpuError>>()?;
        let count = rebuilt.len() as u32;
        self.surfaces.extend(rebuilt);
        self.capabilities = describe(&gpu.device, gpu.renderer.custom_passes());
        self.gpu = gpu;
        self.generation += 1;
        self.stats.recoveries += 1;
        let notice = RecoveryNotice {
            generation: self.generation,
            software: self.capabilities.features.software,
            textures: TextureRecovery::HostMustReupload,
        };
        if let Some(listener) = self.listener.as_mut() {
            listener(notice);
        }
        Ok(RecoveryReport {
            rebuilt_surfaces: count,
        })
    }

    fn read_inner(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        let entry = self
            .surfaces
            .get(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        let FrameState::Rendered { image, .. } = entry.frame else {
            return Err(GpuError::DeviceFailure(
                "surface has no unpresented frame".into(),
            ));
        };
        let extent = entry.swapchain.extent();
        let format = entry.swapchain.format();
        let bytes = self
            .gpu
            .frames
            .read_image(entry.swapchain.image(image), 4)
            .map_err(device_error)?;
        let (width, height) = (extent.width as usize, extent.height as usize);
        let rgba8 = match format {
            vk::Format::B8G8R8A8_UNORM => bgra8_rows_to_rgba8(&bytes, width * 4, width, height),
            _ => bytes,
        };
        Ok(SurfacePixels { extent, rgba8 })
    }
}

impl Drop for WindowBackend {
    fn drop(&mut self) {
        self.surfaces.values_mut().for_each(WindowSurface::settle);
        self.surfaces.clear();
    }
}

impl GpuBackend for WindowBackend {
    type Error = GpuError;

    fn capabilities(&self) -> &GpuCapabilities {
        &self.capabilities
    }

    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
        match target {
            SurfaceTarget::Wayland(_) => Err(GpuError::Unsupported(
                "a Wayland surface without its pacer: use WindowBackend::create_wayland_surface",
            )),
            SurfaceTarget::Metal(_) => {
                Err(GpuError::Unsupported("Metal layers need aqua-gpu-metal"))
            }
            SurfaceTarget::Native(_) | SurfaceTarget::Window(_) => Err(GpuError::Unsupported(
                "non-Wayland window surfaces on the Vulkan backend",
            )),
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
        self.execute_region(plan, inputs.surface, None)
    }

    fn present(
        &mut self,
        surface: SurfaceId,
        _hint: PresentHint,
    ) -> Result<PresentTimes, GpuError> {
        self.present_inner(surface, Damage::Full)
    }

    fn present_with_damage(
        &mut self,
        surface: SurfaceId,
        _hint: PresentHint,
        damage: &[Rect],
    ) -> Result<PresentTimes, GpuError> {
        let extent = self
            .surface_extent(surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        self.present_inner(surface, Damage::from_rects(damage, extent))
    }

    fn recover(&mut self) -> Result<RecoveryReport, GpuError> {
        self.recover_inner()
    }

    fn resize_surface(&mut self, surface: SurfaceId, extent: Extent) -> Result<(), GpuError> {
        let entry = self
            .surfaces
            .get_mut(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        if entry.swapchain.extent() == extent {
            return Ok(());
        }
        let started = Instant::now();
        entry.settle();
        entry.swapchain.resize(extent)?;
        entry.damage = DamageTracker::new(entry.swapchain.image_count());
        self.phases.resize = started.elapsed();
        Ok(())
    }

    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<(), GpuError> {
        let mut entry = self
            .surfaces
            .remove(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        entry.settle();
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
            Wait::Poll => Some(entry.pacer.wait_ready(Duration::ZERO)),
            Wait::For(timeout) => Some(entry.pacer.wait_ready(timeout)),
        };
        match waited {
            None => {}
            Some(Ok(outcome)) => entry.slot.resolve(Instant::now(), outcome, interval),
            Some(Err(error)) => entry.pacer_failed(&error),
        }
        entry.observe_presentations();
        let acquired = entry.acquire();
        self.phases.slot_wait = started.elapsed();
        acquired
    }
}

impl GpuReadback for WindowBackend {
    fn read_surface(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        self.read_inner(surface)
    }
}
