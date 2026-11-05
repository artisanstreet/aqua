use std::collections::HashMap;
use std::time::Instant;

use aqua_graph::{Extent, FramePlan};
use aqua_protocol::{
    DeviceStatus, FeatureTier, Features, FrameInputs, FrameReport, GpuBackend, GpuCapabilities,
    GpuError, GpuReadback, PresentHint, PresentTimes, RecoveryReport, SurfaceId, SurfacePixels,
    SurfaceTarget, UploadBatch,
};
use windows::Win32::Graphics::Direct3D::D3D11_SRV_DIMENSION_TEXTURE2D;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::persistent::PersistentTextures;
use super::renderer::{SceneOptions, SceneRenderer, SceneTarget};
use super::{AdapterPreference, Error};
use crate::hal::budget::Trim;
use crate::hal::{create_factory, DeviceInfo};
use crate::half::{bgra8_rows_to_rgba8, rgba16f_rows_to_rgba8};

pub const HEAP_ALIGNMENT: u64 = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceFormat {
    Rgba16Float,
    Bgra8Unorm,
}

impl SurfaceFormat {
    fn dxgi(self) -> DXGI_FORMAT {
        match self {
            SurfaceFormat::Rgba16Float => DXGI_FORMAT_R16G16B16A16_FLOAT,
            SurfaceFormat::Bgra8Unorm => DXGI_FORMAT_B8G8R8A8_UNORM,
        }
    }

    fn convert(self, bytes: &[u8], pitch: usize, width: usize, height: usize) -> Vec<u8> {
        match self {
            SurfaceFormat::Rgba16Float => rgba16f_rows_to_rgba8(bytes, pitch, width, height),
            SurfaceFormat::Bgra8Unorm => bgra8_rows_to_rgba8(bytes, pitch, width, height),
        }
    }
}

struct Surface {
    extent: Extent,
    texture: ID3D11Texture2D,
    rtv: ID3D11RenderTargetView,
    srv: ID3D11ShaderResourceView,
    executed: bool,
}

struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext1,
    info: DeviceInfo,
    renderer: SceneRenderer,
    textures: PersistentTextures,
    pending_uploads: Vec<UploadBatch>,
}

pub struct OffscreenBackend {
    preference: AdapterPreference,
    format: SurfaceFormat,
    capabilities: GpuCapabilities,
    options: SceneOptions,
    gpu: Gpu,
    surfaces: HashMap<SurfaceId, Surface>,
    next_surface: u32,
}

fn create_surface(
    device: &ID3D11Device,
    extent: Extent,
    format: SurfaceFormat,
) -> Result<Surface, Error> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: extent.width.max(1),
        Height: extent.height.max(1),
        MipLevels: 1,
        ArraySize: 1,
        Format: format.dxgi(),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
    let texture: ID3D11Texture2D = texture.ok_or(Error::NoAdapter)?;
    let rtv_desc = D3D11_RENDER_TARGET_VIEW_DESC {
        Format: format.dxgi(),
        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 },
        },
    };
    let srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
        Format: format.dxgi(),
        ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_SRV {
                MostDetailedMip: 0,
                MipLevels: 1,
            },
        },
    };
    let (mut rtv, mut srv) = (None, None);
    unsafe {
        device.CreateRenderTargetView(&texture, Some(&rtv_desc), Some(&mut rtv))?;
        device.CreateShaderResourceView(&texture, Some(&srv_desc), Some(&mut srv))?;
    }
    Ok(Surface {
        extent,
        texture,
        rtv: rtv.ok_or(Error::NoAdapter)?,
        srv: srv.ok_or(Error::NoAdapter)?,
        executed: false,
    })
}

pub fn read_texture(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext1,
    texture: &ID3D11Texture2D,
    convert: impl FnOnce(&[u8], usize) -> Vec<u8>,
) -> Result<Vec<u8>, Error> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };
    let staging_desc = D3D11_TEXTURE2D_DESC {
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
        ..desc
    };
    let mut staging = None;
    unsafe { device.CreateTexture2D(&staging_desc, None, Some(&mut staging))? };
    let staging: ID3D11Texture2D = staging.ok_or(Error::NoAdapter)?;
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        context.CopyResource(&staging, texture);
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
    }
    let pitch = mapped.RowPitch as usize;
    let bytes = unsafe {
        std::slice::from_raw_parts(mapped.pData as *const u8, pitch * desc.Height as usize)
    };
    let converted = convert(bytes, pitch);
    unsafe { context.Unmap(&staging, 0) };
    Ok(converted)
}

fn capabilities(info: &DeviceInfo) -> GpuCapabilities {
    GpuCapabilities {
        backend: "d3d11".into(),
        adapter: info.adapter.name.clone(),
        tier: FeatureTier::Baseline,
        max_texture_extent: D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION,
        heap_alignment: HEAP_ALIGNMENT,
        features: Features {
            compute: true,
            custom_passes: true,
            color_matrix: true,
            enhanced_barriers: false,
            tearing: false,
            software: info.adapter.kind == crate::hal::selection::Adapter::Software,
            window_surfaces: false,
        },
    }
}

