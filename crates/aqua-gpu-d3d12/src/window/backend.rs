use std::collections::HashMap;
use std::time::{Duration, Instant};

use aqua_core::Oklch;
use aqua_core::geometry::Rect;
use aqua_graph::{Extent, FramePlan, PaintCommand, Quad, plan_paint, validate};
use aqua_protocol::{
    DeviceStatus, FeatureTier, Features, FrameInputs, FrameReport, GpuBackend, GpuCapabilities,
    GpuError, GpuReadback, PresentHint, PresentTimes, RecoveryReport, SurfaceId, SurfacePixels,
    SurfaceTarget, UploadBatch, WindowId,
};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::damage::{Damage, DamageTracker, PixelRect};
use super::mirror::TextureMirror;
use super::pacing::{PaceReport, Pacer};
use super::timing;
use crate::hal::budget::{Trim, Visibility};
use crate::hal::{
    self, BudgetStatus, Core, DeviceOptions, PresentMode, PresentStatus, RENDER_FORMAT, Swapchain,
};
use crate::half::bgra8_rows_to_rgba8;
use crate::scene::util::{footprint_location, texture_location};
use crate::scene::{
    PassTime, PersistentStats, PersistentTextures, PipelineKind as Kind, PipelineStats,
    SceneRenderer, SceneTarget,
};

const FRAME_WAIT_MS: u32 = 1000;
const AMD_VENDOR: u32 = 0x1002;
const AMD_TRACKER_FRAMES: u32 = 160;
const AMD_TRACKER_CHUNK: u32 = 16;
const AMD_TRACKER_EXTENT: Extent = Extent {
    width: 64,
    height: 64,
};

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
}

struct Handoff<T>(T);

unsafe impl<T> Send for Handoff<T> {}

type Early = std::thread::JoinHandle<Handoff<Result<WindowBackend, GpuError>>>;

static EARLY: std::sync::Mutex<Option<Early>> = std::sync::Mutex::new(None);

struct Gpu {
    list: ID3D12GraphicsCommandList,
    renderer: SceneRenderer,
    textures: PersistentTextures,
}

enum StartupStep {
    Pipeline(Kind, DXGI_FORMAT),
    TrackerOpen,
    TrackerFrames(u32),
    TrackerClose,
}

struct Tracker {
    plan: FramePlan,
    surface: SurfaceId,
}

