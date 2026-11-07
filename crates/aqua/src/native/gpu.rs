use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Once};
use std::time::Duration;
use std::time::Instant as WallInstant;

use aqua_core::{Oklch, Rect};
#[cfg(not(feature = "d3d12"))]
use aqua_gpu_d3d11 as gpu;
#[cfg(feature = "d3d12")]
use aqua_gpu_d3d12 as gpu;
use aqua_graph::{Extent, FramePlan};
use aqua_protocol::{
    Feedback, FrameId, FrameInputs, FrameReport, GpuBackend, GpuCapabilities, GpuError,
    NativeSurface, PresentHint, PresentTimes, RecoveryReport, SurfaceId, SurfaceTarget,
    UploadBatch, WindowId,
};
use aqua_runtime::RenderSide;
use aqua_text::{SharedFonts, SharedRuns};
use gpu::hal::budget::Visibility;
use gpu::window::{TextureRecovery, WindowBackend, WindowOptions};

use crate::render::host::{DamageSlot, FrameTarget, GpuFrames, RenderStats, Repaint};
use crate::render::link::{CompositionSink, InitFactory, RenderContext};
use crate::render::lower::{Lowering, RecoverySignal};
use crate::render::namespace::{namespace_plan, namespace_uploads, Namespaces};
use crate::render::persist::{self, PipelineCacheSaver};

static TEXT_SERVICES: Once = Once::new();

type Signals = Rc<RefCell<Vec<(u64, RecoverySignal)>>>;

struct Hub {
    backend: RefCell<WindowBackend>,
    signals: Signals,
    namespaces: RefCell<Namespaces>,
    keys: Cell<u64>,
    hidden: RefCell<HashSet<u64>>,
}

thread_local! {
    static HUB: RefCell<Weak<Hub>> = const { RefCell::new(Weak::new()) };
}

fn since_process_start() -> Option<Duration> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    }
    .ok()?;
    let ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
    let since_epoch = Duration::from_nanos(ticks.checked_sub(116_444_736_000_000_000)? * 100);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .checked_sub(since_epoch)
}

static STARTUP_LINES: std::sync::Mutex<Vec<(f64, String)>> = std::sync::Mutex::new(Vec::new());

fn process_ms() -> f64 {
    since_process_start().map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1000.0)
}

fn record_startup(line: impl FnOnce(f64) -> String) {
    let at = process_ms();
    STARTUP_LINES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push((at, line(at)));
}

fn flush_startup_lines() {
    let mut lines = std::mem::take(
        &mut *STARTUP_LINES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
    #[cfg(not(feature = "d3d12"))]
    lines.extend(gpu::stages::drain());
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    lines.iter().for_each(|(_, line)| eprintln!("{line}"));
}

fn startup_marks_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("AQUA_PROFILE").is_some_and(|value| value == "1"))
}

pub fn startup_mark(label: &str) {
    if crate::mem_probe::memory_report_enabled() {
        eprintln!("aqua-stage {label} {}", crate::mem_probe::stage_summary());
    }
    if startup_marks_enabled() {
        record_startup(|at| format!("aqua-startup {label} process_ms={at:.1}"));
    }
}

fn stage_nth(label: &str, counter: &std::sync::atomic::AtomicU32, limit: u32) {
    if crate::mem_probe::memory_report_enabled() {
        let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= limit {
            startup_mark(&format!("{label}_{n}"));
        }
    }
}

fn window_options() -> WindowOptions {
    let options = WindowOptions::from_env();
    #[cfg(not(feature = "d3d12"))]
    let options = match std::env::var_os("AQUA_D3D11_PREVENT_THREADING") {
        None => WindowOptions {
            device: options.device.with_prevent_internal_threading(true),
            ..options
        },
        Some(_) => options,
    };
    options
}

pub fn prewarm() {
    startup_mark("prewarm_start");
    WindowBackend::begin_early(window_options(), persist::load_pipeline_cache);
}

