use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use aqua_core::geometry::Rect;
use aqua_graph::{validate, Extent, FramePlan};
use aqua_protocol::{
    DeviceStatus, FeatureTier, Features, FrameInputs, FrameReport, GpuBackend, GpuCapabilities,
    GpuError, GpuReadback, PresentHint, PresentTimes, RecoveryReport, SurfaceId, SurfacePixels,
    SurfaceTarget, UploadBatch,
};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION;

use super::damage::{Damage, DamageTracker, PixelRect};
use super::mirror::TextureMirror;
use super::pacing::{PaceReport, Pacer};
use super::timing;
use crate::hal::budget::{Trim, Visibility};
use crate::hal::{
    self, BudgetStatus, Core, DeviceOptions, PresentMode, PresentStatus, Swapchain, RENDER_FORMAT,
};
use crate::half::bgra8_rows_to_rgba8;
use crate::lifetimes::Precision;
use crate::scene::{
    PassTime, PersistentStats, PersistentTextures, PipelineKind as Kind, PipelineStats,
    SceneOptions, SceneRenderer, SceneTarget,
};

const FRAME_WAIT_MS: u32 = 1000;

#[derive(Clone, Copy, Debug, Default)]
pub struct WindowOptions {
    pub device: DeviceOptions,
    pub mirror_textures: bool,
}

impl WindowOptions {
    pub fn from_env() -> Self {
        Self {
            device: DeviceOptions::from_env(),
            mirror_textures: std::env::var("AQUA_MIRROR_TEXTURES").as_deref() == Ok("1"),
        }
    }

