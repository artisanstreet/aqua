use std::time::{Duration, Instant};

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::*;

use super::adapter::{create_factory, enable_debug_layer, select, DeviceInfo};
use super::budget::{pressure, trim_for, Pressure, Trim, Visibility};
use super::descriptors::{CpuHeap, DynamicRange, ShaderVisibleHeap};
use super::error::Error;
use super::frames::{CoreFrame, FrameRing, UploadAlloc};
use super::monitor::{BudgetMonitor, MemorySnapshot};
use super::recovery::{classify, LossCause, LossHistory, RecoveryStep};
use super::selection::{parse_preference, Adapter, AdapterPreference};
use super::swapchain::Swapchain;
use super::tier::{Tier, Tiers};
use super::timeline::Timeline;

const RTV_CAPACITY: u32 = 64;
const STATIC_DESCRIPTORS: u32 = 64;
const DYNAMIC_DESCRIPTORS: u32 = 2_048;
const LOSS_WINDOW: Duration = Duration::from_secs(10);
const LOSS_LIMIT: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceOptions {
    pub preference: AdapterPreference,
    pub max_tier: Tier,
    pub frames_in_flight: usize,
    pub debug_layer: bool,
}

impl Default for DeviceOptions {
    fn default() -> Self {
        Self {
            preference: AdapterPreference::HighPerformance,
            max_tier: Tier::Bindless,
            frames_in_flight: 3,
            debug_layer: false,
        }
    }
}

impl DeviceOptions {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            preference: std::env::var("AQUA_ADAPTER")
                .ok()
                .and_then(|text| parse_preference(&text))
                .unwrap_or(defaults.preference),
            max_tier: match std::env::var("AQUA_TIER").as_deref() {
                Ok("baseline") => Tier::Baseline,
                _ => defaults.max_tier,
            },
            frames_in_flight: defaults.frames_in_flight,
            debug_layer: std::env::var("AQUA_D3D12_DEBUG").as_deref() == Ok("1"),
        }
    }

    pub fn with_preference(mut self, preference: AdapterPreference) -> Self {
        self.preference = preference;
        self
    }

    pub fn with_max_tier(mut self, tier: Tier) -> Self {
        self.max_tier = tier;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recovered {
    pub cause: Option<LossCause>,
    pub step: RecoveryStep,
    pub generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetStatus {
    pub memory: MemorySnapshot,
    pub pressure: Pressure,
    pub applied: Trim,
}

pub struct Core {
    options: DeviceOptions,
    factory: IDXGIFactory6,
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    info: DeviceInfo,
    timeline: Timeline,
    frames: FrameRing,
    rtv: CpuHeap,
    srv: ShaderVisibleHeap,
    monitor: BudgetMonitor,
    history: LossHistory,
    epoch: Instant,
    generation: u32,
    visibility: Visibility,
}

struct Built {
    factory: IDXGIFactory6,
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    info: DeviceInfo,
    timeline: Timeline,
    frames: FrameRing,
    rtv: CpuHeap,
    srv: ShaderVisibleHeap,
    monitor: BudgetMonitor,
}

fn build(options: &DeviceOptions) -> Result<Built, Error> {
    if options.debug_layer {
        enable_debug_layer();
    }
    let factory = create_factory(options.debug_layer)?;
    let selected = select(&factory, options.preference, options.max_tier)?;
    let device = selected.device;
    let queue: ID3D12CommandQueue = unsafe {
        device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Priority: 0,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            NodeMask: 0,
        })?
    };
    Ok(Built {
        timeline: Timeline::new(&device)?,
        frames: FrameRing::new(&device, options.frames_in_flight.max(2))?,
        rtv: CpuHeap::new(&device, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, RTV_CAPACITY)?,
        srv: ShaderVisibleHeap::new(&device, STATIC_DESCRIPTORS, DYNAMIC_DESCRIPTORS)?,
        monitor: BudgetMonitor::new(&selected.adapter)?,
        info: selected.info,
        factory,
        device,
        queue,
    })
}

impl Core {
    pub fn new(options: DeviceOptions) -> Result<Self, Error> {
        let built = build(&options)?;
        Ok(Self {
            options,
            factory: built.factory,
            device: built.device,
            queue: built.queue,
            info: built.info,
            timeline: built.timeline,
            frames: built.frames,
            rtv: built.rtv,
            srv: built.srv,
            monitor: built.monitor,
            history: LossHistory::new(LOSS_WINDOW, LOSS_LIMIT),
            epoch: Instant::now(),
            generation: 0,
            visibility: Visibility::Visible,
        })
    }

    pub fn device(&self) -> &ID3D12Device {
        &self.device
    }

    pub fn queue(&self) -> &ID3D12CommandQueue {
        &self.queue
    }