fn shared_hub(fonts: &SharedFonts) -> Result<Rc<Hub>, String> {
    if let Some(hub) = HUB.with(|slot| slot.borrow().upgrade()) {
        return Ok(hub);
    }
    TEXT_SERVICES.call_once(|| {
        persist::start_text_services(fonts);
    });
    startup_mark("hub_begin");
    let device_started = std::time::Instant::now();
    let early = WindowBackend::take_early().and_then(Result::ok);
    let mut backend = match early {
        Some(backend) => backend,
        None => {
            let cache = persist::load_pipeline_cache();
            WindowBackend::with_pipeline_cache(window_options(), cache.as_deref())
                .map_err(|e| e.to_string())?
        }
    };
    if crate::profile::enabled() {
        record_startup(|at| {
            format!(
                "aqua-startup device_wait device_ms={:.1} process_ms={at:.1}",
                device_started.elapsed().as_secs_f64() * 1000.0
            )
        });
    }
    startup_mark("device_ready");
    let signals: Signals = Rc::default();
    let fan_out = signals.clone();
    backend.set_recovery_listener(Box::new(move |notice| {
        if notice.textures == TextureRecovery::HostMustReupload {
            fan_out
                .borrow()
                .iter()
                .for_each(|(_, signal)| signal.textures_lost());
        }
    }));
    let hub = Rc::new(Hub {
        backend: RefCell::new(backend),
        signals,
        namespaces: RefCell::default(),
        keys: Cell::new(0),
        hidden: RefCell::default(),
    });
    HUB.with(|slot| *slot.borrow_mut() = Rc::downgrade(&hub));
    Ok(hub)
}

pub struct WindowHandle {
    hub: Rc<Hub>,
    key: u64,
    namespace: u32,
    generation: u32,
    capabilities: GpuCapabilities,
    damage: DamageSlot,
    presenting: Option<Vec<Rect>>,
    profile_mark: Cell<(u32, u32, u64)>,
}

impl WindowHandle {
    fn join(hub: Rc<Hub>, signal: RecoverySignal, damage: DamageSlot) -> Result<Self, String> {
        let namespace = hub
            .namespaces
            .borrow_mut()
            .acquire()
            .ok_or("too many windows share this device")?;
        let key = hub.keys.get();
        hub.keys.set(key + 1);
        hub.signals.borrow_mut().push((key, signal));
        let (generation, capabilities) = {
            let backend = hub.backend.borrow();
            (backend.generation(), backend.capabilities().clone())
        };
        Ok(Self {
            hub,
            key,
            namespace,
            generation,
            capabilities,
            damage,
            presenting: None,
            profile_mark: Cell::new((0, 0, 0)),
        })
    }

    fn release_idle(&mut self) {
        let release = gpu::hal::budget::Trim {
            shrink_upload_ring: true,
            free_transients: true,
            evict_idle_resources: true,
        };
        let ledger = std::env::var_os("AQUA_LEDGER").is_some();
        if ledger {
            self.print_ledger("before-release");
        }
        if let Err(error) = self.hub.backend.borrow_mut().trim(release) {
            eprintln!("aqua: idle release failed: {error}");
        }
        self.report_memory();
        if ledger {
            self.print_ledger("after-release");
        }
    }

    fn print_ledger(&self, label: &str) {
        let backend = self.hub.backend.borrow();
        let usage = backend
            .memory_breakdown()
            .map_or(0, |memory| memory.local_usage);
        eprintln!(
            "aqua-ledger {label} dxgi_usage_mib={:.2} {}",
            usage as f64 / 1048576.0,
            backend.ledger().render()
        );
    }

    fn adopt_recovery(&mut self) -> Result<(), GpuError> {
        let backend = self.hub.backend.borrow();
        match backend.generation() == self.generation {
            true => Ok(()),
            false => {
                self.generation = backend.generation();
                self.capabilities = backend.capabilities().clone();
                self.presenting = None;
                Err(GpuError::DeviceFailure(
                    "device was recovered for another window".into(),
                ))
            }
        }
    }

    fn profile_backend(&self) {
        if !crate::profile::enabled() {
            return;
        }
        let backend = self.hub.backend.borrow();
        let pipelines = backend.pipeline_stats().unwrap_or_default();
        let resident = backend
            .persistent_stats()
            .map_or(0, |stats| stats.resident_bytes);
        let (compiled, loaded, before) = self.profile_mark.get();
        self.profile_mark
            .set((pipelines.compiled, pipelines.loaded, resident));
        use aqua_protocol::probe::{count, Metric};
        count(
            Metric::PsoCompiled,
            u64::from(pipelines.compiled.saturating_sub(compiled)),
        );
        count(
            Metric::PsoLoaded,
            u64::from(pipelines.loaded.saturating_sub(loaded)),
        );
        count(Metric::AtlasGrowthBytes, resident.saturating_sub(before));
        crate::profile::note(|| {
            let passes: Vec<String> = backend
                .gpu_pass_times()
                .iter()
                .map(|pass| format!("{}={:.3}", pass.label, pass.duration.as_secs_f64() * 1000.0))
                .collect();
            let phases = backend.phases();
            format!(
                "d3d12 gpu_ms={:.3} passes_ms=[{}] slot_wait_us={} reserve_us={} resize_us={} transients_recreated={} late_pso={:?} window={:?}",
                backend
                    .gpu_frame_time()
                    .map_or(0.0, |time| time.as_secs_f64() * 1000.0),
                passes.join(","),
                phases.slot_wait.as_micros(),
                phases.reserve.as_micros(),
                phases.resize.as_micros(),
                phases.reserve_created,
                backend.late_pipelines(),
                backend.stats()
            )
        });
    }

