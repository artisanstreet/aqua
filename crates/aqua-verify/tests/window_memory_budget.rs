#![cfg(windows)]
#![allow(unsafe_code)]

use std::time::Instant;

use aqua_gpu_d3d12::hal::selection::AdapterPreference;
use aqua_gpu_d3d12::hal::DeviceOptions;
use aqua_gpu_d3d12::window::{WindowBackend, WindowOptions};
use aqua_gpu_ref::conformance::assets::standard_uploads;
use aqua_gpu_ref::conformance::cases::hex;
use aqua_graph::{plan_paint, Extent};
use aqua_protocol::{FrameInputs, GpuBackend, NativeSurface, PresentHint, SurfaceTarget, WindowId};
use aqua_verify::budget::{Measured, MemoryBudget, MIB, RENDERER_WINDOWED};
use aqua_verify::heap::{live_bytes, CountingAllocator};
use aqua_verify::process::{sample, video_memory};
use aqua_verify::stats::{intervals, jank, Distribution};
use aqua_verify::workload::editor_workload;
use windows::core::w;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetClientRect, PeekMessageW, SetWindowPos,
    ShowWindow, TranslateMessage, HWND_BOTTOM, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE, WS_POPUP,
};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const SURFACE: Extent = Extent {
    width: 2560,
    height: 1440,
};
const WARMUP_FRAMES: usize = 60;
const STEADY_FRAMES: usize = 600;

struct Window(HWND);

impl Window {
    fn open(extent: Extent) -> Self {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("aqua-window-budget"),
                WS_POPUP,
                0,
                0,
                extent.width as i32,
                extent.height as i32,
                None,
                None,
                None,
                None,
            )
        }
        .expect("window");
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_BOTTOM),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        let window = Self(hwnd);
        window.pump();
        window
    }

    fn client(&self) -> Extent {
        let mut rect = RECT::default();
        unsafe { GetClientRect(self.0, &mut rect).expect("client rect") };
        Extent {
            width: (rect.right - rect.left) as u32,
            height: (rect.bottom - rect.top) as u32,
        }
    }

    fn pump(&self) {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

struct Stage {
    name: &'static str,
    private: u64,
    heap: u64,
    vram: u64,
}

fn stage(name: &'static str, high_performance: bool) -> Stage {
    Stage {
        name,
        private: sample().unwrap().private_bytes,
        heap: live_bytes(),
        vram: video_memory(high_performance)
            .map(|memory| memory.local_usage)
            .unwrap_or(0),
    }
}

fn run(preference: AdapterPreference, high_performance: bool, budget: Option<MemoryBudget>) {
    let mut stages = vec![stage("process", high_performance)];
    let options = WindowOptions {
        device: DeviceOptions::from_env().with_preference(preference),
        ..WindowOptions::default()
    };
    let mut backend = match WindowBackend::new(options) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("SKIP-NO-ADAPTER {preference:?}: {error}");
            return;
        }
    };
    stages.push(stage("device and renderer", high_performance));
    let window = Window::open(SURFACE);
    let extent = window.client();
    let surface = backend
        .create_surface(SurfaceTarget::Native(NativeSurface {
            window: WindowId::new(1).unwrap(),
            hwnd: window.0 .0 as u64,
            extent,
        }))
        .unwrap();
    backend.upload(standard_uploads()).unwrap();
    stages.push(stage("swapchain and uploads", high_performance));
    let plan = plan_paint(extent, hex("#000000", 1.0), &editor_workload(extent)).unwrap();
    let inputs = FrameInputs::new(surface);
    let frame = |backend: &mut WindowBackend, count: usize| -> (Vec<f64>, Vec<f64>) {
        let mut cpu = Vec::with_capacity(count);
        let mut stamps = Vec::with_capacity(count);
        let origin = Instant::now();
        for _ in 0..count {
            let started = Instant::now();
            backend.wait_for_frame_slot(surface).unwrap();
            backend.execute(&plan, &inputs).unwrap();
            backend.present(surface, PresentHint::Immediate).unwrap();
            window.pump();
            cpu.push(started.elapsed().as_secs_f64() * 1000.0);
            stamps.push(origin.elapsed().as_secs_f64() * 1000.0);
        }
        (cpu, stamps)
    };
    frame(&mut backend, 1);
    stages.push(stage("first present", high_performance));
    frame(&mut backend, WARMUP_FRAMES);
    stages.push(stage("after warmup", high_performance));
    let (steady, stamps) = frame(&mut backend, STEADY_FRAMES);
    stages.push(stage("after steady state", high_performance));

    eprintln!(
        "windowed memory {preference:?} {}x{} ({} passes)",
        extent.width,
        extent.height,
        plan.passes.len()
    );
    let mut previous = 0u64;
    for entry in &stages {
        eprintln!(
            "  {:<22} private {:>8.2} MiB (+{:>7.2})  heap {:>7.2} MiB  vram {:>8.2} MiB",
            entry.name,
            entry.private as f64 / MIB as f64,
            entry.private.saturating_sub(previous) as f64 / MIB as f64,
            entry.heap as f64 / MIB as f64,
            entry.vram as f64 / MIB as f64
        );
        previous = entry.private;
    }
    let distribution = Distribution::from_samples(&steady).unwrap();
    let janky = jank(&steady, 16.667);
    eprintln!(
        "  frame cpu ms {distribution}; over 16.7 ms: {} of {}, longest run {}",
        janky.over_budget, janky.frames, janky.longest_run
    );
    if let Some(pacing) = Distribution::from_samples(&intervals(&stamps)) {
        eprintln!("  present interval ms {pacing}");
    }

    let baseline = &stages[0];
    let warm = &stages[stages.len() - 2];
    let steady_end = &stages[stages.len() - 1];
    let measured = Measured {
        private_total: steady_end.private.saturating_sub(baseline.private),
        framework_heap: steady_end.heap,
        steady_growth: steady_end.private.saturating_sub(warm.private),
        vram_total: steady_end.vram,
    };
    let breaches = budget
        .map(|budget| budget.check(&measured))
        .unwrap_or_default();
    assert!(
        breaches.is_empty(),
        "{}",
        breaches
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        steady_end.heap <= warm.heap + 256 * 1024,
        "framework heap grew from {} to {}",
        warm.heap,
        steady_end.heap
    );
    assert!(
        steady_end.vram <= warm.vram + MIB,
        "video memory grew from {} to {}",
        warm.vram,
        steady_end.vram
    );
}

#[test]
fn hardware_swapchain_stays_inside_windowed_budget() {
    run(
        AdapterPreference::HighPerformance,
        true,
        Some(RENDERER_WINDOWED),
    );
}

#[test]
fn warp_swapchain_steady_state_does_not_grow() {
    run(AdapterPreference::Warp, false, None);
}