    pub fn factory(&self) -> &IDXGIFactory6 {
        &self.factory
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn tiers(&self) -> Tiers {
        self.info.tiers
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    pub fn rtv_heap(&mut self) -> &mut CpuHeap {
        &mut self.rtv
    }

    pub fn srv_heap(&mut self) -> &mut ShaderVisibleHeap {
        &mut self.srv
    }

    pub fn upload_capacity(&self) -> u64 {
        self.frames.upload.capacity()
    }

    pub fn create_command_list(&self) -> Result<ID3D12GraphicsCommandList, Error> {
        let list: ID3D12GraphicsCommandList = unsafe {
            self.device.CreateCommandList(
                0,
                D3D12_COMMAND_LIST_TYPE_DIRECT,
                self.frames.first_allocator(),
                None,
            )?
        };
        unsafe { list.Close()? };
        Ok(list)
    }

    pub fn begin_frame(&mut self) -> Result<CoreFrame, Error> {
        self.check_removed()?;
        let frame = self.frames.begin(&self.timeline)?;
        self.srv.retire(self.timeline.completed());
        Ok(frame)
    }

    pub fn allocate_descriptors(&mut self, count: u32) -> Result<DynamicRange, Error> {
        self.srv.allocate_dynamic(&self.timeline, count)
    }

    pub fn upload(&mut self, size: u64, alignment: u64) -> Result<UploadAlloc, Error> {
        self.frames.upload.allocate(&self.timeline, size, alignment)
    }

    pub fn end_frame(&mut self, frame: &CoreFrame) -> Result<u64, Error> {
        let fence = self.timeline.signal(&self.queue)?;
        self.frames.end(frame.index, fence);
        self.srv.end_frame(fence);
        Ok(fence)
    }

    pub fn execute(&self, list: &ID3D12GraphicsCommandList) -> Result<(), Error> {
        unsafe { self.queue.ExecuteCommandLists(&[Some(list.cast()?)]) };
        Ok(())
    }

    pub fn completed_fence(&self) -> u64 {
        self.timeline.completed()
    }

    pub fn wait_for(&self, fence: u64) -> Result<(), Error> {
        self.timeline.wait(fence).and_then(|_| self.check_removed())
    }

    pub fn wait_idle(&mut self) -> Result<(), Error> {
        self.timeline.wait_idle(&self.queue)?;
        self.check_removed()
    }

    pub fn check_removed(&self) -> Result<(), Error> {
        match unsafe { self.device.GetDeviceRemovedReason() } {
            Ok(()) if !self.timeline.is_poisoned() => Ok(()),
            Ok(()) => Err(Error::DeviceLost {
                cause: LossCause::Unknown,
                detail: "fence poisoned".into(),
            }),
            Err(error) => Err(Error::DeviceLost {
                cause: classify(error.code().0).unwrap_or(LossCause::Unknown),
                detail: error.message(),
            }),
        }
    }

    pub fn create_swapchain(
        &self,
        hwnd: isize,
        width: u32,
        height: u32,
    ) -> Result<Swapchain, Error> {
        Swapchain::new(
            &self.device,
            &self.factory,
            &self.queue,
            hwnd,
            width,
            height,
            self.info.tearing,
        )
    }

    pub fn create_composition_swapchain(
        &self,
        width: u32,
        height: u32,
    ) -> Result<Swapchain, Error> {
        Swapchain::new_composition(&self.device, &self.factory, &self.queue, width, height)
    }

    pub fn resize_swapchain(
        &mut self,
        swapchain: &mut Swapchain,
        width: u32,
        height: u32,
    ) -> Result<bool, Error> {
        self.wait_idle()?;
        swapchain.resize(&self.device, width, height)
    }

    pub fn inject_device_removal(&self) -> Result<(), Error> {
        let device5: ID3D12Device5 = self.device.cast()?;
        unsafe { device5.RemoveDevice() };
        Ok(())
    }

    pub fn recover(self) -> Result<(Core, Recovered), Error> {
        let cause = self
            .check_removed()
            .err()
            .and_then(|error| error.loss_cause());
        let kind = self.info.adapter.kind;
        let luid = self.info.adapter.luid;
        let mut history = self.history.clone();
        let step = history.record(self.epoch.elapsed(), kind);
        let preference = match step {
            RecoveryStep::FallBackToWarp => AdapterPreference::Warp,
            RecoveryStep::Reselect => match kind {
                Adapter::Software => AdapterPreference::Warp,
                Adapter::Hardware => AdapterPreference::Luid(luid),
            },
        };
        let options = DeviceOptions {
            preference,
            ..self.options
        };
        let (epoch, generation) = (self.epoch, self.generation + 1);
        drop(self);
        let built = build(&options)?;
        let core = Self {
            options,
            factory: built.factory,
            device: built.device,
            queue: built.queue,
            info: built.info,
            timeline: built.timeline,
            frames: built.frames,
            rtv: built.rtv,
            srv: built.srv,
            monitor: built.monitor,
            history,
            epoch,
            generation,
            visibility: Visibility::Visible,
        };
        Ok((
            core,
            Recovered {
                cause,
                step,
                generation,
            },
        ))
    }

    pub fn memory(&self) -> Result<MemorySnapshot, Error> {
        self.monitor.snapshot()
    }

    pub fn set_visibility(&mut self, visibility: Visibility) -> Result<Trim, Error> {
        self.visibility = visibility;
        self.trim_now()
    }

    pub fn service_budget(&mut self) -> Result<Option<BudgetStatus>, Error> {
        match self.monitor.changed() {
            false => Ok(None),
            true => {
                let memory = self.monitor.snapshot()?;
                let level = pressure(memory.local);
                let applied = self.apply(trim_for(level, self.visibility))?;
                Ok(Some(BudgetStatus {
                    memory,
                    pressure: level,
                    applied,
                }))
            }
        }
    }

    fn trim_now(&mut self) -> Result<Trim, Error> {
        let level = pressure(self.monitor.snapshot()?.local);
        self.apply(trim_for(level, self.visibility))
    }

    pub fn trim(&mut self, trim: Trim) -> Result<Trim, Error> {
        self.apply(trim)
    }

    fn apply(&mut self, trim: Trim) -> Result<Trim, Error> {
        if trim.shrink_upload_ring {
            self.wait_idle()?;
            self.frames.upload.shrink_to_initial(&self.timeline)?;
        }
        Ok(trim)
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        let _ = self.timeline.wait_idle(&self.queue);
    }
}
