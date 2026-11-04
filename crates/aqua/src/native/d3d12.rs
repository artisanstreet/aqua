use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Once};

use aqua_core::{Oklch, Rect};
use aqua_gpu_d3d12::window::{TextureRecovery, WindowBackend, WindowOptions};
use aqua_graph::{Extent, FramePlan};
use aqua_protocol::{
    Feedback, FrameId, FrameInputs, FrameReport, GpuBackend, GpuCapabilities, GpuError,
    NativeSurface, PresentHint, PresentTimes, RecoveryReport, SurfaceId, SurfaceTarget,
    UploadBatch, WindowId,
};
use aqua_runtime::RenderSide;
use aqua_text::{SharedFonts, SharedRuns};

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
}

thread_local! {
    static HUB: RefCell<Weak<Hub>> = const { RefCell::new(Weak::new()) };
}

fn shared_hub(fonts: &SharedFonts) -> Result<Rc<Hub>, String> {
    if let Some(hub) = HUB.with(|slot| slot.borrow().upgrade()) {
        return Ok(hub);
    }
    TEXT_SERVICES.call_once(|| {
        persist::start_text_services(fonts);
    });
    let cache = persist::load_pipeline_cache();
    let mut backend =
        WindowBackend::with_pipeline_cache(WindowOptions::from_env(), cache.as_deref())
            .map_err(|e| e.to_string())?;
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
        })
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

    fn pipeline_cache(&self) -> Option<Vec<u8>> {
        self.hub.backend.borrow().pipeline_cache()
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
    }
}

impl GpuBackend for WindowHandle {
    type Error = GpuError;

    fn capabilities(&self) -> &GpuCapabilities {
        &self.capabilities
    }

    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
        self.hub.backend.borrow_mut().create_surface(target)
    }

    fn upload(&mut self, mut batch: UploadBatch) -> Result<(), GpuError> {
        self.adopt_recovery()?;
        namespace_uploads(self.namespace, &mut batch);
        self.hub.backend.borrow_mut().upload(batch)
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
        match damage {
            Some(rects) => {
                let report = backend.execute_damaged(plan, inputs, &rects)?;
                self.presenting = Some(rects);
                Ok(report)
            }
            None => {
                self.presenting = None;
                backend.execute(plan, inputs)
            }
        }
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
        let feedback = self.frames.present(side, frame);
        self.stats
            .device_sharers
            .store(self.frames.backend().sharers() as u64, Ordering::Relaxed);
        self.deliver_composition();
        feedback
    }

    fn invalidate(&mut self) {
        self.frames.invalidate();
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
                .with_maintenance(move |backend: &mut WindowHandle| {
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
            };
            target.deliver_composition();
            Ok(Box::new(target) as Box<dyn FrameTarget>)
        })
    })
}
