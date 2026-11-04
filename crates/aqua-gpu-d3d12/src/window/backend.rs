use std::collections::HashMap;
use std::time::Instant;

use aqua_core::geometry::Rect;
use aqua_graph::{validate, Extent, FramePlan};
use aqua_protocol::{
    DeviceStatus, FeatureTier, Features, FrameInputs, FrameReport, GpuBackend, GpuCapabilities,
    GpuError, GpuReadback, PresentHint, PresentTimes, RecoveryReport, SurfaceId, SurfacePixels,
    SurfaceTarget, UploadBatch,
};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::damage::{Damage, DamageTracker};
use crate::hal::{
    self, Core, DeviceOptions, PresentMode, PresentStatus, Swapchain, BACK_BUFFER_FORMAT,
};
use crate::half::bgra8_rows_to_rgba8;
use crate::scene::util::{footprint_location, texture_location};
use crate::scene::{
    PersistentStats, PersistentTextures, PipelineStats, SceneRenderer, SceneTarget,
};

const FRAME_WAIT_MS: u32 = 1000;

#[derive(Clone, Copy, Debug)]
pub struct WindowOptions {
    pub device: DeviceOptions,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            device: DeviceOptions::default(),
        }
    }
}

impl WindowOptions {
    pub fn from_env() -> Self {
        Self {
            device: DeviceOptions::from_env(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryNotice {
    pub generation: u32,
    pub software: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowStats {
    pub frames: u64,
    pub presents: u64,
    pub occluded: u64,
    pub recoveries: u32,
    pub last_repaint_area: i64,
}

struct WindowSurface {
    swapchain: Swapchain,
    hwnd: u64,
    window: aqua_protocol::WindowId,
    damage: DamageTracker,
    rendered: Option<usize>,
    pending: bool,
}

struct Gpu {
    list: ID3D12GraphicsCommandList,
    renderer: SceneRenderer,
    textures: PersistentTextures,
}

pub struct WindowBackend {
    options: WindowOptions,
    capabilities: GpuCapabilities,
    surfaces: HashMap<SurfaceId, WindowSurface>,
    gpu: Option<Gpu>,
    core: Option<Core>,
    pending_uploads: Vec<UploadBatch>,
    next_surface: u32,
    lost: bool,
    stats: WindowStats,
    listener: Option<Box<dyn FnMut(RecoveryNotice)>>,
}

fn build_gpu(core: &Core, options: &WindowOptions, cache: Option<&[u8]>) -> Result<Gpu, GpuError> {
    let slots = options.device.frames_in_flight.max(2);
    let mut renderer = SceneRenderer::with_pipeline_cache(core.device(), slots, cache)?;
    renderer.warm(&[DXGI_FORMAT_R16G16B16A16_FLOAT, BACK_BUFFER_FORMAT])?;
    Ok(Gpu {
        list: core.create_command_list()?,
        renderer,
        textures: PersistentTextures::new(core.device()),
    })
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
            compute: false,
            custom_passes: false,
            color_matrix: false,
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
    hwnd: u64,
    extent: Extent,
) -> Result<WindowSurface, GpuError> {
    let swapchain = core.create_swapchain(hwnd as isize, extent.width, extent.height)?;
    Ok(WindowSurface {
        swapchain,
        hwnd,
        window,
        damage: DamageTracker::new(2),
        rendered: None,
        pending: false,
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
        let gpu = build_gpu(&core, &options, cache)?;
        Ok(Self {
            options,
            capabilities: describe(&core),
            surfaces: HashMap::new(),
            gpu: Some(gpu),
            core: Some(core),
            pending_uploads: Vec::new(),
            next_surface: 0,
            lost: false,
            stats: WindowStats::default(),
            listener: None,
        })
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

    fn execute_inner(
        &mut self,
        plan: &FramePlan,
        surface: SurfaceId,
    ) -> Result<FrameReport, GpuError> {
        let started = Instant::now();
        validate(plan)?;
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
        let frame = core.begin_frame()?;
        let completed = core.completed_fence();
        gpu.renderer.retire(completed);
        gpu.textures.retire(completed);
        let frame_fence = core.timeline().next_value();
        let back = entry.swapchain.current();
        let target = SceneTarget {
            resource: back.resource,
            rtv: back.rtv,
            format: BACK_BUFFER_FORMAT,
            rest_state: D3D12_RESOURCE_STATE_PRESENT,
        };
        unsafe { gpu.list.Reset(&frame.allocator, None) }.map_err(hal::Error::from)?;
        gpu.textures
            .record(&gpu.list, &self.pending_uploads, frame_fence, completed)?;
        self.pending_uploads.clear();
        let textures = &gpu.textures;
        let stats = gpu.renderer.record(
            &gpu.list,
            frame.index,
            frame_fence,
            plan,
            &target,
            &|handle| textures.view(handle),
        )?;
        unsafe { gpu.list.Close() }.map_err(hal::Error::from)?;
        core.execute(&gpu.list)?;
        let signalled = core.end_frame(&frame)?;
        debug_assert_eq!(signalled, frame_fence);
        entry.rendered = Some(back.index);
        entry.pending = true;
        self.stats.frames += 1;
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
        let status = entry.swapchain.present(mode, &dirty_rects(&damage))?;
        entry.pending = false;
        match status {
            PresentStatus::Presented => {
                entry.damage.commit(buffer, damage);
                self.stats.presents += 1;
            }
            PresentStatus::Occluded => {
                entry.damage.reset();
                self.stats.occluded += 1;
            }
        }
        Ok(PresentTimes {
            queued: started.elapsed(),
        })
    }

    fn recover_inner(&mut self) -> Result<RecoveryReport, GpuError> {
        let cache = self.pipeline_cache();
        let layout: Vec<(SurfaceId, aqua_protocol::WindowId, u64, Extent)> = self
            .surfaces
            .iter()
            .map(|(id, entry)| {
                let (width, height) = entry.swapchain.extent();
                (*id, entry.window, entry.hwnd, Extent { width, height })
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
        let gpu = build_gpu(&core, &self.options, cache.as_deref())
            .or_else(|_| build_gpu(&core, &self.options, None))?;
        for (id, window, hwnd, extent) in &layout {
            self.surfaces
                .insert(*id, new_surface(&core, *window, *hwnd, *extent)?);
        }
        self.capabilities = describe(&core);
        let notice = RecoveryNotice {
            generation,
            software: self.capabilities.features.software,
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
                let surface = new_surface(core, native.window, native.hwnd, native.extent);
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
            self.pending_uploads.push(batch);
        }
        Ok(())
    }

    fn execute(&mut self, plan: &FramePlan, inputs: &FrameInputs) -> Result<FrameReport, GpuError> {
        let outcome = self.execute_inner(plan, inputs.surface);
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
        let outcome = core.resize_swapchain(&mut entry.swapchain, extent.width, extent.height);
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
