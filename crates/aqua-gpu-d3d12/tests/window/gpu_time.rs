use std::time::{Duration, Instant};

use aqua_core::Oklch;
use aqua_core::geometry::Rect;
use aqua_gpu_d3d12::hal::selection::AdapterPreference;
use aqua_gpu_d3d12::window::WindowBackend;
use aqua_graph::{Extent, FramePlan, PaintCommand, Quad, plan_paint};
use aqua_protocol::{FrameInputs, GpuBackend, NativeSurface, PresentHint, SurfaceTarget, WindowId};

use crate::backend::{backend, options};
use crate::test_window::TestWindow;

const RESIZE_STEPS: usize = 120;
const SCROLL_FRAMES: usize = 300;
const RESIZE_P99_BUDGET: Duration = Duration::from_micros(8_300);

fn editor_scene(extent: Extent, phase: f32) -> FramePlan {
    let width = extent.width as f32;
    let height = extent.height as f32;
    let quad = |x: f32, y: f32, w: f32, h: f32, hex: &str, radius: f32| {
        PaintCommand::Quad(Quad {
            bounds: Rect { x, y, w, h },
            radius,
            border_width: 1.0,
            fill: Oklch::from_hex(hex).unwrap(),
            border_color: Oklch::from_hex("#394150").unwrap(),
        })
    };
    let mut commands = Vec::new();
    let rows = (height / 36.0) as usize;
    for row in 0..rows {
        let y = (row as f32 * 36.0 - phase * 36.0).rem_euclid(height);
        commands.push(quad(
            260.0,
            y,
            width - 280.0,
            30.0,
            if row % 2 == 0 { "#252b36" } else { "#2c3340" },
            6.0,
        ));
    }
    commands.push(PaintCommand::PushContentBlur {
        bounds: Rect {
            x: 280.0,
            y: height - 160.0,
            w: width - 320.0,
            h: 120.0,
        },
        radius: 12.0,
    });
    commands.push(quad(
        280.0,
        height - 160.0,
        width - 320.0,
        120.0,
        "#3b82f6",
        14.0,
    ));
    commands.push(PaintCommand::PopContentBlur);
    plan_paint(extent, Oklch::from_hex("#181c24").unwrap(), &commands).unwrap()
}

fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    let index = ((sorted.len() as f64 * fraction) as usize).min(sorted.len() - 1);
    sorted[index]
}

