use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DwmGetCompositionTimingInfo, DWM_TIMING_INFO};
use windows::Win32::Graphics::Gdi::{
    EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromWindow, DEVMODEW, ENUM_CURRENT_SETTINGS,
    HMONITOR, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use super::pacing::{
    interval_from_ticks, reconcile_interval, ticks_until_next_vblank, FALLBACK_INTERVAL,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vblank {
    pub interval: Duration,
    pub until_next: Duration,
}

fn frequency() -> u64 {
    let mut value = 0i64;
    let _ = unsafe { QueryPerformanceFrequency(&mut value) };
    value.max(0) as u64
}

fn counter() -> u64 {
    let mut value = 0i64;
    let _ = unsafe { QueryPerformanceCounter(&mut value) };
    value.max(0) as u64
}

pub fn query() -> Option<Vblank> {
    let mut info = DWM_TIMING_INFO {
        cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    unsafe { DwmGetCompositionTimingInfo(HWND(std::ptr::null_mut()), &mut info) }.ok()?;
    let frequency = frequency();
    let now = counter();
    let period = info.qpcRefreshPeriod;
    let remaining = ticks_until_next_vblank(now, info.qpcVBlank, period);
    Some(Vblank {
        interval: interval_from_ticks(period, frequency),
        until_next: Duration::from_nanos(
            ((remaining as u128 * 1_000_000_000) / frequency.max(1) as u128) as u64,
        ),
    })
}

pub fn refresh_interval() -> Duration {
    query()
        .map(|vblank| vblank.interval)
        .unwrap_or(FALLBACK_INTERVAL)
}

pub fn monitor_of(hwnd: isize) -> isize {
    unsafe { MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST) }.0 as isize
}

fn monitor_hz(monitor: isize) -> u32 {
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    let monitor = HMONITOR(monitor as *mut _);
    if !unsafe { GetMonitorInfoW(monitor, &mut info as *mut _ as *mut MONITORINFO) }.as_bool() {
        return 0;
    }
    let mut mode = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    match unsafe {
        EnumDisplaySettingsW(
            windows::core::PCWSTR(info.szDevice.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &mut mode,
        )
    }
    .as_bool()
    {
        true => mode.dmDisplayFrequency,
        false => 0,
    }
}

pub fn monitor_interval(monitor: isize) -> Duration {
    reconcile_interval(refresh_interval(), monitor_hz(monitor))
}
