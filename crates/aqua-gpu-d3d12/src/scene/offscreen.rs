use std::collections::HashMap;
use std::time::Instant;

use aqua_graph::{Extent, FramePlan, validate};
use aqua_protocol::{
    DeviceStatus, FeatureTier, Features, FrameInputs, FrameReport, GpuBackend, GpuCapabilities,
    GpuError, GpuReadback, PresentHint, PresentTimes, RecoveryReport, SurfaceId, SurfacePixels,
    SurfaceTarget, UploadBatch,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::Interface;

use super::Error;
use super::device::{AdapterPreference, Selected, select};
use super::persistent::PersistentTextures;
use super::renderer::{SceneRenderer, SceneTarget};
use super::util::*;
use crate::half::{bgra8_rows_to_rgba8, bgra8_srgb_rows_to_linear_rgba8, rgba16f_rows_to_rgba8};

const SLOTS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceFormat {
    Rgba16Float,
    Bgra8Unorm,
    Bgra8UnormSrgb,
}

impl SurfaceFormat {
    fn resource(self) -> DXGI_FORMAT {
        match self {
            SurfaceFormat::Rgba16Float => DXGI_FORMAT_R16G16B16A16_FLOAT,
            SurfaceFormat::Bgra8Unorm | SurfaceFormat::Bgra8UnormSrgb => {
                DXGI_FORMAT_B8G8R8A8_TYPELESS
            }
        }
    }

    fn view(self) -> DXGI_FORMAT {
        match self {
            SurfaceFormat::Rgba16Float => DXGI_FORMAT_R16G16B16A16_FLOAT,
            SurfaceFormat::Bgra8Unorm => DXGI_FORMAT_B8G8R8A8_UNORM,
            SurfaceFormat::Bgra8UnormSrgb => DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
        }
    }

    fn convert(self, bytes: &[u8], pitch: usize, width: usize, height: usize) -> Vec<u8> {
        match self {
            SurfaceFormat::Rgba16Float => rgba16f_rows_to_rgba8(bytes, pitch, width, height),
            SurfaceFormat::Bgra8Unorm => bgra8_rows_to_rgba8(bytes, pitch, width, height),
            SurfaceFormat::Bgra8UnormSrgb => {
                bgra8_srgb_rows_to_linear_rgba8(bytes, pitch, width, height)
            }
        }
    }
}

struct SurfaceResources {
    extent: Extent,
    resource: ID3D12Resource,
    rtv_heap: ID3D12DescriptorHeap,
    executed: bool,
}

struct Gpu {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    list: ID3D12GraphicsCommandList,
    allocators: Vec<ID3D12CommandAllocator>,
    slot_fences: Vec<u64>,
    fence: ID3D12Fence,
    event: HANDLE,
    fence_value: u64,
    renderer: SceneRenderer,
    textures: PersistentTextures,
    pending_uploads: Vec<UploadBatch>,
}

pub struct OffscreenBackend {
    preference: AdapterPreference,
    format: SurfaceFormat,
    capabilities: GpuCapabilities,
    software: bool,
    unified_memory: bool,
    gpu: Gpu,
    surfaces: HashMap<SurfaceId, SurfaceResources>,
    next_surface: u32,
    next_slot: usize,
}

fn create_surface_resources(
    device: &ID3D12Device,
    extent: Extent,
    format: SurfaceFormat,
) -> Result<SurfaceResources, Error> {
    let clear = D3D12_CLEAR_VALUE {
        Format: format.view(),
        Anonymous: D3D12_CLEAR_VALUE_0 { Color: [0.0; 4] },
    };
    let mut resource: Option<ID3D12Resource> = None;
    unsafe {
        device.CreateCommittedResource(
            &heap_properties(D3D12_HEAP_TYPE_DEFAULT),
            D3D12_HEAP_FLAG_NONE,
            &texture_desc(
                extent.width,
                extent.height,
                format.resource(),
                D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            ),
            D3D12_RESOURCE_STATE_COMMON,
            Some(&clear),
            &mut resource,
        )?
    };
    let resource = resource.ok_or(Error::NoAdapter)?;
    let rtv_heap: ID3D12DescriptorHeap = unsafe {
        device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
            NumDescriptors: 1,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
            NodeMask: 0,
        })?
    };
    let view = D3D12_RENDER_TARGET_VIEW_DESC {
        Format: format.view(),
        ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
        Anonymous: D3D12_RENDER_TARGET_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_RTV {
                MipSlice: 0,
                PlaneSlice: 0,
            },
        },
    };
    unsafe {
        device.CreateRenderTargetView(
            &resource,
            Some(&view),
            rtv_heap.GetCPUDescriptorHandleForHeapStart(),
        )
    };
    Ok(SurfaceResources {
        extent,
        resource,
        rtv_heap,
        executed: false,
    })
}

