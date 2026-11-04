use windows::core::Interface;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, RECT, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::WaitForSingleObjectEx;

use super::error::Error;

pub const BACK_BUFFER_FORMAT: DXGI_FORMAT = DXGI_FORMAT_B8G8R8A8_UNORM;
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
pub struct FrameStatistics {
    pub present_count: u32,
    pub sync_refresh_count: u32,
}

pub struct BackBuffer<'a> {
    pub index: usize,
    pub resource: &'a ID3D12Resource,
    pub rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
}

pub struct Swapchain {
    swapchain: IDXGISwapChain3,
    buffers: Vec<ID3D12Resource>,
    rtv_heap: ID3D12DescriptorHeap,
    rtv_stride: usize,
    waitable: HANDLE,
    hwnd: isize,
    tearing: bool,
    width: u32,
    height: u32,
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
        device: &ID3D12Device,
        factory: &IDXGIFactory6,
        queue: &ID3D12CommandQueue,
        hwnd: isize,
        width: u32,
        height: u32,
        tearing: bool,
    ) -> Result<Self, Error> {
        let (width, height) = (width.max(1), height.max(1));
        let handle = HWND(hwnd as *mut _);
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: BACK_BUFFER_FORMAT,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: BUFFER_COUNT,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: flags(tearing),
        };
        let swapchain1 = unsafe {
            factory.CreateSwapChainForHwnd(queue, handle, &desc, None, None::<&IDXGIOutput>)?
        };
        unsafe { factory.MakeWindowAssociation(handle, DXGI_MWA_NO_ALT_ENTER)? };
        let swapchain: IDXGISwapChain3 = swapchain1.cast()?;
        unsafe { swapchain.SetMaximumFrameLatency(1)? };
        let waitable = unsafe { swapchain.GetFrameLatencyWaitableObject() };
        let rtv_heap: ID3D12DescriptorHeap = unsafe {
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                NumDescriptors: BUFFER_COUNT,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                NodeMask: 0,
            })?
        };
        let mut created = Self {
            swapchain,
            buffers: Vec::new(),
            rtv_stride: unsafe {
                device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV)
            } as usize,
            rtv_heap,
            waitable,
            hwnd,
            tearing,
            width,
            height,
        };
        created.acquire(device)?;
        Ok(created)
    }

    fn rtv_start(&self) -> usize {
        unsafe { self.rtv_heap.GetCPUDescriptorHandleForHeapStart() }.ptr
    }

    fn acquire(&mut self, device: &ID3D12Device) -> Result<(), Error> {
        self.buffers.clear();
        for index in 0..BUFFER_COUNT {
            let buffer: ID3D12Resource = unsafe { self.swapchain.GetBuffer(index)? };
            let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: self.rtv_start() + index as usize * self.rtv_stride,
            };
            unsafe { device.CreateRenderTargetView(&buffer, None, handle) };
            self.buffers.push(buffer);
        }
        Ok(())
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    pub fn extent(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn tearing(&self) -> bool {
        self.tearing
    }

    pub fn resize(
        &mut self,
        device: &ID3D12Device,
        width: u32,
        height: u32,
    ) -> Result<bool, Error> {
        let (width, height) = (width.max(1), height.max(1));
        match width == self.width && height == self.height {
            true => Ok(false),
            false => {
                self.buffers.clear();
                unsafe {
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
                self.acquire(device)?;
                Ok(true)
            }
        }
    }

    pub fn current(&self) -> BackBuffer<'_> {
        let index = unsafe { self.swapchain.GetCurrentBackBufferIndex() } as usize;
        BackBuffer {
            index,
            resource: &self.buffers[index],
            rtv: D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: self.rtv_start() + index * self.rtv_stride,
            },
        }
    }

    pub fn wait_for_frame(&self, timeout_ms: u32) -> Result<bool, Error> {
        match unsafe { WaitForSingleObjectEx(self.waitable, timeout_ms, true) } {
            status if status == WAIT_OBJECT_0 => Ok(true),
            _ => Ok(false),
        }
    }

    pub fn present(&self, mode: PresentMode, dirty: &[RECT]) -> Result<PresentStatus, Error> {
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
                Ok(PresentStatus::Presented)
            }
        }
    }

    pub fn statistics(&self) -> Result<FrameStatistics, Error> {
        let mut stats = DXGI_FRAME_STATISTICS::default();
        unsafe { self.swapchain.GetFrameStatistics(&mut stats)? };
        Ok(FrameStatistics {
            present_count: stats.PresentCount,
            sync_refresh_count: stats.SyncRefreshCount,
        })
    }

    pub fn last_present_count(&self) -> Result<u32, Error> {
        Ok(unsafe { self.swapchain.GetLastPresentCount()? })
    }
}

impl Drop for Swapchain {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.waitable);
        }
    }
}