    pub fn with_mirror(mut self, mirror: bool) -> Self {
        self.mirror_textures = mirror;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    Hwnd(u64),
    Composition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureRecovery {
    Replayed { textures: u32 },
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
    pub occluded: u64,
    pub recoveries: u32,
    pub last_repaint_area: i64,
    pub scissored_frames: u64,
    pub skipped_pixels: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhaseTimes {
    pub validate: Duration,
    pub slot_wait: Duration,
    pub reserve: Duration,
    pub record: Duration,
    pub submit: Duration,
    pub present: Duration,
    pub resize: Duration,
    pub reserve_created: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryBreakdown {
    pub swapchain_bytes: u64,
    pub transient_heap_bytes: u64,
    pub retired_transient_bytes: u64,
    pub frame_upload_bytes: u64,
    pub scene_upload_bytes: u64,
    pub atlas_resident_bytes: u64,
    pub atlas_staging_bytes: u64,
    pub surface_mirror_bytes: u64,
    pub local_usage: u64,
    pub local_budget: u64,
}

impl MemoryBreakdown {
    pub fn attributed(&self) -> u64 {
        self.swapchain_bytes
            + self.transient_heap_bytes
            + self.retired_transient_bytes
            + self.frame_upload_bytes
            + self.scene_upload_bytes
            + self.atlas_resident_bytes
            + self.atlas_staging_bytes
            + self.surface_mirror_bytes
    }

    pub fn unattributed(&self) -> u64 {
        self.local_usage.saturating_sub(self.attributed())
    }
}

struct WindowSurface {
    swapchain: Swapchain,
    kind: SurfaceKind,
    window: aqua_protocol::WindowId,
    damage: DamageTracker,
    rendered: Option<usize>,
    pending: bool,
    frame_damage: Option<Damage>,
    pacer: Pacer,
    last_present: Option<Instant>,
    monitor: isize,
    half_rate: bool,
}

struct Handoff<T>(T);

unsafe impl<T> Send for Handoff<T> {}

type Early = std::thread::JoinHandle<Handoff<Result<WindowBackend, GpuError>>>;

static EARLY: std::sync::Mutex<Option<Early>> = std::sync::Mutex::new(None);

struct Gpu {
    renderer: SceneRenderer,
    textures: PersistentTextures,
}

pub struct WindowBackend {
    startup_deferred: bool,
    startup: VecDeque<Kind>,
    options: WindowOptions,
    capabilities: GpuCapabilities,
    surfaces: HashMap<SurfaceId, WindowSurface>,
    gpu: Option<Gpu>,
    core: Option<Core>,
    pending_uploads: Vec<UploadBatch>,
    mirror: TextureMirror,
    next_surface: u32,
    lost: bool,
    stats: WindowStats,
    phases: PhaseTimes,
    listener: Option<Box<dyn FnMut(RecoveryNotice)>>,
}

fn build_gpu(core: &Core, cache: Option<&[u8]>, deferred: bool) -> Result<Gpu, GpuError> {
    let stages = crate::stages::StageClock::from_env();
    let mut renderer = SceneRenderer::new(core.device(), cache, SceneOptions::default())?;
    stages.mark("  renderer_new");
    match deferred {
        true => renderer.warm_first_frame()?,
        false => renderer.warm()?,
    }
    stages.mark("  warm");
    Ok(Gpu {
        renderer,
        textures: PersistentTextures::new(core.device()),
    })
}

fn describe(core: &Core) -> GpuCapabilities {
    let info = core.info();
    GpuCapabilities {
        backend: "d3d11".into(),
        adapter: info.adapter.name.clone(),
        tier: FeatureTier::Baseline,
        max_texture_extent: D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION,
        heap_alignment: crate::scene::HEAP_ALIGNMENT,
        features: Features {
            compute: true,
            custom_passes: true,
            color_matrix: true,
            enhanced_barriers: false,
            tearing: info.tearing,
            software: info.adapter.kind == hal::selection::Adapter::Software,
            window_surfaces: true,
        },
    }
}

fn to_rect(rect: PixelRect) -> RECT {
    RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

fn bounding_rect(damage: &Damage) -> Option<RECT> {
    damage
        .present_rects()
        .iter()
        .copied()
        .reduce(PixelRect::union)
        .map(to_rect)
}

fn owner(window: aqua_protocol::WindowId, kind: SurfaceKind) -> isize {
    match kind {
        SurfaceKind::Hwnd(hwnd) => hwnd as isize,
        SurfaceKind::Composition => window.raw() as isize,
    }
}

fn new_surface(
    core: &Core,
    window: aqua_protocol::WindowId,
    kind: SurfaceKind,
    extent: Extent,
) -> Result<WindowSurface, GpuError> {
    let monitor = timing::monitor_of(owner(window, kind));
    let swapchain = match kind {
        SurfaceKind::Hwnd(hwnd) => {
            core.create_swapchain(hwnd as isize, extent.width, extent.height)?
        }
        SurfaceKind::Composition => {
            core.create_composition_swapchain(extent.width, extent.height)?
        }
    };
    Ok(WindowSurface {
        swapchain,
        kind,
        window,
        damage: DamageTracker::new(2),
        rendered: None,
        pending: false,
        frame_damage: None,
        pacer: Pacer::new(timing::monitor_interval(monitor)),
        last_present: None,
        monitor,
        half_rate: false,
    })
}

fn no_device() -> GpuError {
    GpuError::DeviceFailure("no device: recovery is required".into())
}

impl From<hal::Error> for GpuError {
    fn from(error: hal::Error) -> Self {
        GpuError::DeviceFailure(error.to_string())
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
        let core = Core::new(options.device)?;
        let gpu = build_gpu(&core, cache, false)?;
        Ok(Self::assemble(options, core, gpu))
    }

    pub fn begin_early(
        options: WindowOptions,
        cache: impl FnOnce() -> Option<Vec<u8>> + Send + 'static,
    ) {
        let mut slot = EARLY
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_some() {
            return;
        }
        *slot = Some(std::thread::spawn(move || {
            let stages = crate::stages::StageClock::from_env();
            let cache = cache();
            stages.mark("pipeline_cache_load");
            let outcome = Core::new(options.device)
                .map_err(GpuError::from)
                .and_then(|core| {
                    stages.mark("create_device");
                    build_gpu(&core, cache.as_deref(), true).map(|gpu| (core, gpu))
                });
            stages.mark("build_gpu");
            if let Ok((_, gpu)) = &outcome {
                stages.note(&format!("{:?}", gpu.renderer.pipeline_stats()));
            }
            Handoff(outcome.map(|(core, gpu)| {
                let mut backend = Self::assemble(options, core, gpu);
                stages.mark("assemble");
                backend.startup_deferred = true;
                backend
            }))
        }));
    }

    pub fn take_early() -> Option<Result<Self, GpuError>> {
        let handle = EARLY
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()?;
        let Handoff(outcome) = handle.join().ok()?;
        Some(outcome)
    }

    pub fn finish_startup(&mut self) {
        if !std::mem::take(&mut self.startup_deferred) {
            return;
        }
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        gpu.renderer.close_startup();
        self.startup.extend(gpu.renderer.deferred_pipelines());
    }

    pub fn startup_pending(&self) -> bool {
        !self.startup.is_empty()
    }

    pub fn startup_step(&mut self) {
        let stages = crate::stages::StageClock::from_env();
        let Some(kind) = self.startup.pop_front() else {
            return;
        };
        let outcome = match self.gpu.as_mut() {
            Some(gpu) => gpu.renderer.warm_one(kind).map_err(GpuError::from),
            None => Ok(()),
        };
        if outcome.is_err() {
            self.startup.clear();
        }
        stages.mark(&format!("startup_shader {kind:?}"));
    }

    fn assemble(options: WindowOptions, core: Core, gpu: Gpu) -> Self {
        Self {
            startup_deferred: false,
            startup: VecDeque::new(),
            options,
            capabilities: describe(&core),
            surfaces: HashMap::new(),
            gpu: Some(gpu),
            core: Some(core),
            pending_uploads: Vec::new(),
            mirror: TextureMirror::default(),
            next_surface: 0,
            lost: false,
            stats: WindowStats::default(),
            phases: PhaseTimes::default(),
            listener: None,
        }
    }

    pub fn set_recovery_listener(&mut self, listener: Box<dyn FnMut(RecoveryNotice)>) {
        self.listener = Some(listener);
    }

    pub fn core(&self) -> Option<&Core> {
        self.core.as_ref()
    }

    pub fn core_mut(&mut self) -> Option<&mut Core> {
        self.core.as_mut()
    }

    pub fn device_status(&self) -> DeviceStatus {
        match self.lost {
            true => DeviceStatus::Lost,
            false => DeviceStatus::Healthy,
        }
    }

    pub fn generation(&self) -> u32 {
        self.core.as_ref().map(Core::generation).unwrap_or(0)
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

    pub fn debug_errors(&self) -> Vec<String> {
        self.core
            .as_ref()
            .map(|core| crate::scene::debug::drain(core.device()))
            .unwrap_or_default()
    }

    pub fn memory_breakdown(&self) -> Result<MemoryBreakdown, GpuError> {
        let core = self.core.as_ref().ok_or_else(no_device)?;
        let memory = core.memory()?;
        let swapchain_bytes = self
            .surfaces
            .values()
            .map(|entry| {
                let (width, height) = entry.swapchain.texture_extent();
                width as u64 * height as u64 * 4 * entry.swapchain.buffer_count() as u64
            })
            .sum();
        let gpu = self.gpu.as_ref();
        Ok(MemoryBreakdown {
            swapchain_bytes,
            transient_heap_bytes: gpu.map_or(0, |gpu| gpu.renderer.transient_bytes()),
            retired_transient_bytes: 0,
            frame_upload_bytes: gpu.map_or(0, |gpu| gpu.renderer.constant_bytes()),
            scene_upload_bytes: gpu.map_or(0, |gpu| gpu.renderer.instance_bytes()),
            atlas_resident_bytes: gpu.map_or(0, |gpu| {
                let stats = gpu.textures.stats();
                stats.resident_bytes + stats.pooled_bytes
            }),
            atlas_staging_bytes: 0,
            surface_mirror_bytes: gpu.map_or(0, |gpu| gpu.renderer.mirror_bytes()),
            local_usage: memory.local.usage,
            local_budget: memory.local.budget,
        })
    }

    pub fn trim(&mut self, trim: Trim) -> Result<u64, GpuError> {
        let core = self.core.as_mut().ok_or_else(no_device)?;
        let trim = core.trim(trim)?;
        self.release(&trim)
    }

    pub fn set_half_rate(&mut self, surface: SurfaceId, half_rate: bool) {
        if let Some(entry) = self.surfaces.get_mut(&surface) {
            entry.half_rate = half_rate;
        }
    }

    pub fn set_visibility(&mut self, visibility: Visibility) -> Result<Trim, GpuError> {
        let core = self.core.as_mut().ok_or_else(no_device)?;
        let trim = core.set_visibility(visibility)?;
        self.release(&trim)?;
        Ok(trim)
    }

    pub fn settle_surfaces(&mut self) -> Result<(), GpuError> {
        let core = self.core.as_ref().ok_or_else(no_device)?;
        let mut failure = None;
        for entry in self.surfaces.values_mut() {
            match core.settle_swapchain(&mut entry.swapchain) {
                Ok(true) => {
                    entry.damage.reset();
                    entry.rendered = None;
                    entry.pending = false;
                }
                Ok(false) => {}
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        match failure {
            Some(error) => {
                self.note_failure();
                Err(error.into())
            }
            None => Ok(()),
        }
    }

    pub fn service_budget(&mut self) -> Result<Option<BudgetStatus>, GpuError> {
        let core = self.core.as_mut().ok_or_else(no_device)?;
        let status = core.service_budget()?;
        if let Some(status) = &status {
            self.release(&status.applied)?;
        }
        Ok(status)
    }

    fn release(&mut self, trim: &Trim) -> Result<u64, GpuError> {
        let (Some(core), Some(gpu)) = (self.core.as_ref(), self.gpu.as_mut()) else {
            return Err(no_device());
        };
        let freed = gpu.renderer.trim(trim);
        if trim.free_transients {
            gpu.textures.drain_pool();
        }
        if trim.evict_idle_resources {
            core.trim_device();
        }
        Ok(freed)
    }

    pub fn gpu_frame_time(&self) -> Option<Duration> {
        self.gpu.as_ref().and_then(|gpu| gpu.renderer.gpu_time())
    }

    pub fn gpu_pass_times(&self) -> &[PassTime] {
        self.gpu
            .as_ref()
            .map(|gpu| gpu.renderer.gpu_pass_times())
            .unwrap_or(&[])
    }

    pub fn late_pipelines(&self) -> Vec<String> {
        self.gpu
            .as_ref()
            .map(|gpu| {
                gpu.renderer
                    .late_pipelines()
                    .iter()
                    .map(|kind| format!("{kind:?}"))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn pipeline_stats(&self) -> Option<PipelineStats> {
        self.gpu.as_ref().map(|gpu| gpu.renderer.pipeline_stats())
    }

    pub fn pipeline_cache(&self) -> Option<Vec<u8>> {
        self.gpu
            .as_ref()
            .and_then(|gpu| gpu.renderer.pipeline_cache())
    }

    pub fn persistent_stats(&self) -> Option<PersistentStats> {
        self.gpu.as_ref().map(|gpu| gpu.textures.stats())
    }

    pub fn buffer_age(&self, surface: SurfaceId) -> Option<u32> {
        let entry = self.surfaces.get(&surface)?;
        Some(entry.damage.age(entry.swapchain.current_index()))
    }

    pub fn repaint_region(&self, surface: SurfaceId, damage: &[Rect]) -> Option<Damage> {
        let entry = self.surfaces.get(&surface)?;
        let (width, height) = entry.swapchain.extent();
        let current = Damage::from_rects(damage, Extent { width, height });
        Some(
            entry
                .damage
                .repaint(entry.swapchain.current_index(), &current),
        )
    }

    pub fn repaint_bounds(&self, surface: SurfaceId, damage: &[Rect]) -> Option<Rect> {
        let region = self.repaint_region(surface, damage)?;
        bounding_rect(&region).map(|rect| Rect {
            x: rect.left as f32,
            y: rect.top as f32,
            w: (rect.right - rect.left) as f32,
            h: (rect.bottom - rect.top) as f32,
        })
    }

    pub fn surface_kind(&self, surface: SurfaceId) -> Option<SurfaceKind> {
        self.surfaces.get(&surface).map(|entry| entry.kind)
    }

    pub fn create_composition_surface(
        &mut self,
        window: aqua_protocol::WindowId,
        extent: Extent,
    ) -> Result<SurfaceId, GpuError> {
        let core = self.core.as_ref().ok_or_else(no_device)?;
        let surface = match new_surface(core, window, SurfaceKind::Composition, extent) {
            Ok(surface) => surface,
            Err(error) => {
                self.note_failure();
                return Err(error);
            }
        };
        Ok(self.insert_surface(surface))
    }

    fn insert_surface(&mut self, surface: WindowSurface) -> SurfaceId {
        let id = SurfaceId(self.next_surface);
        self.next_surface += 1;
        self.surfaces.insert(id, surface);
        id
    }

    pub fn composition_content(
        &self,
        surface: SurfaceId,
    ) -> Result<windows::core::IUnknown, GpuError> {
        let entry = self
            .surfaces
            .get(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        match entry.kind {
            SurfaceKind::Composition => Ok(entry.swapchain.content()?),
            SurfaceKind::Hwnd(_) => Err(GpuError::Unsupported(
                "composition content of a window surface",
            )),
        }
    }

    pub fn refresh_interval(&self, surface: SurfaceId) -> Option<Duration> {
        self.surfaces
            .get(&surface)
            .map(|entry| entry.pacer.interval())
    }

    pub fn pacing(&self, surface: SurfaceId) -> Option<PaceReport> {
        self.surfaces
            .get(&surface)
            .map(|entry| entry.pacer.report())
    }

    pub fn until_next_vblank(&self) -> Option<Duration> {
        timing::query().map(|vblank| vblank.until_next)
    }

    pub fn mirrored_textures(&self) -> usize {
        self.mirror.textures()
    }

    pub fn execute_damaged(
        &mut self,
        plan: &FramePlan,
        inputs: &FrameInputs,
        damage: &[Rect],
    ) -> Result<FrameReport, GpuError> {
        let outcome = self.execute_region(plan, inputs.surface, Some(damage));
        if outcome.is_err() {
            self.note_failure();
        }
        outcome
    }

    pub fn inject_device_removal(&self) -> Result<(), GpuError> {
        let core = self.core.as_ref().ok_or_else(no_device)?;
        core.inject_device_removal();
        Ok(())
    }

    fn note_failure(&mut self) {
        let removed = self
            .core
            .as_ref()
            .map(|core| core.check_removed().is_err())
            .unwrap_or(false);
        self.lost |= removed;
    }

    fn execute_region(
        &mut self,
        plan: &FramePlan,
        surface: SurfaceId,
        damage: Option<&[Rect]>,
    ) -> Result<FrameReport, GpuError> {
        let started = Instant::now();
        if self.stats.frames < 2 {
            crate::stages::StageClock::from_env().mark("execute_begin");
        }
        match self.gpu.as_ref() {
            Some(gpu) => gpu.renderer.validate(plan)?,
            None => validate(plan)?,
        }
        let validated = started.elapsed();
        let core = self.core.as_ref().ok_or_else(no_device)?;
        let gpu = self.gpu.as_mut().ok_or_else(no_device)?;
        let entry = self
            .surfaces
            .get_mut(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        let (width, height) = entry.swapchain.extent();
        let extent = Extent { width, height };
        if plan.extent != extent {
            return Err(GpuError::ExtentMismatch {
                plan: plan.extent,
                surface: extent,
            });
        }
        core.check_removed()?;
        let index = entry.swapchain.current_index();
        let (repaint, current) = match damage {
            None => (None, None),
            Some(rects) => {
                let mut current = Damage::from_rects(rects, extent);
                let region = entry.damage.repaint(index, &current);
                let repaint = bounding_rect(&region).map(|bounds| {
                    let region = plan.repaint_bounds(Rect::from_edges(
                        bounds.left as f32,
                        bounds.top as f32,
                        bounds.right as f32,
                        bounds.bottom as f32,
                    ));
                    let expanded = RECT {
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
        let context = core.context();
        let reserve_started = Instant::now();
        let created = gpu
            .renderer
            .reserve(plan, Precision::EightBit)
            .map_err(GpuError::from)?;
        let reserve = reserve_started.elapsed();
        let record_started = Instant::now();
        gpu.textures
            .apply(context, &self.pending_uploads)
            .map_err(GpuError::from)?;
        self.pending_uploads.clear();
        let (texture, rtv) = match (entry.swapchain.texture(), entry.swapchain.rtv()) {
            (Some(texture), Some(rtv)) => (texture, rtv),
            _ => {
                return Err(GpuError::DeviceFailure(
                    "swapchain has no back buffer".into(),
                ));
            }
        };
        let target = SceneTarget {
            texture,
            rtv,
            srv: entry.swapchain.srv(),
            format: RENDER_FORMAT,
            extent: {
                let (width, height) = entry.swapchain.texture_extent();
                Extent { width, height }
            },
        };
        let stats = gpu
            .renderer
            .record(context, plan, &target, &gpu.textures, repaint)
            .map_err(GpuError::from)?;
        let recorded_in = record_started.elapsed();
        if self.stats.frames < 2 {
            crate::stages::StageClock::from_env().mark("execute_end");
        }
        self.phases = PhaseTimes {
            validate: validated,
            slot_wait: Duration::ZERO,
            reserve,
            record: recorded_in,
            submit: Duration::ZERO,
            present: self.phases.present,
            resize: self.phases.resize,
            reserve_created: created,
        };
        entry.rendered = Some(index);
        entry.pending = true;
        entry.frame_damage = current;
        self.stats.frames += 1;
        match repaint {
            Some(rect) => {
                let area =
                    (rect.right - rect.left).max(0) as i64 * (rect.bottom - rect.top).max(0) as i64;
                let total = extent.width as i64 * extent.height as i64;
                self.stats.scissored_frames += 1;
                self.stats.last_repaint_area = area;
                self.stats.skipped_pixels += (total - area).max(0) as u64;
            }
            None => self.stats.last_repaint_area = extent.width as i64 * extent.height as i64,
        }
        Ok(FrameReport {
            passes: stats.passes,
            instances: stats.instances,
            cpu_time: started.elapsed(),
            device: DeviceStatus::Healthy,
            barriers: 0,
            transient_bytes: gpu.renderer.transient_bytes(),
            gpu_time: gpu.renderer.gpu_time(),
        })
    }

    fn present_inner(
        &mut self,
        surface: SurfaceId,
        hint: PresentHint,
        damage: Damage,
    ) -> Result<PresentTimes, GpuError> {
        let started = Instant::now();
        let core = self.core.as_ref().ok_or_else(no_device)?;
        core.check_removed()?;
        let entry = self
            .surfaces
            .get_mut(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        let buffer = match (entry.pending, entry.rendered) {
            (true, Some(buffer)) => buffer,
            _ => {
                return Err(GpuError::DeviceFailure(
                    "surface has no executed frame".into(),
                ));
            }
        };
        let mode = match hint {
            PresentHint::Immediate => PresentMode::Immediate,
            PresentHint::Vsync if entry.half_rate => PresentMode::HalfVsync,
            PresentHint::Vsync => PresentMode::Vsync,
        };
        let damage = match entry.damage.has_presented() {
            true => entry.frame_damage.take().unwrap_or(damage),
            false => {
                entry.frame_damage = None;
                Damage::Full
            }
        };
        let mut dirty = [RECT::default(); 16];
        let rects = damage.present_rects();
        let count = rects.len().min(dirty.len());
        rects
            .iter()
            .take(count)
            .zip(dirty.iter_mut())
            .for_each(|(rect, slot)| *slot = to_rect(*rect));
        let present_started = Instant::now();
        let stages = crate::stages::StageClock::from_env();
        if self.stats.presents < 3 {
            stages.mark("present_enter");
        }
        let status = entry.swapchain.present(mode, &dirty[..count])?;
        if self.stats.presents < 3 {
            stages.mark("present_returned");
        }
        self.phases.present = present_started.elapsed();
        entry.pending = false;
        let now = Instant::now();
        match status {
            PresentStatus::Presented => {
                entry.damage.commit(buffer, damage);
                self.stats.presents += 1;
                let monitor = timing::monitor_of(owner(entry.window, entry.kind));
                if monitor != entry.monitor {
                    entry.monitor = monitor;
                    entry.pacer.set_interval(timing::monitor_interval(monitor));
                    entry.last_present = None;
                }
                if let Some(previous) = entry.last_present.replace(now) {
                    entry.pacer.observe(now - previous);
                }
            }
            PresentStatus::Occluded => {
                entry.damage.reset();
                entry.pacer.restart();
                entry.last_present = None;
                self.stats.occluded += 1;
            }
        }
        Ok(PresentTimes {
            queued: started.elapsed(),
        })
    }

    fn recover_inner(&mut self) -> Result<RecoveryReport, GpuError> {
        let cache = self.pipeline_cache();
        let layout: Vec<(SurfaceId, aqua_protocol::WindowId, SurfaceKind, Extent)> = self
            .surfaces
            .iter()
            .map(|(id, entry)| {
                let (width, height) = entry.swapchain.extent();
                (*id, entry.window, entry.kind, Extent { width, height })
            })
            .collect();
        self.surfaces.clear();
        self.gpu = None;
        self.pending_uploads.clear();
        let (core, generation) = match self.core.take() {
            Some(core) => {
                let (core, recovered) = core.recover()?;
                (core, recovered.generation)
            }
            None => {
                let device = self
                    .options
                    .device
                    .with_preference(hal::selection::AdapterPreference::Warp);
                (Core::new(device)?, self.generation() + 1)
            }
        };
        let gpu = build_gpu(&core, cache.as_deref(), false)?;
        for (id, window, kind, extent) in &layout {
            self.surfaces
                .insert(*id, new_surface(&core, *window, *kind, *extent)?);
        }
        self.capabilities = describe(&core);
        let textures = match self.options.mirror_textures {
            true => {
                let count = self.mirror.textures() as u32;
                if count > 0 {
                    self.pending_uploads.push(self.mirror.replay());
                }
                TextureRecovery::Replayed { textures: count }
            }
            false => TextureRecovery::HostMustReupload,
        };
        let notice = RecoveryNotice {
            generation,
            software: self.capabilities.features.software,
            textures,
        };
        self.core = Some(core);
        self.gpu = Some(gpu);
        self.lost = false;
        self.stats.recoveries += 1;
        if let Some(listener) = self.listener.as_mut() {
            listener(notice);
        }
        Ok(RecoveryReport {
            rebuilt_surfaces: layout.len() as u32,
        })
    }

    fn read_inner(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        let core = self.core.as_ref().ok_or_else(no_device)?;
        let entry = self
            .surfaces
            .get(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        if !entry.pending {
            return Err(GpuError::DeviceFailure(
                "surface has no unpresented frame".into(),
            ));
        }
        let (width, height) = entry.swapchain.extent();
        let texture = entry.swapchain.texture().ok_or(GpuError::DeviceFailure(
            "swapchain has no back buffer".into(),
        ))?;
        let rgba8 =
            crate::scene::read_texture(core.device(), core.context(), texture, |bytes, pitch| {
                bgra8_rows_to_rgba8(bytes, pitch, width as usize, height as usize)
            })?;
        Ok(SurfacePixels {
            extent: Extent { width, height },
            rgba8,
        })
    }
}

impl Drop for WindowBackend {
    fn drop(&mut self) {
        self.surfaces.clear();
        self.gpu = None;
        if let Some(core) = self.core.as_ref() {
            core.release_deferred();
        }
    }
}

impl GpuBackend for WindowBackend {
    type Error = GpuError;

    fn capabilities(&self) -> &GpuCapabilities {
        &self.capabilities
    }

    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
        match target {
            SurfaceTarget::Native(native) => {
                let core = self.core.as_ref().ok_or_else(no_device)?;
                let surface = new_surface(
                    core,
                    native.window,
                    SurfaceKind::Hwnd(native.hwnd),
                    native.extent,
                );
                match surface {
                    Ok(surface) => Ok(self.insert_surface(surface)),
                    Err(error) => {
                        self.note_failure();
                        Err(error)
                    }
                }
            }
            SurfaceTarget::Window(_) => Err(GpuError::Unsupported(
                "window surfaces without a native handle",
            )),
            SurfaceTarget::Offscreen(_) => Err(GpuError::Unsupported(
                "offscreen surfaces on the window backend",
            )),
        }
    }

    fn upload(&mut self, batch: UploadBatch) -> Result<(), GpuError> {
        if !batch.textures.is_empty() {
            if self.options.mirror_textures {
                self.mirror.apply(&batch);
            }
            self.pending_uploads.push(batch);
        }
        Ok(())
    }

    fn execute(&mut self, plan: &FramePlan, inputs: &FrameInputs) -> Result<FrameReport, GpuError> {
        let outcome = self.execute_region(plan, inputs.surface, None);
        if outcome.is_err() {
            self.note_failure();
        }
        outcome
    }

    fn present(&mut self, surface: SurfaceId, hint: PresentHint) -> Result<PresentTimes, GpuError> {
        let outcome = self.present_inner(surface, hint, Damage::Full);
        if outcome.is_err() {
            self.note_failure();
        }
        outcome
    }

    fn present_with_damage(
        &mut self,
        surface: SurfaceId,
        hint: PresentHint,
        damage: &[Rect],
    ) -> Result<PresentTimes, GpuError> {
        let extent = self
            .surfaces
            .get(&surface)
            .map(|entry| {
                let (width, height) = entry.swapchain.extent();
                Extent { width, height }
            })
            .ok_or(GpuError::UnknownSurface(surface))?;
        let outcome = self.present_inner(surface, hint, Damage::from_rects(damage, extent));
        if outcome.is_err() {
            self.note_failure();
        }
        outcome
    }

    fn recover(&mut self) -> Result<RecoveryReport, GpuError> {
        self.recover_inner()
    }

    fn resize_surface(&mut self, surface: SurfaceId, extent: Extent) -> Result<(), GpuError> {
        let core = self.core.as_ref().ok_or_else(no_device)?;
        let entry = self
            .surfaces
            .get_mut(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        let resize_started = Instant::now();
        let outcome = core.resize_swapchain(&mut entry.swapchain, extent.width, extent.height);
        self.phases.resize = resize_started.elapsed();
        match outcome {
            Ok(changed) => {
                if changed {
                    entry.damage.reset();
                    entry.rendered = None;
                    entry.pending = false;
                }
                Ok(())
            }
            Err(error) => {
                self.note_failure();
                Err(error.into())
            }
        }
    }

    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<(), GpuError> {
        let found = self.surfaces.remove(&surface).is_some();
        if let Some(core) = self.core.as_ref() {
            core.release_deferred();
        }
        match found {
            true => Ok(()),
            false => Err(GpuError::UnknownSurface(surface)),
        }
    }

    fn wait_for_frame_slot(&mut self, surface: SurfaceId) -> Result<(), GpuError> {
        let entry = self
            .surfaces
            .get(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        entry.swapchain.wait_for_frame(FRAME_WAIT_MS);
        Ok(())
    }
}

impl GpuReadback for WindowBackend {
    fn read_surface(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        let outcome = self.read_inner(surface);
        if outcome.is_err() {
            self.note_failure();
        }
        outcome
    }
}
