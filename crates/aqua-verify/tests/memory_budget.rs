#![cfg(windows)]

use std::time::Instant;

use aqua_gpu_d3d12::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat};
use aqua_gpu_ref::conformance::assets::standard_uploads;
use aqua_gpu_ref::conformance::cases::hex;
use aqua_graph::{Extent, plan_paint};
use aqua_protocol::{FrameInputs, GpuBackend, SurfaceTarget};
use aqua_verify::budget::{MIB, Measured, MemoryBudget, RENDERER_OFFSCREEN};
use aqua_verify::heap::{CountingAllocator, live_bytes};
use aqua_verify::process::{sample, video_memory};
use aqua_verify::stats::{Distribution, jank};
use aqua_verify::workload::editor_workload;

#[path = "support/exclusive.rs"]
mod exclusive;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const SURFACE: Extent = Extent {
    width: 2560,
    height: 1440,
};
const WARMUP_FRAMES: usize = 60;
const STEADY_FRAMES: usize = 600;

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

const PRIME_VARIABLE: &str = "AQUA_MEMORY_BUDGET_PRIME";

fn prime_driver_cache(preference: AdapterPreference) {
    let mut backend = OffscreenBackend::with_format(preference, SurfaceFormat::Bgra8Unorm)
        .unwrap_or_else(|error| panic!("no adapter {preference:?}: {error}"));
    let surface = backend
        .create_surface(SurfaceTarget::Offscreen(SURFACE))
        .unwrap();
    backend.upload(standard_uploads()).unwrap();
    let plan = plan_paint(SURFACE, hex("#000000", 1.0), &editor_workload(SURFACE)).unwrap();
    backend.execute(&plan, &FrameInputs { surface }).unwrap();
}

fn prime_in_a_child_process(test: &str) {
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([test, "--exact", "--test-threads=1"])
        .env(PRIME_VARIABLE, "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "driver cache priming run failed");
}

fn run(
    test: &str,
    preference: AdapterPreference,
    high_performance: bool,
    budget: Option<MemoryBudget>,
    primed: bool,
) {
    exclusive::hold_exclusive();
    if std::env::var_os(PRIME_VARIABLE).is_some() {
        prime_driver_cache(preference);
        return;
    }
    if primed {
        prime_in_a_child_process(test);
    }
    let mut stages = vec![stage("process", high_performance)];
    let mut backend = match OffscreenBackend::with_format(preference, SurfaceFormat::Bgra8Unorm) {
        Ok(backend) => backend,
        Err(error) => {
            panic!("no adapter {preference:?}: {error}");
        }
    };
    stages.push(stage("device and renderer", high_performance));
    let surface = backend
        .create_surface(SurfaceTarget::Offscreen(SURFACE))
        .unwrap();
    backend.upload(standard_uploads()).unwrap();
    stages.push(stage("surface and uploads", high_performance));
    let plan = plan_paint(SURFACE, hex("#000000", 1.0), &editor_workload(SURFACE)).unwrap();
    let inputs = FrameInputs { surface };
    let mut gpu_ms: Vec<f64> = Vec::new();
    let mut run_frames = |backend: &mut OffscreenBackend, count: usize| -> Vec<f64> {
        (0..count)
            .map(|_| {
                let started = Instant::now();
                backend.execute(&plan, &inputs).unwrap();
                let elapsed = started.elapsed().as_secs_f64() * 1000.0;
                if let Some(gpu) = backend.gpu_time() {
                    gpu_ms.push(gpu.as_secs_f64() * 1000.0);
                }
                elapsed
            })
            .collect()
    };
    run_frames(&mut backend, 1);
    stages.push(stage("first frame", high_performance));
    run_frames(&mut backend, WARMUP_FRAMES);
    stages.push(stage("after warmup", high_performance));
    let steady = run_frames(&mut backend, STEADY_FRAMES);
    stages.push(stage("after steady state", high_performance));

    let transient = backend.transient_bytes();
    eprintln!(
        "memory {preference:?} ({} passes, {} instances)",
        plan.passes.len(),
        plan.instance_count()
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

    match Distribution::from_samples(&gpu_ms) {
        Some(gpu) => eprintln!(
            "  frame gpu ms {gpu}; transient heap {:.2} MiB",
            transient as f64 / MIB as f64
        ),
        None => eprintln!(
            "  frame gpu ms unavailable; transient heap {:.2} MiB",
            transient as f64 / MIB as f64
        ),
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
fn hardware_adapter_steady_state_does_not_grow() {
    run(
        "hardware_adapter_steady_state_does_not_grow",
        AdapterPreference::HighPerformance,
        true,
        Some(RENDERER_OFFSCREEN),
        true,
    );
}

#[test]
fn warp_steady_state_does_not_grow() {
    run(
        "warp_steady_state_does_not_grow",
        AdapterPreference::Warp,
        false,
        None,
        true,
    );
}

#[test]
fn hardware_adapter_unprimed_first_run_stays_in_budget() {
    run(
        "hardware_adapter_unprimed_first_run_stays_in_budget",
        AdapterPreference::HighPerformance,
        true,
        Some(RENDERER_OFFSCREEN),
        false,
    );
}