pub struct WindowBackend {
    startup_deferred: bool,
    startup: std::collections::VecDeque<StartupStep>,
    tracker: Option<Tracker>,
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

fn build_gpu(
    core: &Core,
    options: &WindowOptions,
    cache: Option<&[u8]>,
    deferred_layers: bool,
) -> Result<Gpu, GpuError> {
    let slots = options.device.frames_in_flight.max(2);
    let stages = crate::stages::StageClock::from_env();
    let mut renderer = SceneRenderer::with_pipeline_cache(core.device(), slots, cache)?;
    stages.mark("  renderer_new");
    match deferred_layers {
        true => renderer.warm_first_frame(&[RENDER_FORMAT])?,
        false => renderer.warm(&[DXGI_FORMAT_R16G16B16A16_FLOAT, RENDER_FORMAT])?,
    }
    stages.mark("  warm");
    let list = core.create_command_list()?;
    stages.mark("  command_list");
    Ok(Gpu {
        list,
        renderer,
        textures: PersistentTextures::new(core.device()),
    })
}

fn tracker_plan() -> Result<FramePlan, GpuError> {
    let quad = PaintCommand::Quad(Quad {
        bounds: Rect {
            x: 0.0,
            y: 0.0,
            w: AMD_TRACKER_EXTENT.width as f32,
            h: AMD_TRACKER_EXTENT.height as f32,
        },
        radius: 0.0,
        border_width: 0.0,
        fill: Oklch::WHITE,
        border_color: Oklch::BLACK,
    });
    plan_paint(AMD_TRACKER_EXTENT, Oklch::BLACK, &[quad])
        .map_err(|_| GpuError::Unsupported("amd frame tracker plan"))
}

fn runs_amd_frame_tracker(core: &Core) -> bool {
    let adapter = &core.info().adapter;
    adapter.vendor_id == AMD_VENDOR && adapter.kind == hal::selection::Adapter::Hardware
}

fn describe(core: &Core) -> GpuCapabilities {
    let info = core.info();
    GpuCapabilities {
        backend: "d3d12".into(),
        adapter: info.adapter.name.clone(),
        tier: FeatureTier::Baseline,
        max_texture_extent: D3D12_REQ_TEXTURE2D_U_OR_V_DIMENSION,
        heap_alignment: D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT as u64,
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

fn dirty_rects(damage: &Damage) -> Vec<RECT> {
    damage
        .present_rects()
        .iter()
        .map(|rect| RECT {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
        .collect()
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
    })
}

fn owner(window: aqua_protocol::WindowId, kind: SurfaceKind) -> isize {
    match kind {
        SurfaceKind::Hwnd(hwnd) => hwnd as isize,
        SurfaceKind::Composition => window.raw() as isize,
    }
}

fn bounding_rect(damage: &Damage) -> Option<RECT> {
    let rects = damage.present_rects();
    rects
        .iter()
        .copied()
        .reduce(PixelRect::union)
        .map(|rect| RECT {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
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
        let gpu = build_gpu(&core, &options, cache, false)?;
        let mut backend = Self::assemble(options, core, gpu);
        backend.settle_amd_frame_tracker();
        Ok(backend)
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
                    build_gpu(&core, &options, cache.as_deref(), true).map(|gpu| (core, gpu))
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
        let amd = self.core.as_ref().is_some_and(runs_amd_frame_tracker);
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        gpu.renderer.close_startup();
        let pipelines = gpu
            .renderer
            .deferred_pipelines(&[RENDER_FORMAT, DXGI_FORMAT_R16G16B16A16_FLOAT]);
        self.startup.extend(
            pipelines
                .into_iter()
                .map(|(kind, format)| StartupStep::Pipeline(kind, format)),
        );
        if amd {
            self.startup.push_back(StartupStep::TrackerOpen);
            let chunks = AMD_TRACKER_FRAMES.div_ceil(AMD_TRACKER_CHUNK);
            (0..chunks).for_each(|index| {
                let frames = AMD_TRACKER_CHUNK.min(AMD_TRACKER_FRAMES - index * AMD_TRACKER_CHUNK);
                self.startup.push_back(StartupStep::TrackerFrames(frames));
            });
            self.startup.push_back(StartupStep::TrackerClose);
        }
    }

    pub fn startup_pending(&self) -> bool {
        !self.startup.is_empty()
    }

    pub fn startup_step(&mut self) {
        let stages = crate::stages::StageClock::from_env();
        let Some(step) = self.startup.pop_front() else {
            return;
        };
        let label = match &step {
            StartupStep::Pipeline(kind, format) => format!("startup_pso {kind:?}-{}", format.0),
            StartupStep::TrackerOpen => "startup_tracker_open".to_owned(),
            StartupStep::TrackerFrames(frames) => format!("startup_tracker_frames {frames}"),
            StartupStep::TrackerClose => "startup_tracker_close".to_owned(),
        };
        let (stats, phases) = (self.stats, self.phases);
        let outcome = match step {
            StartupStep::Pipeline(kind, format) => match self.gpu.as_mut() {
                Some(gpu) => gpu.renderer.warm_one(kind, format).map_err(GpuError::from),
                None => Ok(()),
            },
            StartupStep::TrackerOpen => self.open_tracker(),
            StartupStep::TrackerFrames(frames) => self.present_tracker(frames),
            StartupStep::TrackerClose => self.close_tracker(),
        };
        self.stats = stats;
        self.phases = phases;
        if outcome.is_err() {
            self.startup
                .retain(|step| matches!(step, StartupStep::Pipeline(..)));
            let _ = self.close_tracker();
        }
        stages.mark(&label);
    }

    fn open_tracker(&mut self) -> Result<(), GpuError> {
        let plan = tracker_plan()?;
        let window = WindowId::new(1).ok_or(GpuError::Unsupported("window id"))?;
        let surface = self.create_composition_surface(window, AMD_TRACKER_EXTENT)?;
        self.tracker = Some(Tracker { plan, surface });
        Ok(())
    }

    fn present_tracker(&mut self, frames: u32) -> Result<(), GpuError> {
        let Some((plan, surface)) = self
            .tracker
            .as_ref()
            .map(|tracker| (tracker.plan.clone(), tracker.surface))
        else {
            return Ok(());
        };
        (0..frames).try_for_each(|_| {
            self.wait_for_frame_slot(surface)?;
            self.execute(&plan, &FrameInputs::new(surface))?;
            self.present(surface, PresentHint::Immediate).map(|_| ())
        })
    }

    fn close_tracker(&mut self) -> Result<(), GpuError> {
        match self.tracker.take() {
            Some(tracker) => self.destroy_surface(tracker.surface),
            None => Ok(()),
        }
    }

    fn settle_amd_frame_tracker(&mut self) {
        if !self.core.as_ref().is_some_and(runs_amd_frame_tracker) {
            return;
        }
        let (stats, phases, next_surface) = (self.stats, self.phases, self.next_surface);
        let _ = self.present_tracker_frames();
        self.stats = stats;
        self.phases = phases;
        self.next_surface = next_surface;
    }

    fn present_tracker_frames(&mut self) -> Result<(), GpuError> {
        let plan = tracker_plan()?;
        let window = WindowId::new(1).ok_or(GpuError::Unsupported("window id"))?;
        let surface = self.create_composition_surface(window, AMD_TRACKER_EXTENT)?;
        let presented = (0..AMD_TRACKER_FRAMES).try_for_each(|_| {
            self.wait_for_frame_slot(surface)?;
            self.execute(&plan, &FrameInputs::new(surface))?;
            self.present(surface, PresentHint::Immediate).map(|_| ())
        });
        let destroyed = self.destroy_surface(surface);
        presented.and(destroyed)
    }

    fn assemble(options: WindowOptions, core: Core, gpu: Gpu) -> Self {
        Self {
            startup_deferred: false,
            startup: std::collections::VecDeque::new(),
            tracker: None,
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

    pub fn memory_breakdown(&self) -> Result<MemoryBreakdown, GpuError> {
        let core = self.core.as_ref().ok_or(no_device())?;
        let memory = core.memory()?;
        let swapchain_bytes = self
            .surfaces
            .values()
            .map(|entry| {
                let (width, height) = entry.swapchain.extent();
                width as u64 * height as u64 * 4 * entry.swapchain.buffer_count() as u64
            })
            .sum();
        let (transient, retired, scene_upload, atlas) = match self.gpu.as_ref() {
            Some(gpu) => (
                gpu.renderer.transient_heap_bytes(),
                gpu.renderer.retired_transient_bytes(),
                gpu.renderer.slot_upload_bytes(),
                gpu.textures.stats(),
            ),
            None => Default::default(),
        };
        Ok(MemoryBreakdown {
            swapchain_bytes,
            transient_heap_bytes: transient,
            retired_transient_bytes: retired,
            frame_upload_bytes: core.upload_capacity(),
            scene_upload_bytes: scene_upload,
            atlas_resident_bytes: atlas.resident_bytes,
            atlas_staging_bytes: atlas.staging_bytes,
            local_usage: memory.local.usage,
            local_budget: memory.local.budget,
        })
    }

    pub fn trim(&mut self, trim: Trim) -> Result<u64, GpuError> {
        let core = self.core.as_mut().ok_or(no_device())?;
        core.trim(trim)?;
        self.release(&trim)
    }

    pub fn set_visibility(&mut self, visibility: Visibility) -> Result<Trim, GpuError> {
        let core = self.core.as_mut().ok_or(no_device())?;
        let trim = core.set_visibility(visibility)?;
        self.release(&trim)?;
        Ok(trim)
    }

    pub fn service_budget(&mut self) -> Result<Option<BudgetStatus>, GpuError> {
        let core = self.core.as_mut().ok_or(no_device())?;
        let status = core.service_budget()?;
        if let Some(status) = &status {
            self.release(&status.applied)?;
        }
        Ok(status)
    }

    fn release(&mut self, trim: &Trim) -> Result<u64, GpuError> {
        let (Some(core), Some(gpu)) = (self.core.as_mut(), self.gpu.as_mut()) else {
            return Err(no_device());
        };
        if !trim.free_transients && !trim.shrink_upload_ring {
            return Ok(0);
        }
        core.wait_idle()?;
        let completed = core.completed_fence();
        gpu.renderer.retire(completed);
        let freed = gpu.renderer.trim(trim, completed);
        gpu.renderer.retire(completed);
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
                    .map(|(kind, format)| format!("{kind:?}/{format}"))
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
        let index = entry.swapchain.current().index;
        Some(entry.damage.age(index))
    }

    pub fn repaint_region(&self, surface: SurfaceId, damage: &[Rect]) -> Option<Damage> {
        let entry = self.surfaces.get(&surface)?;
        let (width, height) = entry.swapchain.extent();
        let index = entry.swapchain.current().index;
        let current = Damage::from_rects(damage, Extent { width, height });
        Some(entry.damage.repaint(index, &current))
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
        let core = self.core.as_ref().ok_or(no_device())?;
        let surface = match new_surface(core, window, SurfaceKind::Composition, extent) {
            Ok(surface) => surface,
            Err(error) => {
                self.note_failure();
                return Err(error);
            }
        };
        let id = SurfaceId(self.next_surface);
        self.next_surface += 1;
        self.surfaces.insert(id, surface);
        Ok(id)
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
        let core = self.core.as_ref().ok_or(no_device())?;
        Ok(core.inject_device_removal()?)
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
        match self.gpu.as_mut() {
            Some(gpu) => gpu.renderer.validate(plan)?,
            None => validate(plan)?,
        }
        let validated = started.elapsed();
        let core = self.core.as_mut().ok_or(no_device())?;
        let gpu = self.gpu.as_mut().ok_or(no_device())?;
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
        let (repaint, current) = match damage {
            None => (None, None),
            Some(rects) => {
                let current = Damage::from_rects(rects, extent);
                let region = entry
                    .damage
                    .repaint(entry.swapchain.current().index, &current);
                (bounding_rect(&region), Some(current))
            }
        };
        let frame = core.begin_frame()?;
        let completed = core.completed_fence();
        gpu.renderer.retire(completed);
        gpu.textures.retire(completed);
        let reserve_started = Instant::now();
        let created = gpu
            .renderer
            .reserve(plan, core.timeline().next_value())
            .map_err(GpuError::from)?;
        let reserve = reserve_started.elapsed();
        if created && crate::scene::debug::requested() {
            core.wait_idle()?;
        }
        let frame_fence = core.timeline().next_value();
        let back = entry.swapchain.current();
        let target = SceneTarget {
            resource: back.resource,
            rtv: back.rtv,
            format: RENDER_FORMAT,
            rest_state: D3D12_RESOURCE_STATE_PRESENT,
        };
        let record_started = Instant::now();
        unsafe { gpu.list.Reset(&frame.allocator, None) }.map_err(hal::Error::from)?;
        let recorded = gpu
            .textures
            .record(&gpu.list, &self.pending_uploads, frame_fence, completed)
            .and_then(|_| {
                let textures = &gpu.textures;
                gpu.renderer.record_region(
                    &gpu.list,
                    frame.index,
                    frame_fence,
                    plan,
                    &target,
                    &|handle| textures.view(handle),
                    repaint,
                )
            });
        let stats = match recorded {
            Ok(stats) => {
                self.pending_uploads.clear();
                stats
            }
            Err(error) => {
                let _ = unsafe { gpu.list.Close() };
                return Err(error.into());
            }
        };
        unsafe { gpu.list.Close() }.map_err(hal::Error::from)?;
        let recorded_in = record_started.elapsed();
        let submit_started = Instant::now();
        core.execute(&gpu.list)?;
        let signalled = core.end_frame(&frame)?;
        debug_assert_eq!(signalled, frame_fence);
        self.phases = PhaseTimes {
            validate: validated,
            slot_wait: Duration::from_micros(frame.waited_us as u64),
            reserve,
            record: recorded_in,
            submit: submit_started.elapsed(),
            present: self.phases.present,
            resize: self.phases.resize,
            reserve_created: created,
        };
        entry.rendered = Some(back.index);
        entry.pending = true;
        entry.frame_damage = current;
        self.stats.frames += 1;
        if let Some(rect) = repaint {
            let area =
                (rect.right - rect.left).max(0) as i64 * (rect.bottom - rect.top).max(0) as i64;
            let total = extent.width as i64 * extent.height as i64;
            self.stats.scissored_frames += 1;
            self.stats.last_repaint_area = area;
            self.stats.skipped_pixels += (total - area).max(0) as u64;
        } else {
            self.stats.last_repaint_area = extent.width as i64 * extent.height as i64;
        }
        Ok(FrameReport {
            passes: stats.passes,
            instances: stats.instances,
            cpu_time: started.elapsed(),
            device: DeviceStatus::Healthy,
            barriers: stats.barriers,
            transient_bytes: gpu.renderer.transient_heap_bytes(),
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
            PresentHint::Vsync => PresentMode::Vsync,
        };
        let damage = match entry.damage.has_presented() {
            true => entry.frame_damage.take().unwrap_or(damage),
            false => {
                entry.frame_damage = None;
                Damage::Full
            }
        };
        let present_started = Instant::now();
        let status = entry.swapchain.present(mode, &dirty_rects(&damage))?;
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
        let tracker = self.tracker.take().map(|tracker| tracker.surface);
        self.startup.clear();
        let layout: Vec<(SurfaceId, aqua_protocol::WindowId, SurfaceKind, Extent)> = self
            .surfaces
            .iter()
            .filter(|(id, _)| Some(**id) != tracker)
            .map(|(id, entry)| {
                let (width, height) = entry.swapchain.extent();
                (*id, entry.window, entry.kind, Extent { width, height })
            })
            .collect();
        if let Some(core) = self.core.as_mut() {
            let _ = core.wait_idle();
        }
        self.surfaces.clear();
        self.gpu = None;
        self.pending_uploads.clear();
        let old = self.core.take();
        let (core, generation) = match old {
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
        let gpu = build_gpu(&core, &self.options, cache.as_deref(), false)
            .or_else(|_| build_gpu(&core, &self.options, None, false))?;
        for (id, window, kind, extent) in &layout {
            self.surfaces
                .insert(*id, new_surface(&core, *window, *kind, *extent)?);
        }
        self.capabilities = describe(&core);
        let textures = match self.options.mirror_textures {
            true => {
                let replay = self.mirror.replay();
                let count = self.mirror.textures() as u32;
                if count > 0 {
                    self.pending_uploads.push(replay);
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
        self.settle_amd_frame_tracker();
        self.stats.recoveries += 1;
        if let Some(listener) = self.listener.as_mut() {
            listener(notice);
        }
        Ok(RecoveryReport {
            rebuilt_surfaces: layout.len() as u32,
        })
    }
}

fn no_device() -> GpuError {
    GpuError::DeviceFailure("no device: recovery is required".into())
}

impl From<hal::Error> for GpuError {
    fn from(error: hal::Error) -> Self {
        GpuError::DeviceFailure(error.to_string())
    }
}

impl Drop for WindowBackend {
    fn drop(&mut self) {
        if let Some(core) = self.core.as_mut() {
            let _ = core.wait_idle();
        }
        self.surfaces.clear();
        self.gpu = None;
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
                let core = self.core.as_ref().ok_or(no_device())?;
                let surface = new_surface(
                    core,
                    native.window,
                    SurfaceKind::Hwnd(native.hwnd),
                    native.extent,
                );
                let surface = match surface {
                    Ok(surface) => surface,
                    Err(error) => {
                        self.note_failure();
                        return Err(error);
                    }
                };
                let id = SurfaceId(self.next_surface);
                self.next_surface += 1;
                self.surfaces.insert(id, surface);
                Ok(id)
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
        let core = self.core.as_mut().ok_or(no_device())?;
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
        let core = self.core.as_mut().ok_or(no_device())?;
        core.wait_idle()?;
        self.surfaces
            .remove(&surface)
            .map(|_| ())
            .ok_or(GpuError::UnknownSurface(surface))
    }

    fn wait_for_frame_slot(&mut self, surface: SurfaceId) -> Result<(), GpuError> {
        let entry = self
            .surfaces
            .get(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        entry.swapchain.wait_for_frame(FRAME_WAIT_MS)?;
        Ok(())
    }
}

impl WindowBackend {
    fn read_inner(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        let core = self.core.as_mut().ok_or(no_device())?;
        let entry = self
            .surfaces
            .get(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        let (width, height) = entry.swapchain.extent();
        if !entry.pending {
            return Err(GpuError::DeviceFailure(
                "surface has no unpresented frame".into(),
            ));
        }
        let resource = entry.swapchain.current().resource;
        core.wait_idle()?;
        let desc = unsafe { resource.GetDesc() };
        let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut total = 0u64;
        unsafe {
            core.device().GetCopyableFootprints(
                &desc,
                0,
                1,
                0,
                Some(&mut footprint),
                None,
                None,
                Some(&mut total),
            )
        };
        let readback = crate::scene::util::create_buffer(
            core.device(),
            D3D12_HEAP_TYPE_READBACK,
            total,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
        .map_err(GpuError::from)?;
        let frame = core.begin_frame()?;
        let list = &self.gpu.as_ref().ok_or(no_device())?.list;
        unsafe {
            list.Reset(&frame.allocator, None)
                .map_err(hal::Error::from)?;
            let source = texture_location(resource);
            let destination = footprint_location(&readback, footprint);
            list.CopyTextureRegion(&destination, 0, 0, 0, &source, None);
            list.Close().map_err(hal::Error::from)?;
        }
        core.execute(list)?;
        let fence = core.end_frame(&frame)?;
        core.wait_for(fence)?;
        let pointer = crate::scene::util::map_whole(&readback).map_err(GpuError::from)?;
        let bytes = unsafe { std::slice::from_raw_parts(pointer as *const u8, total as usize) };
        let rgba8 = bgra8_rows_to_rgba8(
            bytes,
            footprint.Footprint.RowPitch as usize,
            width as usize,
            height as usize,
        );
        unsafe { readback.Unmap(0, None) };
        Ok(SurfacePixels {
            extent: Extent { width, height },
            rgba8,
        })
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