impl Gpu {
    fn new(
        selected: &Selected,
        format: SurfaceFormat,
        cache: Option<&[u8]>,
    ) -> Result<Self, Error> {
        let device = selected.device.clone();
        let queue: ID3D12CommandQueue = unsafe {
            device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                Priority: 0,
                Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
                NodeMask: 0,
            })?
        };
        let allocators = (0..SLOTS)
            .map(|_| unsafe {
                device.CreateCommandAllocator::<ID3D12CommandAllocator>(
                    D3D12_COMMAND_LIST_TYPE_DIRECT,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let list: ID3D12GraphicsCommandList = unsafe {
            device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocators[0], None)?
        };
        unsafe { list.Close()? };
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
        let event = unsafe { CreateEventW(None, false, false, None)? };
        let mut renderer = SceneRenderer::with_pipeline_cache(&device, SLOTS, cache)?;
        renderer.warm(&[DXGI_FORMAT_R16G16B16A16_FLOAT, format.view()])?;
        let textures = PersistentTextures::new(&device);
        Ok(Self {
            device,
            queue,
            list,
            allocators,
            slot_fences: vec![0; SLOTS],
            fence,
            event,
            fence_value: 0,
            renderer,
            textures,
            pending_uploads: Vec::new(),
        })
    }

    fn completed(&self) -> u64 {
        unsafe { self.fence.GetCompletedValue() }
    }

    fn wait_for(&self, value: u64) -> Result<(), Error> {
        if self.completed() >= value {
            return Ok(());
        }
        unsafe {
            self.fence.SetEventOnCompletion(value, self.event)?;
            WaitForSingleObject(self.event, INFINITE);
        }
        Ok(())
    }

    fn wait_idle(&mut self) -> Result<(), Error> {
        self.fence_value += 1;
        unsafe { self.queue.Signal(&self.fence, self.fence_value)? };
        self.wait_for(self.fence_value)?;
        self.check_removed()
    }

    fn check_removed(&self) -> Result<(), Error> {
        unsafe { self.device.GetDeviceRemovedReason() }
            .map_err(|error| Error::DeviceRemoved(error.message()))
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        let _ = self.wait_idle();
        unsafe {
            let _ = CloseHandle(self.event);
        }
    }
}

impl OffscreenBackend {
    pub fn new(preference: AdapterPreference) -> Result<Self, Error> {
        Self::with_format(preference, SurfaceFormat::Rgba16Float)
    }

    pub fn with_format(
        preference: AdapterPreference,
        format: SurfaceFormat,
    ) -> Result<Self, Error> {
        Self::with_pipeline_cache(preference, format, None)
    }

    pub fn with_pipeline_cache(
        preference: AdapterPreference,
        format: SurfaceFormat,
        cache: Option<&[u8]>,
    ) -> Result<Self, Error> {
        let selected = select(preference)?;
        let gpu = Gpu::new(&selected, format, cache)?;
        let capabilities = GpuCapabilities {
            backend: "d3d12".into(),
            adapter: selected.name.clone(),
            tier: FeatureTier::Baseline,
            max_texture_extent: D3D12_REQ_TEXTURE2D_U_OR_V_DIMENSION,
            heap_alignment: D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT as u64,
            features: Features {
                compute: false,
                custom_passes: false,
                enhanced_barriers: false,
                tearing: false,
                software: selected.software,
                window_surfaces: false,
            },
        };
        Ok(Self {
            preference,
            format,
            capabilities,
            software: selected.software,
            unified_memory: selected.unified_memory,
            gpu,
            surfaces: HashMap::new(),
            next_surface: 0,
            next_slot: 0,
        })
    }

    pub fn is_software(&self) -> bool {
        self.software
    }

    pub fn is_unified_memory(&self) -> bool {
        self.unified_memory
    }

    pub fn pipeline_stats(&self) -> super::PipelineStats {
        self.gpu.renderer.pipeline_stats()
    }

    pub fn pipeline_cache(&self) -> Option<Vec<u8>> {
        self.gpu.renderer.pipeline_cache()
    }

    pub fn persistent_stats(&self) -> super::PersistentStats {
        self.gpu.textures.stats()
    }

    fn run_frame(&mut self, plan: &FramePlan, surface: SurfaceId) -> Result<FrameReport, GpuError> {
        let started = Instant::now();
        validate(plan)?;
        let entry = self
            .surfaces
            .get_mut(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        if entry.extent != plan.extent {
            return Err(GpuError::ExtentMismatch {
                plan: plan.extent,
                surface: entry.extent,
            });
        }
        self.gpu.check_removed().map_err(GpuError::from)?;
        let self_format = self.format.view();
        let slot = self.next_slot;
        self.next_slot = (self.next_slot + 1) % SLOTS;
        self.gpu
            .wait_for(self.gpu.slot_fences[slot])
            .map_err(GpuError::from)?;
        let completed = self.gpu.completed();
        self.gpu.renderer.retire(completed);
        self.gpu.textures.retire(completed);
        let frame_fence = self.gpu.fence_value + 1;
        let gpu = &mut self.gpu;
        let target = SceneTarget {
            resource: &entry.resource,
            rtv: unsafe { entry.rtv_heap.GetCPUDescriptorHandleForHeapStart() },
            format: self_format,
            rest_state: D3D12_RESOURCE_STATE_COMMON,
        };
        let stats = (|| -> Result<_, Error> {
            unsafe {
                gpu.allocators[slot].Reset()?;
                gpu.list.Reset(&gpu.allocators[slot], None)?;
            }
            gpu.textures
                .record(&gpu.list, &gpu.pending_uploads, frame_fence, completed)?;
            gpu.pending_uploads.clear();
            let textures = &gpu.textures;
            let stats =
                gpu.renderer
                    .record(&gpu.list, slot, frame_fence, plan, &target, &|handle| {
                        textures.view(handle)
                    })?;
            unsafe {
                gpu.list.Close()?;
                gpu.queue.ExecuteCommandLists(&[Some(gpu.list.cast()?)]);
                gpu.fence_value = frame_fence;
                gpu.queue.Signal(&gpu.fence, frame_fence)?;
            }
            gpu.slot_fences[slot] = frame_fence;
            Ok(stats)
        })()
        .map_err(GpuError::from)?;
        entry.executed = true;
        Ok(FrameReport {
            passes: stats.passes,
            instances: stats.instances,
            cpu_time: started.elapsed(),
            device: DeviceStatus::Healthy,
            barriers: stats.barriers,
            transient_bytes: gpu.renderer.transient_heap_bytes(),
            gpu_time: None,
        })
    }
}

impl GpuBackend for OffscreenBackend {
    type Error = GpuError;

