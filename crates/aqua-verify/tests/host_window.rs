#![cfg(windows)]

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant as WallInstant};

use aqua::render::host::{FrameTarget, GpuFrames};
use aqua::render::link::InitFactory;
use aqua::render::lower::Lowering;
use aqua::{Aqua, AquaConfig, UiApp};
use aqua_core::{InputEvent, Point, ScrollDelta};
use aqua_gpu_d3d12::hal::selection::{Adapter, AdapterInfo, AdapterPreference};
use aqua_gpu_d3d12::hal::{create_factory, list_adapters, DeviceOptions};
use aqua_gpu_d3d12::window::{WindowBackend, WindowOptions};
use aqua_graph::Extent;
use aqua_protocol::geometry::{AquaWindowId, LogicalSize};
use aqua_protocol::window::WindowEvent;
use aqua_protocol::{GpuBackend, NativeSurface, SurfaceTarget, WindowId};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_verify::budget::{Measured, MemoryBudget, HOST_WINDOWED, MIB};
use aqua_verify::heap::{live_bytes, CountingAllocator};
use aqua_verify::process::{sample, video_memory};
use aqua_verify::stats::{jank, Distribution};
use windows::Win32::Foundation::{HWND, LUID};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter1, IDXGIOutput};
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTOPRIMARY};

#[path = "support/ui.rs"]
mod ui;
#[path = "support/window.rs"]
mod window;

use ui::{editor_like, observed};
use window::Window;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const SURFACE: Extent = Extent {
    width: 2560,
    height: 1440,
};
const WARMUP_INPUTS: usize = 60;
const STEADY_INPUTS: usize = 600;
const RELEASE_INPUT_P99_MS: f64 = 4.0;

fn factory(
    preference: AdapterPreference,
    hwnd: u64,
    extent: Extent,
    fonts: SharedFonts,
    runs: SharedRuns,
) -> InitFactory {
    Box::new(move |context| {
        Box::new(move || {
            let mut backend = WindowBackend::new(WindowOptions {
                device: DeviceOptions::from_env().with_preference(preference),
                ..WindowOptions::default()
            })
            .map_err(|error| error.to_string())?;
            let surface = backend
                .create_surface(SurfaceTarget::Native(NativeSurface {
                    window: WindowId::new(1).unwrap(),
                    hwnd,
                    extent,
                }))
                .map_err(|error| error.to_string())?;
            Ok(Box::new(GpuFrames::new(
                backend,
                surface,
                extent,
                Lowering::new(fonts, runs),
                context.externals,
                context.stats,
            )) as Box<dyn FrameTarget>)
        })
    })
}

struct Outcome {
    private: u64,
    heap: u64,
    vram: u64,
    growth: u64,
    heap_growth: u64,
    vram_growth: u64,
    presented: u64,
    failures: u64,
    startup_failures: u64,
    recoveries: u64,
    input_ms: Vec<f64>,
}

fn pump(aqua: &mut Aqua, app: &mut UiApp, id: AquaWindowId, window: &Window) {
    window.pump();
    let now = aqua.clock();
    aqua.pump_inbox(app);
    aqua.draw_ready(app, now);
    aqua.drain_render_events(app, id);
}

