use std::ffi::c_void;
use std::mem::size_of;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::*;

use super::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterPreference {
    HighPerformance,
    MinimumPower,
    Warp,
}

pub struct Selected {
    pub device: ID3D12Device,
    pub name: String,
    pub software: bool,
    pub unified_memory: bool,
}

fn adapter_name(desc: &DXGI_ADAPTER_DESC1) -> String {
    let end = desc
        .Description
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(desc.Description.len());
    String::from_utf16_lossy(&desc.Description[..end])
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

fn describe(adapter: &IDXGIAdapter1, device: ID3D12Device) -> Result<Selected, Error> {
    let desc = unsafe { adapter.GetDesc1()? };
    Ok(Selected {
        name: adapter_name(&desc),
        software: desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
        unified_memory: unified_memory(&device),
        device,
    })
}

fn warp(factory: &IDXGIFactory6) -> Result<Selected, Error> {
    let adapter: IDXGIAdapter1 = unsafe { factory.EnumWarpAdapter()? };
    let device = open_device(&adapter)?;
    describe(&adapter, device)
}

fn hardware(factory: &IDXGIFactory6, preference: DXGI_GPU_PREFERENCE) -> Result<Selected, Error> {
    let mut index = 0;
    loop {
        let adapter: Result<IDXGIAdapter1, _> =
            unsafe { factory.EnumAdapterByGpuPreference(index, preference) };
        match adapter {
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => return Err(Error::NoAdapter),
            Err(error) => return Err(error.into()),
            Ok(adapter) => {
                index += 1;
                let desc = unsafe { adapter.GetDesc1()? };
                if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                    continue;
                }
                if let Ok(device) = open_device(&adapter) {
                    return describe(&adapter, device);
                }
            }
        }
    }
}

pub fn select(preference: AdapterPreference) -> Result<Selected, Error> {
    let factory2: IDXGIFactory2 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
    let factory: IDXGIFactory6 = factory2.cast()?;
    match preference {
        AdapterPreference::Warp => warp(&factory),
        AdapterPreference::HighPerformance => {
            hardware(&factory, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE)
        }
        AdapterPreference::MinimumPower => hardware(&factory, DXGI_GPU_PREFERENCE_MINIMUM_POWER),
    }
}