    fn set_half_rate(&self, surface: SurfaceId, half_rate: bool) {
        #[cfg(not(feature = "d3d12"))]
        self.hub
            .backend
            .borrow_mut()
            .set_half_rate(surface, half_rate);
        #[cfg(feature = "d3d12")]
        let _ = (surface, half_rate);
    }

    fn set_hidden(&self, hidden: bool) -> bool {
        let all_hidden = {
            let mut set = self.hub.hidden.borrow_mut();
            match hidden {
                true => set.insert(self.key),
                false => set.remove(&self.key),
            };
            !set.is_empty() && set.len() >= self.sharers()
        };
        let visibility = match all_hidden {
            true => Visibility::Hidden,
            false => Visibility::Visible,
        };
        match self.hub.backend.borrow_mut().set_visibility(visibility) {
            Ok(trim) => trim.evict_idle_resources,
            Err(error) => {
                eprintln!("aqua: visibility change failed: {error}");
                false
            }
        }
    }

    fn repaint_bounds(&self, surface: SurfaceId, damage: &[Rect]) -> Option<Rect> {
        self.hub.backend.borrow().repaint_bounds(surface, damage)
    }

    fn service_budget(&self) -> bool {
        match self.hub.backend.borrow_mut().service_budget() {
            Ok(Some(status)) => status.applied.evict_idle_resources,
            Ok(None) => false,
            Err(error) => {
                eprintln!("aqua: budget service failed: {error}");
                false
            }
        }
    }

    fn finish_startup(&self) {
        self.hub.backend.borrow_mut().finish_startup();
    }

    fn startup_pending(&self) -> bool {
        self.hub.backend.borrow().startup_pending()
    }

    fn startup_step(&self) {
        self.hub.backend.borrow_mut().startup_step();
    }

    fn pipeline_cache(&self) -> Option<Vec<u8>> {
        self.hub.backend.borrow().pipeline_cache()
    }

    fn settle_surfaces(&mut self) {
        if let Err(error) = self.hub.backend.borrow_mut().settle_surfaces() {
            eprintln!("aqua: swapchain settle failed: {error}");
        }
    }

    fn settle_surface(&mut self, surface: SurfaceId) -> bool {
        #[cfg(not(feature = "d3d12"))]
        {
            match self.hub.backend.borrow_mut().settle_surface(surface) {
                Ok(changed) => changed,
                Err(error) => {
                    eprintln!("aqua: swapchain settle failed: {error}");
                    true
                }
            }
        }
        #[cfg(feature = "d3d12")]
        {
            let _ = surface;
            false
        }
    }

    fn report_memory(&self) {
        if !crate::mem_probe::memory_report_enabled() {
            return;
        }
        if let Ok(memory) = self.hub.backend.borrow().memory_breakdown() {
            eprintln!(
                "aqua-gpu-memory: {memory:?} unattributed={} heap={:?}",
                memory.unattributed(),
                crate::mem_probe::process_heap()
            );
            eprintln!(
                "aqua-gpu-ledger: {}",
                self.hub.backend.borrow().ledger().render()
            );
        }
    }

    fn generation(&self) -> u32 {
        self.generation
    }

    fn inject_removal(&self) {
        if let Err(error) = self.hub.backend.borrow().inject_device_removal() {
            eprintln!("aqua: device removal injection failed: {error}");
        }
    }

    fn sharers(&self) -> usize {
        Rc::strong_count(&self.hub)
    }

    fn composition_content(&self, surface: SurfaceId) -> Result<windows::core::IUnknown, GpuError> {
        self.hub.backend.borrow().composition_content(surface)
    }
}

impl Drop for WindowHandle {
    fn drop(&mut self) {
        self.hub.namespaces.borrow_mut().release(self.namespace);
        self.hub
            .signals
            .borrow_mut()
            .retain(|(key, _)| *key != self.key);
        self.hub.hidden.borrow_mut().remove(&self.key);
    }
}

impl GpuBackend for WindowHandle {
    type Error = GpuError;