fn run(preference: AdapterPreference, high_performance: bool) -> Option<Outcome> {
    let baseline = sample().unwrap().private_bytes;
    let window = Window::open(SURFACE);
    let extent = window.client();
    let mut aqua = Aqua::new(AquaConfig::default()).unwrap();
    let mut app = UiApp::new(aqua.fonts().clone(), aqua.runs().clone());
    editor_like(&mut app, extent.width as f32, extent.height as f32);
    let id = aqua.add_headless_window(observed(extent.width as f32, extent.height as f32));
    aqua.attach_render(
        id,
        factory(
            preference,
            window.0 .0 as u64,
            extent,
            aqua.fonts().clone(),
            aqua.runs().clone(),
        ),
        Duration::from_millis(2),
    );
    aqua.dispatch_window_event(
        &mut app,
        id,
        &WindowEvent::Resized(
            id,
            LogicalSize::new(f64::from(extent.width), f64::from(extent.height)),
        ),
    );
    let stats = aqua.render_stats(id).unwrap();
    let deadline = WallInstant::now() + Duration::from_secs(30);
    while aqua.presented_frames(id).unwrap_or(0) == 0 {
        if WallInstant::now() > deadline {
            eprintln!("SKIP-NO-ADAPTER {preference:?}: nothing presented");
            return None;
        }
        pump(&mut aqua, &mut app, id, &window);
        std::thread::sleep(Duration::from_millis(2));
    }
    let startup_failures = stats.failures.load(Ordering::Relaxed);
    let wheel = |aqua: &mut Aqua, app: &mut UiApp, index: usize| {
        let started = WallInstant::now();
        let y = if (index / 40).is_multiple_of(2) {
            60.0
        } else {
            -60.0
        };
        aqua.dispatch_input(
            app,
            id,
            &InputEvent::Scroll {
                at: Point::new(1000.0, 600.0),
                delta: ScrollDelta { x: 0.0, y },
            },
        );
        pump(aqua, app, id, &window);
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        std::thread::sleep(Duration::from_millis(8));
        elapsed
    };
    (0..WARMUP_INPUTS).for_each(|index| {
        wheel(&mut aqua, &mut app, index);
    });
    let settle = |aqua: &mut Aqua, app: &mut UiApp| {
        let until = WallInstant::now() + Duration::from_millis(1500);
        while WallInstant::now() < until {
            pump(aqua, app, id, &window);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    settle(&mut aqua, &mut app);
    let warm_private = sample().unwrap().private_bytes;
    let warm_heap = live_bytes();
    let warm_vram = video_memory(high_performance)
        .map(|memory| memory.local_usage)
        .unwrap_or(0);
    let input_ms: Vec<f64> = (0..STEADY_INPUTS)
        .map(|index| wheel(&mut aqua, &mut app, index))
        .collect();
    settle(&mut aqua, &mut app);
    let private = sample().unwrap().private_bytes;
    let heap = live_bytes();
    let vram = video_memory(high_performance)
        .map(|memory| memory.local_usage)
        .unwrap_or(0);
    Some(Outcome {
        private: private.saturating_sub(baseline),
        heap,
        vram,
        growth: private.saturating_sub(warm_private),
        heap_growth: heap.saturating_sub(warm_heap),
        vram_growth: vram.saturating_sub(warm_vram),
        presented: aqua.presented_frames(id).unwrap_or(0),
        failures: stats.failures.load(Ordering::Relaxed) - startup_failures,
        startup_failures,
        recoveries: stats.recoveries.load(Ordering::Relaxed),
        input_ms,
    })
}

fn report(label: &str, outcome: &Outcome) -> Distribution {
    let distribution = Distribution::from_samples(&outcome.input_ms).unwrap();
    let janky = jank(&outcome.input_ms, 16.667);
    eprintln!(
        "host window {label}: private +{:.2} MiB (steady +{:.2}), heap {:.2} MiB (+{:.2}), vram {:.2} MiB (+{:.2}), presented {}, failures {} (startup {}), recoveries {}",
        outcome.private as f64 / MIB as f64,
        outcome.growth as f64 / MIB as f64,
        outcome.heap as f64 / MIB as f64,
        outcome.heap_growth as f64 / MIB as f64,
        outcome.vram as f64 / MIB as f64,
        outcome.vram_growth as f64 / MIB as f64,
        outcome.presented,
        outcome.failures,
        outcome.startup_failures,
        outcome.recoveries,
    );
    eprintln!(
        "  input to scene ms {distribution}; over 16.7 ms: {} of {}, longest run {}",
        janky.over_budget, janky.frames, janky.longest_run
    );
    distribution
}

fn steady(label: &str, outcome: &Outcome) {
    assert!(outcome.presented > 0, "{label} presented nothing");
    assert_eq!(outcome.failures, 0, "{label} failed frames");
    assert_eq!(outcome.recoveries, 0, "{label} lost the device");
    assert!(
        outcome.growth <= MIB,
        "{label}: private bytes grew {} over steady state",
        outcome.growth
    );
    assert!(
        outcome.heap_growth <= 256 * 1024,
        "{label}: heap grew {}",
        outcome.heap_growth
    );
    assert!(
        outcome.vram_growth <= MIB,
        "{label}: video memory grew {}",
        outcome.vram_growth
    );
}

fn gate(label: &str, outcome: &Outcome, budget: MemoryBudget) {
    let distribution = report(label, outcome);
    steady(label, outcome);
    let breaches = budget.check(&Measured {
        private_total: outcome.private,
        framework_heap: outcome.heap,
        steady_growth: outcome.growth,
        vram_total: outcome.vram,
    });
    assert!(
        breaches.is_empty(),
        "{label}: {}",
        breaches
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    );
    if cfg!(not(debug_assertions)) {
        assert!(
            distribution.p99 <= RELEASE_INPUT_P99_MS,
            "{label}: input-to-scene p99 {:.3} ms over {RELEASE_INPUT_P99_MS} ms",
            distribution.p99
        );
        assert_eq!(
            jank(&outcome.input_ms, 16.667).over_budget,
            0,
            "{label}: inputs over 16.7 ms"
        );
    }
}

#[test]
fn the_editor_workload_through_the_full_host_stays_inside_the_windowed_budget() {
    if let Some(outcome) = run(AdapterPreference::HighPerformance, true) {
        gate("high performance", &outcome, HOST_WINDOWED);
    }
}

#[test]
fn the_editor_workload_through_the_full_host_does_not_grow_on_warp() {
    let outcome = run(AdapterPreference::Warp, false).expect("WARP must present");
    report("warp", &outcome);
    steady("warp", &outcome);
}

fn hardware_adapters() -> Vec<AdapterInfo> {
    let factory = create_factory(false).expect("factory");
    list_adapters(&factory)
        .into_iter()
        .filter(|info| info.kind == Adapter::Hardware)
        .collect()
}

fn drives_monitor(info: &AdapterInfo, window: &Window) -> bool {
    let factory = create_factory(false).expect("factory");
    let luid = LUID {
        LowPart: info.luid as u32,
        HighPart: (info.luid >> 32) as i32,
    };
    let Ok(adapter) = (unsafe { factory.EnumAdapterByLuid::<IDXGIAdapter1>(luid) }) else {
        return false;
    };
    let monitor = unsafe { MonitorFromWindow(HWND(window.0 .0), MONITOR_DEFAULTTOPRIMARY) };
    (0..)
        .map_while(|index| unsafe { adapter.EnumOutputs(index) }.ok())
        .filter_map(|output: IDXGIOutput| unsafe { output.GetDesc() }.ok())
        .any(|desc| desc.Monitor == monitor)
}

#[test]
fn every_hardware_adapter_presents_the_full_host_native_or_across_adapters() {
    let adapters = hardware_adapters();
    assert!(!adapters.is_empty(), "no hardware adapter on this machine");
    let probe = Window::open(Extent {
        width: 320,
        height: 200,
    });
    let driven: Vec<(AdapterInfo, bool)> = adapters
        .into_iter()
        .map(|info| {
            let native = drives_monitor(&info, &probe);
            (info, native)
        })
        .collect();
    drop(probe);
    let mut crossed = false;
    for (info, native) in &driven {
        let Some(outcome) = run(AdapterPreference::Luid(info.luid), true) else {
            panic!("{} did not present", info.name);
        };
        let label = format!(
            "{} ({})",
            info.name,
            if *native {
                "drives the monitor"
            } else {
                "cross-adapter"
            }
        );
        report(&label, &outcome);
        steady(&label, &outcome);
        crossed |= !native;
    }
    if driven.len() < 2 {
        eprintln!(
            "SKIP-NO-SECOND-ADAPTER only {} is enabled: cross-adapter present is untested",
            driven[0].0.name
        );
    }
    assert!(
        driven.len() < 2 || crossed,
        "a second adapter exists but none presented across adapters"
    );
}