impl Gpu {
    fn new(
        preference: AdapterPreference,
        cache: Option<&[u8]>,
        options: SceneOptions,
    ) -> Result<Self, Error> {
        let debug = super::debug::requested();
        let factory = create_factory(debug)?;
        let creation = crate::hal::Creation {
            debug,
            single_threaded: false,
        };
        let selected = crate::hal::select_adapter(&factory, preference, creation)?;
        let mut renderer = SceneRenderer::new(&selected.device, cache, options)?;
        renderer.warm()?;
        Ok(Self {
            textures: PersistentTextures::new(&selected.device),
            device: selected.device,
            context: selected.context,
            info: selected.info,
            renderer,
            pending_uploads: Vec::new(),
        })
    }

    fn check_removed(&self) -> Result<(), Error> {
        unsafe { self.device.GetDeviceRemovedReason() }
            .map_err(|error| Error::DeviceRemoved(error.message()))
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            self.context.ClearState();
            self.context.Flush();
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
        Self::with_options(preference, format, cache, SceneOptions::default())
    }

    pub fn with_options(
        preference: AdapterPreference,
        format: SurfaceFormat,
        cache: Option<&[u8]>,
        options: SceneOptions,
    ) -> Result<Self, Error> {
        let gpu = Gpu::new(preference, cache, options)?;
        Ok(Self {
            preference,
            format,
            capabilities: capabilities(&gpu.info),
            options,
            gpu,
            surfaces: HashMap::new(),
            next_surface: 0,
        })
    }

    pub fn gpu_time(&self) -> Option<std::time::Duration> {
        self.gpu.renderer.gpu_time()
    }

    pub fn transient_bytes(&self) -> u64 {
        self.gpu.renderer.transient_bytes()
    }

    pub fn transient_textures(&self) -> usize {
        self.gpu.renderer.transient_textures()
    }

    pub fn trim(&mut self, trim: &Trim) -> Result<u64, GpuError> {
        Ok(self.gpu.renderer.trim(trim))
    }

    pub fn register_compute(
        &mut self,
        id: aqua_graph::CustomShaderId,
        hlsl: &str,
        entry: &str,
    ) -> Result<(), GpuError> {
        self.gpu
            .renderer
            .register_compute(id, hlsl, entry)
            .map_err(GpuError::from)
    }

    pub fn debug_layer(&self) -> bool {
        super::debug::active(&self.gpu.device)
    }

    pub fn debug_errors(&self) -> Vec<String> {
        super::debug::drain(&self.gpu.device)
    }

    pub fn is_software(&self) -> bool {
        self.capabilities.features.software
    }

    pub fn is_unified_memory(&self) -> bool {
        self.gpu.info.unified_memory
    }

    pub fn device_info(&self) -> &DeviceInfo {
        &self.gpu.info
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
        self.gpu.renderer.validate(plan)?;
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
        self.gpu.check_removed()?;
        let gpu = &mut self.gpu;
        gpu.textures.apply(&gpu.context, &gpu.pending_uploads)?;
        gpu.pending_uploads.clear();
        let target = SceneTarget {
            texture: &entry.texture,
            rtv: &entry.rtv,
            srv: Some(&entry.srv),
            format: self.format.dxgi(),
        };
        let stats = gpu
            .renderer
            .record(&gpu.context, plan, &target, &gpu.textures, None)?;
        entry.executed = true;
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
}

impl GpuBackend for OffscreenBackend {
    type Error = GpuError;

    fn capabilities(&self) -> &GpuCapabilities {
        &self.capabilities
    }

    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
        match target {
            SurfaceTarget::Offscreen(extent) => {
                let surface = create_surface(&self.gpu.device, extent, self.format)?;
                let id = SurfaceId(self.next_surface);
                self.next_surface += 1;
                self.surfaces.insert(id, surface);
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
        self.surfaces.clear();
        self.gpu = Gpu::new(self.preference, None, self.options)?;
        self.capabilities = capabilities(&self.gpu.info);
        for (id, extent) in &extents {
            self.surfaces
                .insert(*id, create_surface(&self.gpu.device, *extent, self.format)?);
        }
        Ok(RecoveryReport {
            rebuilt_surfaces: extents.len() as u32,
        })
    }
}

impl GpuReadback for OffscreenBackend {
    fn read_surface(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
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
        let format = self.format;
        let rgba8 = read_texture(
            &self.gpu.device,
            &self.gpu.context,
            &entry.texture,
            |bytes, pitch| {
                format.convert(bytes, pitch, extent.width as usize, extent.height as usize)
            },
        )?;
        Ok(SurfacePixels { extent, rgba8 })
    }
}