    fn capabilities(&self) -> &GpuCapabilities {
        &self.capabilities
    }

    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
        let created = self.hub.backend.borrow_mut().create_surface(target);
        startup_mark("surface_created");
        created
    }

    fn upload(&mut self, mut batch: UploadBatch) -> Result<(), GpuError> {
        self.adopt_recovery()?;
        namespace_uploads(self.namespace, &mut batch);
        let uploaded = self.hub.backend.borrow_mut().upload(batch);
        static UPLOADS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        stage_nth("uploaded", &UPLOADS, 6);
        uploaded
    }

    fn execute(&mut self, plan: &FramePlan, inputs: &FrameInputs) -> Result<FrameReport, GpuError> {
        self.adopt_recovery()?;
        let damage = self.damage.borrow_mut().take();
        let shifted;
        let plan = match self.namespace {
            0 => plan,
            namespace => {
                let mut copy = plan.clone();
                namespace_plan(namespace, &mut copy);
                shifted = copy;
                &shifted
            }
        };
        let mut backend = self.hub.backend.borrow_mut();
        let result = match damage {
            Some(rects) => {
                let report = backend.execute_damaged(plan, inputs, &rects)?;
                self.presenting = Some(rects);
                Ok(report)
            }
            None => {
                self.presenting = None;
                backend.execute(plan, inputs)
            }
        };
        static EXECUTES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        stage_nth("executed", &EXECUTES, 8);
        result
    }

    fn present(&mut self, surface: SurfaceId, hint: PresentHint) -> Result<PresentTimes, GpuError> {
        self.adopt_recovery()?;
        let mut backend = self.hub.backend.borrow_mut();
        match self.presenting.take() {
            Some(rects) => backend.present_with_damage(surface, hint, &rects),
            None => backend.present(surface, hint),
        }
    }

    fn recover(&mut self) -> Result<RecoveryReport, GpuError> {
        let mut backend = self.hub.backend.borrow_mut();
        self.presenting = None;
        if backend.generation() == self.generation {
            let report = backend.recover()?;
            self.generation = backend.generation();
            self.capabilities = backend.capabilities().clone();
            return Ok(report);
        }
        self.generation = backend.generation();
        self.capabilities = backend.capabilities().clone();
        Ok(RecoveryReport {
            rebuilt_surfaces: 1,
        })
    }

    fn resize_surface(&mut self, surface: SurfaceId, extent: Extent) -> Result<(), GpuError> {
        self.hub
            .backend
            .borrow_mut()
            .resize_surface(surface, extent)
    }

    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<(), GpuError> {
        self.hub.backend.borrow_mut().destroy_surface(surface)
    }

    fn wait_for_frame_slot(&mut self, surface: SurfaceId) -> Result<(), GpuError> {
        self.hub.backend.borrow_mut().wait_for_frame_slot(surface)
    }

    fn wait_for_present_slot(
        &mut self,
        surface: SurfaceId,
        hint: PresentHint,
    ) -> Result<(), GpuError> {
        self.hub
            .backend
            .borrow_mut()
            .wait_for_present_slot(surface, hint)
    }

    fn present_with_damage(
        &mut self,
        surface: SurfaceId,
        hint: PresentHint,
        damage: &[Rect],
    ) -> Result<PresentTimes, GpuError> {
        self.hub
            .backend
            .borrow_mut()
            .present_with_damage(surface, hint, damage)
    }
}

pub struct CompositionContent(pub windows::core::IUnknown);

unsafe impl Send for CompositionContent {}

struct WindowTarget {
    stats: Arc<RenderStats>,
    frames: GpuFrames<WindowHandle>,
    surface: SurfaceId,
    composition: Option<CompositionSink>,
    delivered: Option<u32>,
    announced: bool,
    viewport: Option<(aqua_core::Size, f32)>,
    settle_at: Option<WallInstant>,
}

impl WindowTarget {
    fn deliver_composition(&mut self) {
        let Some(sink) = self.composition.as_ref() else {
            return;
        };
        let generation = self.frames.backend().generation();
        if self.delivered == Some(generation) {
            return;
        }
        match self.frames.backend().composition_content(self.surface) {
            Ok(content) => {
                self.delivered = Some(generation);
                sink(Box::new(CompositionContent(content)) as Box<dyn Any + Send>);
            }
            Err(error) => eprintln!("aqua: composition content unavailable: {error}"),
        }
    }
}

impl FrameTarget for WindowTarget {
    fn paced(&self) -> bool {
        self.frames.paced()
    }

    fn wait_frame(&mut self) {
        self.frames.wait_frame();
    }

