use std::cell::Cell;
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::Interface;

use super::adapter::{DeviceInfo, create_factory, select};
use super::budget::{Pressure, Trim, Visibility, pressure, trim_for};
use super::error::Error;
use super::monitor::{BudgetMonitor, MemorySnapshot};
use super::recovery::{LossCause, LossHistory, RecoveryStep, classify};
use super::selection::{Adapter, AdapterPreference, parse_preference};
use super::swapchain::{Swapchain, SwapchainTarget};

const LOSS_WINDOW: Duration = Duration::from_secs(10);
const LOSS_LIMIT: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceOptions {
    pub preference: AdapterPreference,
    pub debug_layer: bool,
}

impl Default for DeviceOptions {
    fn default() -> Self {
        Self {
            preference: AdapterPreference::HighPerformance,
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
            debug_layer: std::env::var("AQUA_D3D11_DEBUG").as_deref() == Ok("1"),
        }
    }

    pub fn with_preference(mut self, preference: AdapterPreference) -> Self {
        self.preference = preference;
        self
    }

    pub fn with_debug_layer(mut self, debug_layer: bool) -> Self {
        self.debug_layer = debug_layer;
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
    device: ID3D11Device,
    context: ID3D11DeviceContext1,
    info: DeviceInfo,
    monitor: BudgetMonitor,
    history: LossHistory,
    epoch: Instant,
    generation: u32,
    visibility: Visibility,
    injected: Cell<bool>,
}

struct Built {
    factory: IDXGIFactory6,
    device: ID3D11Device,
    context: ID3D11DeviceContext1,
    info: DeviceInfo,
    monitor: BudgetMonitor,
}

fn build(options: &DeviceOptions) -> Result<Built, Error> {
    let stages = crate::stages::StageClock::from_env();
    let factory = create_factory(options.debug_layer)?;
    stages.mark("    factory");
    let selected = select(&factory, options.preference, options.debug_layer)?;
    stages.mark("    select_device");
    Ok(Built {
        monitor: BudgetMonitor::new(&selected.adapter)?,
        info: selected.info,
        factory,
        device: selected.device,
        context: selected.context,
    })
}

impl Core {
    pub fn new(options: DeviceOptions) -> Result<Self, Error> {
        let built = build(&options)?;
        Ok(Self::assemble(
            options,
            built,
            LossHistory::new(LOSS_WINDOW, LOSS_LIMIT),
            Instant::now(),
            0,
        ))
    }

    fn assemble(
        options: DeviceOptions,
        built: Built,
        history: LossHistory,
        epoch: Instant,
        generation: u32,
    ) -> Self {
        Self {
            options,
            factory: built.factory,
            device: built.device,
            context: built.context,
            info: built.info,
            monitor: built.monitor,
            history,
            epoch,
            generation,
            visibility: Visibility::Visible,
            injected: Cell::new(false),
        }
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    pub fn context(&self) -> &ID3D11DeviceContext1 {
        &self.context
    }

    pub fn factory(&self) -> &IDXGIFactory6 {
        &self.factory
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn options(&self) -> DeviceOptions {
        self.options
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn check_removed(&self) -> Result<(), Error> {
        match unsafe { self.device.GetDeviceRemovedReason() } {
            Ok(()) if !self.injected.get() => Ok(()),
            Ok(()) => Err(Error::DeviceLost {
                cause: LossCause::Removed,
                detail: "device removal injected".into(),
            }),
            Err(error) => Err(Error::DeviceLost {
                cause: classify(error.code().0).unwrap_or(LossCause::Unknown),
                detail: error.message(),
            }),
        }
    }

    pub fn release_deferred(&self) {
        unsafe {
            self.context.ClearState();
            self.context.Flush();
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
            SwapchainTarget::Hwnd(hwnd),
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
        Swapchain::new(
            &self.device,
            &self.factory,
            SwapchainTarget::Composition,
            width,
            height,
            false,
        )
    }

    pub fn resize_swapchain(
        &self,
        swapchain: &mut Swapchain,
        width: u32,
        height: u32,
    ) -> Result<bool, Error> {
        self.check_removed()?;
        swapchain.resize(&self.device, &self.context, width, height)
    }

    pub fn inject_device_removal(&self) {
        self.injected.set(true);
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
        self.release_deferred();
        drop(self);
        let core = Self::assemble(options, build(&options)?, history, epoch, generation);
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
        let level = pressure(self.monitor.snapshot()?.local);
        self.apply(trim_for(level, self.visibility))
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

    pub fn trim(&mut self, trim: Trim) -> Result<Trim, Error> {
        self.apply(trim)
    }

    fn apply(&mut self, trim: Trim) -> Result<Trim, Error> {
        self.check_removed()?;
        Ok(trim)
    }

    pub fn trim_device(&self) {
        self.release_deferred();
        if let Ok(device) = self.device.cast::<IDXGIDevice3>() {
            unsafe { device.Trim() };
        }
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.release_deferred();
    }
}
