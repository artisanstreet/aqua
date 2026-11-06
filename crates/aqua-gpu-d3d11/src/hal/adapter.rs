use std::ffi::c_void;
use std::mem::size_of;

use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, LUID};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;

use super::error::Error;
use super::selection::{Adapter, AdapterInfo, AdapterPreference};

const FEATURE_LEVELS: [D3D_FEATURE_LEVEL; 2] = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub adapter: AdapterInfo,
    pub feature_level_11_1: bool,
    pub unified_memory: bool,
    pub tearing: bool,
    pub debug_layer: bool,
    pub fell_back_to_warp: bool,
}

pub struct Selected {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext1,
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

struct Opened {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    level: D3D_FEATURE_LEVEL,
    debug: bool,
}

fn create(adapter: &IDXGIAdapter1, flags: D3D11_CREATE_DEVICE_FLAG) -> Result<Opened, Error> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    let mut level = D3D_FEATURE_LEVEL_11_0;
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            flags,
            Some(&FEATURE_LEVELS),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut level),
            Some(&mut context),
        )?
    };
    match (device, context) {
        (Some(device), Some(context)) => Ok(Opened {
            device,
            context,
            level,
            debug: flags.0 & D3D11_CREATE_DEVICE_DEBUG.0 != 0,
        }),
        _ => Err(Error::NoAdapter),
    }
}

#[derive(Clone, Copy)]
pub struct Creation {
    pub debug: bool,
    pub single_threaded: bool,
}

fn open_device(adapter: &IDXGIAdapter1, creation: Creation) -> Result<Opened, Error> {
    let debug = creation.debug;
    let base = match creation.single_threaded {
        true => D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_SINGLETHREADED,
        false => D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    };
    let base = match std::env::var("AQUA_D3D11_PREVENT_THREADING").as_deref() {
        Ok("1") => base | D3D11_CREATE_DEVICE_PREVENT_INTERNAL_THREADING_OPTIMIZATIONS,
        _ => base,
    };
    match debug {
        false => create(adapter, base),
        true => {
            create(adapter, base | D3D11_CREATE_DEVICE_DEBUG).or_else(|_| create(adapter, base))
        }
    }
}

fn unified_memory(device: &ID3D11Device) -> bool {
    let mut data = D3D11_FEATURE_DATA_D3D11_OPTIONS2::default();
    let queried = unsafe {
        device.CheckFeatureSupport(
            D3D11_FEATURE_D3D11_OPTIONS2,
            &mut data as *mut _ as *mut c_void,
            size_of::<D3D11_FEATURE_DATA_D3D11_OPTIONS2>() as u32,
        )
    };
    queried.is_ok() && data.UnifiedMemoryArchitecture.as_bool()
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
    creation: Creation,
    fell_back: bool,
) -> Result<Selected, Error> {
    let stages = crate::stages::StageClock::from_env();
    let adapter_info = describe_adapter(&adapter)?;
    stages.mark("      describe_adapter");
    let opened = open_device(&adapter, creation)?;
    stages.mark("      create_device_call");
    let info = DeviceInfo {
        adapter: adapter_info,
        feature_level_11_1: opened.level == D3D_FEATURE_LEVEL_11_1,
        unified_memory: unified_memory(&opened.device),
        tearing: tearing_supported(factory),
        debug_layer: opened.debug,
        fell_back_to_warp: fell_back,
    };
    Ok(Selected {
        context: opened.context.cast()?,
        device: opened.device,
        adapter: adapter.cast()?,
        info,
    })
}

fn warp(factory: &IDXGIFactory6, creation: Creation, fell_back: bool) -> Result<Selected, Error> {
    let adapter: IDXGIAdapter1 = unsafe { factory.EnumWarpAdapter()? };
    open_selected(factory, adapter, creation, fell_back)
}

fn hardware(
    factory: &IDXGIFactory6,
    preference: DXGI_GPU_PREFERENCE,
    creation: Creation,
) -> Option<Selected> {
    (0..)
        .map_while(|index| {
            unsafe { factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(index, preference) }.ok()
        })
        .filter(|adapter| {
            unsafe { adapter.GetDesc1() }
                .is_ok_and(|desc| desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0)
        })
        .find_map(|adapter| open_selected(factory, adapter, creation, false).ok())
}

fn by_luid(factory: &IDXGIFactory6, luid: i64, creation: Creation) -> Option<Selected> {
    let adapter: IDXGIAdapter1 = unsafe { factory.EnumAdapterByLuid(luid_from(luid)) }.ok()?;
    open_selected(factory, adapter, creation, false).ok()
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

pub fn select(
    factory: &IDXGIFactory6,
    preference: AdapterPreference,
    creation: Creation,
) -> Result<Selected, Error> {
    let hardware_chain = |gpu: DXGI_GPU_PREFERENCE| match hardware(factory, gpu, creation) {
        Some(selected) => Ok(selected),
        None => warp(factory, creation, true),
    };
    match preference {
        AdapterPreference::Warp => warp(factory, creation, false),
        AdapterPreference::HighPerformance => hardware_chain(DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE),
        AdapterPreference::MinimumPower => hardware_chain(DXGI_GPU_PREFERENCE_MINIMUM_POWER),
        AdapterPreference::Luid(luid) => match by_luid(factory, luid, creation) {
            Some(selected) => Ok(selected),
            None => hardware_chain(DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE),
        },
    }
}