    fn capabilities(&self) -> &GpuCapabilities {
        &self.capabilities
    }

    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
        match target {
            SurfaceTarget::Offscreen(extent) => {
                let resources = create_surface_resources(&self.gpu.device, extent, self.format)?;
                let id = SurfaceId(self.next_surface);
                self.next_surface += 1;
                self.surfaces.insert(id, resources);
                Ok(id)
            }
            SurfaceTarget::Window(_) | SurfaceTarget::Native(_) => Err(GpuError::Unsupported(
                "window surfaces on the offscreen backend",
            )),
        }
    }

    fn upload(&mut self, batch: UploadBatch) -> Result<(), GpuError> {
        if !batch.textures.is_empty() {
            self.gpu.pending_uploads.push(batch);
        }
        Ok(())
    }

    fn execute(&mut self, plan: &FramePlan, inputs: &FrameInputs) -> Result<FrameReport, GpuError> {
        self.run_frame(plan, inputs.surface)
    }

    fn present(
        &mut self,
        surface: SurfaceId,
        _hint: PresentHint,
    ) -> Result<PresentTimes, GpuError> {
        match self.surfaces.contains_key(&surface) {
            true => Ok(PresentTimes::default()),
            false => Err(GpuError::UnknownSurface(surface)),
        }
    }

    fn recover(&mut self) -> Result<RecoveryReport, GpuError> {
        let extents: Vec<(SurfaceId, Extent)> = self
            .surfaces
            .iter()
            .map(|(id, entry)| (*id, entry.extent))
            .collect();
        let selected = select(self.preference)?;
        self.surfaces.clear();
        self.gpu = Gpu::new(&selected, self.format, None)?;
        self.software = selected.software;
        for (id, extent) in &extents {
            self.surfaces.insert(
                *id,
                create_surface_resources(&self.gpu.device, *extent, self.format)?,
            );
        }
        Ok(RecoveryReport {
            rebuilt_surfaces: extents.len() as u32,
        })
    }
}

impl GpuReadback for OffscreenBackend {
    fn read_surface(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        self.gpu.wait_idle()?;
        let entry = self
            .surfaces
            .get(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        if !entry.executed {
            return Err(GpuError::DeviceFailure(
                "surface has no executed frame".into(),
            ));
        }
        let extent = entry.extent;
        let gpu = &self.gpu;
        let desc = texture_desc(
            extent.width,
            extent.height,
            self.format.resource(),
            D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        );
        let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut total = 0u64;
        unsafe {
            gpu.device.GetCopyableFootprints(
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
        let pixels = (|| -> Result<Vec<u8>, Error> {
            let readback = create_buffer(
                &gpu.device,
                D3D12_HEAP_TYPE_READBACK,
                total,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )?;
            unsafe {
                gpu.allocators[0].Reset()?;
                gpu.list.Reset(&gpu.allocators[0], None)?;
                let source = texture_location(&entry.resource);
                let destination = footprint_location(&readback, footprint);
                gpu.list
                    .CopyTextureRegion(&destination, 0, 0, 0, &source, None);
                gpu.list.Close()?;
                gpu.queue.ExecuteCommandLists(&[Some(gpu.list.cast()?)]);
                gpu.queue.Signal(&gpu.fence, gpu.fence_value + 1)?;
            }
            gpu.wait_for(gpu.fence_value + 1)?;
            let pointer = map_whole(&readback)?;
            let bytes = unsafe { std::slice::from_raw_parts(pointer as *const u8, total as usize) };
            let converted = self.format.convert(
                bytes,
                footprint.Footprint.RowPitch as usize,
                extent.width as usize,
                extent.height as usize,
            );
            unsafe { readback.Unmap(0, None) };
            Ok(converted)
        })()?;
        self.gpu.fence_value += 1;
        Ok(SurfacePixels {
            extent,
            rgba8: pixels,
        })
    }
}