fn summary(label: &str, samples: &mut [Duration]) -> Duration {
    samples.sort();
    let p99 = percentile(samples, 0.99);
    eprintln!(
        "[{label}] n={} p50 {:.3} ms p90 {:.3} ms p99 {:.3} ms max {:.3} ms",
        samples.len(),
        percentile(samples, 0.5).as_secs_f64() * 1000.0,
        percentile(samples, 0.9).as_secs_f64() * 1000.0,
        p99.as_secs_f64() * 1000.0,
        samples.last().unwrap().as_secs_f64() * 1000.0,
    );
    p99
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn startup(preference: AdapterPreference, label: &str, cache: Option<&[u8]>) -> Option<Vec<u8>> {
    let probe = Instant::now();
    let core = aqua_gpu_d3d12::hal::Core::new(options(preference).device).ok()?;
    let core_time = probe.elapsed();
    let probe = Instant::now();
    let mut renderer =
        aqua_gpu_d3d12::scene::SceneRenderer::with_pipeline_cache(core.device(), 2, cache).ok()?;
    let renderer_time = probe.elapsed();
    let probe = Instant::now();
    renderer
        .warm(&[
            windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R16G16B16A16_FLOAT,
            aqua_gpu_d3d12::hal::RENDER_FORMAT,
        ])
        .ok()?;
    let warm_time = probe.elapsed();
    eprintln!(
        "[{label}] core {:.1} ms, renderer {:.1} ms, warm {:.1} ms",
        ms(core_time),
        ms(renderer_time),
        ms(warm_time)
    );
    drop(renderer);
    drop(core);
    let begun = Instant::now();
    let mut backend = WindowBackend::with_pipeline_cache(options(preference), cache).ok()?;
    let device_ready = begun.elapsed();
    let window = TestWindow::open(1500, 900);
    let (width, height) = window.client();
    let extent = Extent { width, height };
    let id = backend
        .create_surface(SurfaceTarget::Native(NativeSurface {
            window: WindowId::new(1).unwrap(),
            hwnd: window.raw() as u64,
            extent,
        }))
        .unwrap();
    let surface_ready = begun.elapsed();
    backend.wait_for_frame_slot(id).unwrap();
    backend
        .execute(&editor_scene(extent, 0.0), &FrameInputs::new(id))
        .unwrap();
    let executed = begun.elapsed();
    backend.present(id, PresentHint::Vsync).unwrap();
    let presented = begun.elapsed();
    let stats = backend.pipeline_stats().unwrap();
    eprintln!(
        "[{label}] startup: device+pipelines {:.1} ms, surface {:.1} ms, first execute {:.1} ms, first present {:.1} ms; pipelines compiled {} loaded {} late {}",
        ms(device_ready),
        ms(surface_ready),
        ms(executed),
        ms(presented),
        stats.compiled,
        stats.loaded,
        stats.late
    );
    assert_eq!(backend.late_pipelines(), Vec::<String>::new());
    backend.pipeline_cache()
}

fn run(preference: AdapterPreference, label: &str) {
    let mut backend = backend(preference);
    let window = TestWindow::open(1500, 900);
    let (width, height) = window.client();
    let mut extent = Extent { width, height };
    let id = backend
        .create_surface(SurfaceTarget::Native(NativeSurface {
            window: WindowId::new(1).unwrap(),
            hwnd: window.raw() as u64,
            extent,
        }))
        .unwrap();
    for warm in 0..20 {
        backend.wait_for_frame_slot(id).unwrap();
        backend
            .execute(
                &editor_scene(extent, warm as f32 / 20.0),
                &FrameInputs::new(id),
            )
            .unwrap();
        backend.present(id, PresentHint::Vsync).unwrap();
        window.pump();
    }
    let mut scroll = Vec::new();
    let mut gpu = Vec::new();
    let mut presents = Vec::new();
    let mut waits = Vec::new();
    let mut passes: std::collections::BTreeMap<&'static str, Duration> = Default::default();
    for index in 0..SCROLL_FRAMES {
        let begun = Instant::now();
        backend.wait_for_frame_slot(id).unwrap();
        let waited = begun.elapsed();
        let work = Instant::now();
        backend
            .execute(
                &editor_scene(extent, index as f32 / 7.0),
                &FrameInputs::new(id),
            )
            .unwrap();
        backend.present(id, PresentHint::Vsync).unwrap();
        scroll.push(work.elapsed());
        presents.push(backend.phases().present);
        waits.push(waited);
        window.pump();
        if let Some(time) = backend.gpu_frame_time() {
            gpu.push(time);
        }
        for pass in backend.gpu_pass_times() {
            let slot = passes.entry(pass.label).or_default();
            *slot = (*slot).max(pass.duration);
        }
    }
    summary(&format!("{label}] [steady cpu submit+present"), &mut scroll);
    summary(&format!("{label}] [steady Present1 call"), &mut presents);
    summary(&format!("{label}] [steady frame latency wait"), &mut waits);
    if !gpu.is_empty() {
        summary(&format!("{label}] [steady gpu"), &mut gpu);
    }
    assert!(!gpu.is_empty() && passes.contains_key("quads"));
    for (name, worst) in &passes {
        eprintln!("[{label}] pass {name} worst {:.3} ms", ms(*worst));
    }

    let mut resize_total = Vec::new();
    let mut resize_only = Vec::new();
    let mut latency_waits = Vec::new();
    let mut created = 0;
    let mut sums = [Duration::ZERO; 6];
    for step in 0..RESIZE_STEPS {
        let target_width = 1100 + (step as i32 * 7) % 700;
        let target_height = 700 + (step as i32 * 5) % 400;
        window.resize(target_width, target_height);
        let (width, height) = window.client();
        let next = Extent { width, height };
        let begun = Instant::now();
        if next != extent {
            backend.resize_surface(id, next).unwrap();
            extent = next;
        }
        let resized = begun.elapsed();
        let wait_started = Instant::now();
        backend.wait_for_frame_slot(id).unwrap();
        let paced = wait_started.elapsed();
        backend
            .execute(
                &editor_scene(extent, step as f32 / 7.0),
                &FrameInputs::new(id),
            )
            .unwrap();
        backend.present(id, PresentHint::Vsync).unwrap();
        let phases = backend.phases();
        created += phases.reserve_created as usize;
        sums[0] += phases.slot_wait;
        sums[1] += phases.reserve;
        sums[2] += phases.record;
        sums[3] += phases.submit;
        sums[4] += phases.present;
        sums[5] += resized;
        resize_total.push(begun.elapsed() - paced);
        latency_waits.push(paced);
        resize_only.push(resized);
        if begun.elapsed() - paced > Duration::from_millis(4) {
            eprintln!(
                "[{label}] slow resize frame {:?}: resize {:.2} wait {:.2} reserve {:.2} record {:.2} submit {:.2} present {:.2} ms",
                extent,
                ms(resized),
                ms(phases.slot_wait),
                ms(phases.reserve),
                ms(phases.record),
                ms(phases.submit),
                ms(phases.present)
            );
        }
        window.pump();
    }
    eprintln!("[{label}] resize frames that created transients: {created}/{RESIZE_STEPS}");
    eprintln!(
        "[{label}] resize mean ms: slot_wait {:.3} reserve {:.3} record {:.3} submit {:.3} present {:.3} swapchain {:.3}",
        ms(sums[0]) / RESIZE_STEPS as f64,
        ms(sums[1]) / RESIZE_STEPS as f64,
        ms(sums[2]) / RESIZE_STEPS as f64,
        ms(sums[3]) / RESIZE_STEPS as f64,
        ms(sums[4]) / RESIZE_STEPS as f64,
        ms(sums[5]) / RESIZE_STEPS as f64,
    );
    summary(
        &format!("{label}] [resize swapchain only"),
        &mut resize_only,
    );
    summary(
        &format!("{label}] [resize frame latency wait"),
        &mut latency_waits,
    );
    let p99 = summary(&format!("{label}] [resize frame work"), &mut resize_total);
    assert_eq!(backend.late_pipelines(), Vec::<String>::new());
    if preference == AdapterPreference::HighPerformance {
        assert!(p99 <= RESIZE_P99_BUDGET, "resize p99 {p99:?}");
    }
}

#[test]
fn hardware_steady_and_resize() {
    run(AdapterPreference::HighPerformance, "hardware");
}

#[test]
fn startup_cold_and_cached() {
    crate::support::hold_software();
    let cache = startup(AdapterPreference::HighPerformance, "cold", None);
    startup(
        AdapterPreference::HighPerformance,
        "cached",
        cache.as_deref(),
    );
}

#[test]
fn startup_breakdown() {
    use aqua_gpu_d3d12::hal::{
        BudgetMonitor, CpuHeap, FrameRing, ShaderVisibleHeap, Timeline, create_factory,
    };
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::core::Interface;
    crate::support::hold_software();
    let mut lap = Instant::now();
    let mut note = |name: &str| {
        eprintln!("[breakdown] {name}: {:.2} ms", ms(lap.elapsed()));
        lap = Instant::now();
    };
    let factory = create_factory(false).unwrap();
    note("create factory");
    let adapter: IDXGIAdapter1 = unsafe {
        factory
            .EnumAdapterByGpuPreference(0, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE)
            .unwrap()
    };
    note("enumerate adapter");
    let mut device: Option<ID3D12Device> = None;
    unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device).unwrap() };
    let device = device.unwrap();
    note("D3D12CreateDevice");
    let mut second: Option<ID3D12Device> = None;
    unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut second).unwrap() };
    note("second D3D12CreateDevice on the same adapter");
    drop(second);
    let _features = aqua_gpu_d3d12::hal::detect(&device);
    note("feature detection");
    let _queue: ID3D12CommandQueue = unsafe {
        device
            .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                ..Default::default()
            })
            .unwrap()
    };
    note("command queue");
    let _timeline = Timeline::new(&device).unwrap();
    note("timeline");
    let _frames = FrameRing::new(&device, 2).unwrap();
    note("frame ring");
    let _rtv = CpuHeap::new(&device, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 64).unwrap();
    note("rtv heap");
    let _srv = ShaderVisibleHeap::new(&device, 65_536, 4_096).unwrap();
    note("srv heap bindless sized");
    let _srv_small = ShaderVisibleHeap::new(&device, 1_024, 2_048).unwrap();
    note("srv heap baseline sized");
    let _monitor = BudgetMonitor::new(&adapter.cast().unwrap()).unwrap();
    note("budget monitor");
}
