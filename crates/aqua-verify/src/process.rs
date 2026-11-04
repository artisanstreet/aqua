#![allow(unsafe_code)]

use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessMemory {
    pub private_bytes: u64,
    pub working_set: u64,
    pub peak_working_set: u64,
}

pub fn sample() -> windows::core::Result<ProcessMemory> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )?;
    }
    Ok(ProcessMemory {
        private_bytes: counters.PrivateUsage as u64,
        working_set: counters.WorkingSetSize as u64,
        peak_working_set: counters.PeakWorkingSetSize as u64,
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoMemory {
    pub local_usage: u64,
    pub local_budget: u64,
    pub non_local_usage: u64,
}

fn query(
    adapter: &windows::Win32::Graphics::Dxgi::IDXGIAdapter3,
    group: windows::Win32::Graphics::Dxgi::DXGI_MEMORY_SEGMENT_GROUP,
) -> windows::core::Result<windows::Win32::Graphics::Dxgi::DXGI_QUERY_VIDEO_MEMORY_INFO> {
    let mut info = windows::Win32::Graphics::Dxgi::DXGI_QUERY_VIDEO_MEMORY_INFO::default();
    unsafe { adapter.QueryVideoMemoryInfo(0, group, &mut info)? };
    Ok(info)
}

pub fn video_memory(high_performance: bool) -> windows::core::Result<VideoMemory> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter3, IDXGIFactory6, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
        DXGI_GPU_PREFERENCE_MINIMUM_POWER, DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
        DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL,
    };
    let factory: IDXGIFactory6 = unsafe { CreateDXGIFactory1()? };
    let preference = match high_performance {
        true => DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
        false => DXGI_GPU_PREFERENCE_MINIMUM_POWER,
    };
    let adapter: IDXGIAdapter3 = unsafe { factory.EnumAdapterByGpuPreference(0, preference)? };
    let local = query(&adapter, DXGI_MEMORY_SEGMENT_GROUP_LOCAL)?;
    let non_local = query(&adapter, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL)?;
    Ok(VideoMemory {
        local_usage: local.CurrentUsage,
        local_budget: local.Budget,
        non_local_usage: non_local.CurrentUsage,
    })
}
