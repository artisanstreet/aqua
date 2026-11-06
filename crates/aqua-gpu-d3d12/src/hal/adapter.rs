use std::ffi::c_void;
use std::mem::size_of;

use windows::core::Interface;
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::*;

use super::error::Error;
use super::features;
use super::selection::{Adapter, AdapterInfo, AdapterPreference};
use super::tier::{HardwareFeatures, Tier, Tiers};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub adapter: AdapterInfo,
    pub features: HardwareFeatures,
    pub tiers: Tiers,
    pub unified_memory: bool,
    pub tearing: bool,
    pub fell_back_to_warp: bool,
}

pub struct Selected {
    pub device: ID3D12Device,
    pub adapter: IDXGIAdapter3,
    pub info: DeviceInfo,
}

fn adapter_name(desc: &DXGI_ADAPTER_DESC1) -> String {
    let end = desc
        .Description
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(desc.Description.len());
    String::from_utf16_lossy(&desc.Description[..end])
}

fn luid_value(luid: LUID) -> i64 {
    ((luid.HighPart as i64) << 32) | luid.LowPart as i64
}

fn luid_from(value: i64) -> LUID {
    LUID {
        LowPart: value as u32,
        HighPart: (value >> 32) as i32,
    }
}

fn describe_adapter(adapter: &IDXGIAdapter1) -> Result<AdapterInfo, Error> {
    let desc = unsafe { adapter.GetDesc1()? };
    Ok(AdapterInfo {
        name: adapter_name(&desc),
        luid: luid_value(desc.AdapterLuid),
        vendor_id: desc.VendorId,
        kind: match desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            true => Adapter::Software,
            false => Adapter::Hardware,
        },
        dedicated_video_bytes: desc.DedicatedVideoMemory as u64,
        shared_system_bytes: desc.SharedSystemMemory as u64,
    })
}

fn open_device(adapter: &IDXGIAdapter1) -> Result<ID3D12Device, Error> {
    let mut device: Option<ID3D12Device> = None;
    unsafe { D3D12CreateDevice(adapter, D3D_FEATURE_LEVEL_11_0, &mut device)? };
    device.ok_or(Error::NoAdapter)
}

fn unified_memory(device: &ID3D12Device) -> bool {
    let mut data = D3D12_FEATURE_DATA_ARCHITECTURE::default();
    let queried = unsafe {
        device.CheckFeatureSupport(
            D3D12_FEATURE_ARCHITECTURE,
            &mut data as *mut _ as *mut c_void,
            size_of::<D3D12_FEATURE_DATA_ARCHITECTURE>() as u32,
        )
    };
    queried.is_ok() && data.UMA.as_bool()
}

fn tearing_supported(factory: &IDXGIFactory6) -> bool {
    let mut allowed = windows::core::BOOL(0);
    let queried = unsafe {
        factory.CheckFeatureSupport(
            DXGI_FEATURE_PRESENT_ALLOW_TEARING,
            &mut allowed as *mut _ as *mut c_void,
            size_of::<windows::core::BOOL>() as u32,
        )
    };
    queried.is_ok() && allowed.as_bool()
}

fn open_selected(
    factory: &IDXGIFactory6,
    adapter: IDXGIAdapter1,
    cap: Tier,
    fell_back: bool,
) -> Result<Selected, Error> {
    let stages = crate::stages::StageClock::from_env();
    let adapter_info = describe_adapter(&adapter)?;
    stages.mark("      describe_adapter");
    let device = open_device(&adapter)?;
    stages.mark("      create_device_call");
    let features = features::detect(&device);
    stages.mark("      detect_features");
    let info = DeviceInfo {
        adapter: adapter_info,
        features,
        tiers: features.tiers(cap),
        unified_memory: unified_memory(&device),
        tearing: tearing_supported(factory),
        fell_back_to_warp: fell_back,
    };
    Ok(Selected {
        device,
        adapter: adapter.cast()?,
        info,
    })
}

fn warp(factory: &IDXGIFactory6, cap: Tier, fell_back: bool) -> Result<Selected, Error> {
    let adapter: IDXGIAdapter1 = unsafe { factory.EnumWarpAdapter()? };
    open_selected(factory, adapter, cap, fell_back)
}

fn usable(selected: Selected) -> Option<Selected> {
    selected.info.features.meets_baseline().then_some(selected)
}

fn hardware(
    factory: &IDXGIFactory6,
    preference: DXGI_GPU_PREFERENCE,
    cap: Tier,
) -> Option<Selected> {
    (0..)
        .map_while(|index| {
            unsafe { factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(index, preference) }.ok()
        })
        .filter(|adapter| {
            unsafe { adapter.GetDesc1() }
                .is_ok_and(|desc| desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0)
        })
        .find_map(|adapter| {
            open_selected(factory, adapter, cap, false)
                .ok()
                .and_then(usable)
        })
}

fn by_luid(factory: &IDXGIFactory6, luid: i64, cap: Tier) -> Option<Selected> {
    let adapter: IDXGIAdapter1 = unsafe { factory.EnumAdapterByLuid(luid_from(luid)) }.ok()?;
    open_selected(factory, adapter, cap, false)
        .ok()
        .and_then(usable)
}

pub fn list_adapters(factory: &IDXGIFactory6) -> Vec<AdapterInfo> {
    (0..)
        .map_while(|index| {
            unsafe {
                factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(
                    index,
                    DXGI_GPU_PREFERENCE_UNSPECIFIED,
                )
            }
            .ok()
        })
        .filter_map(|adapter| describe_adapter(&adapter).ok())
        .collect()
}

pub fn create_factory(debug: bool) -> Result<IDXGIFactory6, Error> {
    let flags = match debug {
        true => DXGI_CREATE_FACTORY_DEBUG,
        false => DXGI_CREATE_FACTORY_FLAGS(0),
    };
    let factory: IDXGIFactory2 = match unsafe { CreateDXGIFactory2(flags) } {
        Ok(factory) => factory,
        Err(_) if debug => unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? },
        Err(error) => return Err(error.into()),
    };
    Ok(factory.cast()?)
}

pub fn enable_debug_layer() {
    let mut debug: Option<ID3D12Debug> = None;
    if unsafe { D3D12GetDebugInterface(&mut debug) }.is_ok() {
        if let Some(debug) = debug {
            unsafe { debug.EnableDebugLayer() };
        }
    }
}

pub fn select(
    factory: &IDXGIFactory6,
    preference: AdapterPreference,
    cap: Tier,
) -> Result<Selected, Error> {
    let hardware_chain = |gpu: DXGI_GPU_PREFERENCE| match hardware(factory, gpu, cap) {
        Some(selected) => Ok(selected),
        None => warp(factory, cap, true),
    };
    match preference {
        AdapterPreference::Warp => warp(factory, cap, false),
        AdapterPreference::HighPerformance => hardware_chain(DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE),
        AdapterPreference::MinimumPower => hardware_chain(DXGI_GPU_PREFERENCE_MINIMUM_POWER),
        AdapterPreference::Luid(luid) => match by_luid(factory, luid, cap) {
            Some(selected) => Ok(selected),
            None => hardware_chain(DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE),
        },
    }
}