    fn present(&mut self, side: &mut RenderSide, frame: FrameId) -> Vec<Feedback> {
        if self.stats.inject_loss.swap(false, Ordering::AcqRel) {
            self.frames.backend().inject_removal();
        }
        if !self.announced {
            startup_mark("frame_present_begin");
        }
        let feedback = self.frames.present(side, frame);
        if !cfg!(feature = "d3d12")
            && self.composition.is_none()
            && self.viewport != side.viewport()
            && feedback
                .iter()
                .any(|item| matches!(item, Feedback::Presented(_)))
        {
            self.viewport = side.viewport();
            self.settle_at = Some(WallInstant::now() + Duration::from_millis(500));
        }
        if crate::mem_probe::memory_report_enabled() {
            static PRESENTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let count = PRESENTS.fetch_add(1, Ordering::Relaxed) + 1;
            if matches!(count, 1..=8 | 60 | 400) {
                startup_mark(&format!("present_{count}"));
            }
        }
        if !self.announced && crate::profile::enabled() {
            self.announced = true;
            startup_mark("first_present");
            flush_startup_lines();
        }
        self.frames.backend().finish_startup();
        self.frames.backend().profile_backend();
        self.stats
            .device_sharers
            .store(self.frames.backend().sharers() as u64, Ordering::Relaxed);
        self.deliver_composition();
        feedback
    }

    fn invalidate(&mut self) {
        self.frames.invalidate();
    }

    fn startup_pending(&self) -> bool {
        self.frames.backend().startup_pending()
    }

    fn startup_step(&mut self) {
        self.frames.backend().startup_step();
    }

    fn maintenance_in(&self) -> Option<Duration> {
        self.settle_at
            .map(|at| at.saturating_duration_since(WallInstant::now()))
    }

    fn maintenance(&mut self) -> bool {
        self.settle_at = None;
        self.frames.backend_mut().settle_surface(self.surface)
    }

    fn idle_trim_in(&mut self) -> Option<std::time::Duration> {
        self.frames.idle_trim_in()
    }

    fn visibility(&mut self, hidden: bool) {
        self.frames.visibility(hidden);
    }

    fn repaint(&self) -> Repaint {
        self.frames.repaint()
    }
}

impl Drop for WindowTarget {
    fn drop(&mut self) {
        let surface = self.surface;
        let _ = self.frames.backend_mut().destroy_surface(surface);
    }
}

pub fn factory(
    window: WindowId,
    hwnd: isize,
    extent: Extent,
    fonts: SharedFonts,
    runs: SharedRuns,
    clear: Oklch,
) -> InitFactory {
    Box::new(move |context: RenderContext| {
        let RenderContext {
            externals,
            stats,
            composition,
        } = context;
        Box::new(move || {
            let hub = shared_hub(&fonts)?;
            let signal = RecoverySignal::default();
            let damage: DamageSlot = Rc::default();
            let listener = signal.listener();
            let mut handle = WindowHandle::join(hub.clone(), listener, damage.clone())?;
            let transparent = clear.alpha < 1.0;
            let surface = match transparent {
                true => hub
                    .backend
                    .borrow_mut()
                    .create_composition_surface(window, extent),
                false => handle.create_surface(SurfaceTarget::Native(NativeSurface {
                    window,
                    hwnd: hwnd as u64,
                    extent,
                })),
            }
            .map_err(|e| e.to_string())?;
            let lowering = Lowering::new(fonts.clone(), runs.clone())
                .with_clear(clear)
                .with_recovery(signal);
            let mut saver = PipelineCacheSaver::default();
            let shared_stats = stats.clone();
            let frames = GpuFrames::new(handle, surface, extent, lowering, externals, stats)
                .with_damage(damage, runs)
                .with_repaint_probe(|backend: &WindowHandle, surface, damage: &[Rect]| {
                    backend.repaint_bounds(surface, damage)
                })
                .with_idle_release(|backend: &mut WindowHandle| backend.release_idle())
                .with_pressure(|backend: &mut WindowHandle| backend.service_budget())
                .with_visibility(|backend: &mut WindowHandle, hidden| backend.set_hidden(hidden))
                .with_half_rate(|backend: &mut WindowHandle, surface, half| {
                    backend.set_half_rate(surface, half)
                })
                .with_maintenance(move |backend: &mut WindowHandle| {
                    backend.settle_surfaces();
                    saver.save(backend.pipeline_cache());
                    backend.report_memory();
                    persist::save_fallback_index(&fonts);
                });
            let mut target = WindowTarget {
                stats: shared_stats,
                frames,
                surface,
                composition: transparent.then_some(composition),
                delivered: None,
                announced: false,
                viewport: None,
                settle_at: None,
            };
            target.deliver_composition();
            Ok(Box::new(target) as Box<dyn FrameTarget>)
        })
    })
}
