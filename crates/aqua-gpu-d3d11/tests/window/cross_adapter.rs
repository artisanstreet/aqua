use std::time::Instant;

use aqua_gpu_d3d11::hal::selection::{Adapter, AdapterInfo, AdapterPreference};
use aqua_gpu_d3d11::hal::{create_factory, list_adapters};
use aqua_gpu_d3d11::window::{WindowBackend, WindowOptions};
use aqua_protocol::{FrameInputs, GpuBackend, PresentHint};
use windows::Win32::Foundation::{HWND, LUID};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter1, IDXGIOutput};
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTOPRIMARY};

use crate::backend::{frame, matches_reference, open, options, scene};
use crate::test_window::TestWindow;

fn hardware_adapters() -> Vec<AdapterInfo> {
    let factory = create_factory(false).expect("factory");
    list_adapters(&factory)
        .into_iter()
        .filter(|info| info.kind == Adapter::Hardware)
        .collect()
}

fn drives_window(info: &AdapterInfo, window: &TestWindow) -> bool {
    let factory = create_factory(false).expect("factory");
    let luid = LUID {
        LowPart: info.luid as u32,
        HighPart: (info.luid >> 32) as i32,
    };
    let Ok(adapter) = (unsafe { factory.EnumAdapterByLuid::<IDXGIAdapter1>(luid) }) else {
        return false;
    };
    let monitor =
        unsafe { MonitorFromWindow(HWND(window.raw() as *mut _), MONITOR_DEFAULTTOPRIMARY) };
    (0..)
        .map_while(|index| unsafe { adapter.EnumOutputs(index) }.ok())
        .filter_map(|output: IDXGIOutput| unsafe { output.GetDesc() }.ok())
        .any(|desc| desc.Monitor == monitor)
}

fn present_on(info: &AdapterInfo) -> (bool, u64, u64) {
    crate::support::hold_software();
    let preference = AdapterPreference::Luid(info.luid);
    let mut backend = WindowBackend::new(WindowOptions {
        device: options(preference).device,
        mirror_textures: false,
    })
    .unwrap_or_else(|error| panic!("{} must open: {error}", info.name));
    assert_eq!(backend.capabilities().adapter, info.name);
    assert!(!backend.capabilities().features.software);
    let window = TestWindow::open(520, 340);
    let native = drives_window(info, &window);
    let (id, extent) = open(&mut backend, &window);
    matches_reference(&mut backend, id, &scene(extent, 0.2), &info.name);
    window.pump();
    let started = Instant::now();
    let frames = 90;
    for index in 0..frames {
        backend.wait_for_frame_slot(id).unwrap();
        backend
            .execute(
                &scene(extent, index as f32 / frames as f32),
                &FrameInputs::new(id),
            )
            .unwrap();
        backend.present(id, PresentHint::Vsync).unwrap();
        window.pump();
    }
    let mean = started.elapsed().as_secs_f64() * 1000.0 / frames as f64;
    let interval = backend.refresh_interval(id).unwrap().as_secs_f64() * 1000.0;
    let report = backend.pacing(id).unwrap();
    eprintln!(
        "[present] {} ({} MiB dedicated) {} the monitor: mean {mean:.2} ms of {interval:.2} ms, missed {}",
        info.name,
        info.dedicated_video_bytes >> 20,
        if native {
            "drives"
        } else {
            "does not drive (cross-adapter)"
        },
        backend.pacing(id).unwrap().missed,
    );
    frame(&mut backend, &window, id, &scene(extent, 0.9));
    assert_eq!(backend.stats().occluded, 0);
    (native, report.frames, report.missed)
}

#[test]
fn every_hardware_adapter_presents_to_a_window_native_or_across_adapters() {
    let adapters = hardware_adapters();
    assert!(!adapters.is_empty(), "no hardware adapter on this machine");
    let results: Vec<(String, bool, u64, u64)> = adapters
        .iter()
        .map(|info| {
            let (native, frames, missed) = present_on(info);
            (info.name.clone(), native, frames, missed)
        })
        .collect();
    for (name, native, frames, missed) in &results {
        assert!(
            *frames >= 90 && *missed < *frames,
            "{name} (native={native}) missed {missed} of {frames} vblanks"
        );
    }
    assert!(
        adapters.len() >= 2,
        "cross-adapter present needs a second adapter, only {} is enabled",
        adapters[0].name
    );
    assert!(
        results.iter().any(|(_, native, _, _)| !native),
        "a second adapter exists but none presented across adapters"
    );
}

#[test]
fn the_integrated_adapter_presents_and_matches_the_reference() {
    let adapters = hardware_adapters();
    let Some(integrated) = adapters
        .iter()
        .min_by_key(|info| info.dedicated_video_bytes)
    else {
        panic!("no hardware adapter is enabled");
    };
    assert!(
        adapters.len() >= 2,
        "no integrated adapter: only {} is enabled",
        integrated.name
    );
    crate::support::hold_software();
    let mut backend = WindowBackend::new(WindowOptions {
        device: options(AdapterPreference::Luid(integrated.luid)).device,
        mirror_textures: false,
    })
    .expect("integrated adapter opens");
    assert_eq!(backend.capabilities().adapter, integrated.name);
    let window = TestWindow::open(520, 340);
    let (id, extent) = open(&mut backend, &window);
    matches_reference(&mut backend, id, &scene(extent, 0.4), "integrated");
    for index in 0..30 {
        frame(
            &mut backend,
            &window,
            id,
            &scene(extent, index as f32 / 30.0),
        );
    }
    assert_eq!(backend.stats().occluded, 0);
}
