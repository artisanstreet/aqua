use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, RECT, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::D3D11_SRV_DIMENSION_TEXTURE2D;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::WaitForSingleObjectEx;
use windows::core::Interface;

use super::error::Error;
use crate::ledger::{Kind, Token, record};

pub const RENDER_FORMAT: DXGI_FORMAT = DXGI_FORMAT_B8G8R8A8_UNORM;
const BUFFER_COUNT: u32 = 2;
const OCCLUDED: i32 = 0x087A0001;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentMode {
    Vsync,
    Immediate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentStatus {
    Presented,
    Occluded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapchainTarget {
    Hwnd(isize),
    Composition,
}

struct Buffer {
    texture: ID3D11Texture2D,
    rtv: ID3D11RenderTargetView,
    srv: Option<ID3D11ShaderResourceView>,
    _tokens: Vec<Token>,
}

pub struct Swapchain {
    swapchain: IDXGISwapChain2,
    buffer: Option<Buffer>,
    waitable: HANDLE,
    target: SwapchainTarget,
    tearing: bool,
    width: u32,
    height: u32,
    flips: u64,
    shader_views: bool,
}

fn flags(tearing: bool) -> u32 {
    let base = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32;
    match tearing {
        true => base | DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32,
        false => base,
    }
}

impl Swapchain {
    pub fn new(
        device: &ID3D11Device,
        factory: &IDXGIFactory6,
        target: SwapchainTarget,
        width: u32,
        height: u32,
        tearing: bool,
    ) -> Result<Self, Error> {
        let (width, height) = (width.max(1), height.max(1));
        let tearing = tearing && matches!(target, SwapchainTarget::Hwnd(_));
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: RENDER_FORMAT,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE(DXGI_USAGE_RENDER_TARGET_OUTPUT.0 | DXGI_USAGE_SHADER_INPUT.0),
            BufferCount: BUFFER_COUNT,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: match target {
                SwapchainTarget::Hwnd(_) => DXGI_ALPHA_MODE_IGNORE,
                SwapchainTarget::Composition => DXGI_ALPHA_MODE_PREMULTIPLIED,
            },
            Flags: flags(tearing),
        };
        let swapchain1 = match target {
            SwapchainTarget::Hwnd(hwnd) => {
                let handle = HWND(hwnd as *mut _);
                let created = unsafe {
                    factory.CreateSwapChainForHwnd(
                        device,
                        handle,
                        &desc,
                        None,
                        None::<&IDXGIOutput>,
                    )?
                };
                unsafe { factory.MakeWindowAssociation(handle, DXGI_MWA_NO_ALT_ENTER)? };
                created
            }
            SwapchainTarget::Composition => unsafe {
                factory.CreateSwapChainForComposition(device, &desc, None::<&IDXGIOutput>)?
            },
        };
        let swapchain: IDXGISwapChain2 = swapchain1.cast()?;
        unsafe { swapchain.SetMaximumFrameLatency(1)? };
        let waitable = unsafe { swapchain.GetFrameLatencyWaitableObject() };
        let mut created = Self {
            swapchain,
            buffer: None,
            waitable,
            target,
            tearing,
            width,
            height,
            flips: 0,
            shader_views: std::env::var("AQUA_D3D11_SURFACE_MIRROR").as_deref() != Ok("1"),
        };
        created.acquire(device)?;
        Ok(created)
    }

    fn acquire(&mut self, device: &ID3D11Device) -> Result<(), Error> {
        let label = match self.target {
            SwapchainTarget::Hwnd(_) => "swapchain_buffer",
            SwapchainTarget::Composition => "composition_swapchain_buffer",
        };
        let bytes = self.width as u64 * self.height as u64 * 4;
        let texture: ID3D11Texture2D = unsafe { self.swapchain.GetBuffer(0)? };
        let view = D3D11_RENDER_TARGET_VIEW_DESC {
            Format: RENDER_FORMAT,
            ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 },
            },
        };
        let mut rtv: Option<ID3D11RenderTargetView> = None;
        unsafe { device.CreateRenderTargetView(&texture, Some(&view), Some(&mut rtv))? };
        let shader_view = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: RENDER_FORMAT,
            ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                },
            },
        };
        let mut srv: Option<ID3D11ShaderResourceView> = None;
        let srv = match self.shader_views {
            true => unsafe {
                device.CreateShaderResourceView(&texture, Some(&shader_view), Some(&mut srv))
            }
            .ok()
            .and(srv),
            false => None,
        };
        self.buffer = Some(Buffer {
            texture,
            srv,
            rtv: rtv.ok_or(Error::NoAdapter)?,
            _tokens: (0..BUFFER_COUNT)
                .map(|_| record(label, Kind::Swapchain, bytes))
                .collect(),
        });
        Ok(())
    }

    pub fn content(&self) -> Result<windows::core::IUnknown, Error> {
        Ok(self.swapchain.cast()?)
    }

    pub fn target(&self) -> SwapchainTarget {
        self.target
    }

    pub fn buffer_count(&self) -> u32 {
        BUFFER_COUNT
    }

    pub fn extent(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn tearing(&self) -> bool {
        self.tearing
    }

    pub fn current_index(&self) -> usize {
        (self.flips % BUFFER_COUNT as u64) as usize
    }

    pub fn texture(&self) -> Option<&ID3D11Texture2D> {
        self.buffer.as_ref().map(|buffer| &buffer.texture)
    }

    pub fn rtv(&self) -> Option<&ID3D11RenderTargetView> {
        self.buffer.as_ref().map(|buffer| &buffer.rtv)
    }

    pub fn srv(&self) -> Option<&ID3D11ShaderResourceView> {
        self.buffer.as_ref().and_then(|buffer| buffer.srv.as_ref())
    }

    pub fn release_buffers(&mut self) {
        self.buffer = None;
    }

    pub fn resize(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext1,
        width: u32,
        height: u32,
    ) -> Result<bool, Error> {
        let (width, height) = (width.max(1), height.max(1));
        if width == self.width && height == self.height {
            return Ok(false);
        }
        self.buffer = None;
        unsafe {
            context.ClearState();
            context.Flush();
            self.swapchain.ResizeBuffers(
                BUFFER_COUNT,
                width,
                height,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(flags(self.tearing) as i32),
            )?;
        }
        self.width = width;
        self.height = height;
        self.flips = 0;
        self.acquire(device)?;
        Ok(true)
    }

    pub fn wait_for_frame(&self, timeout_ms: u32) -> bool {
        unsafe { WaitForSingleObjectEx(self.waitable, timeout_ms, true) == WAIT_OBJECT_0 }
    }

    pub fn present(&mut self, mode: PresentMode, dirty: &[RECT]) -> Result<PresentStatus, Error> {
        let (interval, flags) = match (mode, self.tearing) {
            (PresentMode::Immediate, true) => (0, DXGI_PRESENT_ALLOW_TEARING),
            (PresentMode::Immediate, false) => (0, DXGI_PRESENT(0)),
            (PresentMode::Vsync, _) => (1, DXGI_PRESENT(0)),
        };
        let parameters = DXGI_PRESENT_PARAMETERS {
            DirtyRectsCount: dirty.len() as u32,
            pDirtyRects: match dirty.is_empty() {
                true => std::ptr::null_mut(),
                false => dirty.as_ptr() as *mut RECT,
            },
            pScrollRect: std::ptr::null_mut(),
            pScrollOffset: std::ptr::null_mut(),
        };
        let code = unsafe { self.swapchain.Present1(interval, flags, &parameters) };
        match code.0 {
            OCCLUDED => Ok(PresentStatus::Occluded),
            _ => {
                code.ok()?;
                self.flips += 1;
                Ok(PresentStatus::Presented)
            }
        }
    }
}

impl Drop for Swapchain {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.waitable);
        }
    }
}
