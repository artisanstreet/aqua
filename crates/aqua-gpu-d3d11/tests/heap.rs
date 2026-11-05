#![cfg(windows)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use aqua_core::Oklch;
use aqua_core::geometry::Rect;
use aqua_gpu_d3d11::hal::DeviceOptions;
use aqua_gpu_d3d11::scene::{AdapterPreference, OffscreenBackend, SurfaceFormat};
use aqua_gpu_d3d11::window::{WindowBackend, WindowOptions};
use aqua_gpu_ref::conformance::{Case, Session, cases};
use aqua_graph::{Extent, PaintCommand, Quad, plan_paint};
use aqua_protocol::{FrameInputs, GpuBackend, NativeSurface, PresentHint, SurfaceTarget, WindowId};

#[allow(dead_code)]
#[path = "hal/window.rs"]
mod test_window;

struct Counting;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static TRACING: AtomicBool = AtomicBool::new(false);
static SITES: Mutex<Vec<String>> = Mutex::new(Vec::new());

thread_local! {
    static INSIDE: Cell<bool> = const { Cell::new(false) };
}

fn record_site() {
    if !TRACING.load(Ordering::Relaxed) {
        return;
    }
    INSIDE.with(|inside| {
        if inside.replace(true) {
            return;
        }
        let trace = std::backtrace::Backtrace::force_capture().to_string();
        let frames: Vec<&str> = trace
            .lines()
            .filter(|line| {
                line.trim_start().starts_with(|c: char| c.is_ascii_digit())
                    && !line.contains("heap::")
                    && !line.contains("std::")
                    && !line.contains("core::")
                    && !line.contains("alloc::")
            })
            .take(10)
            .collect();
        SITES.lock().unwrap().push(frames.join(" <- "));
        inside.set(false);
    });
}
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        record_site();
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        record_site();
        BYTES.fetch_add(size as u64, Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

fn counted<T>(work: impl FnOnce() -> T) -> (T, u64, u64) {
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    let value = work();
    (
        value,
        ALLOCATIONS.load(Ordering::Relaxed) - allocations,
        BYTES.load(Ordering::Relaxed) - bytes,
    )
}

fn named(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("conformance case {name} is missing"))
}

const SETTLE_FRAMES: usize = 90;

fn settle(backend: &mut OffscreenBackend, case: &Case) -> aqua_protocol::SurfaceId {
    let mut session = Session::new(backend);
    session.render_case(case).unwrap();
    let surface = session.surface(case.plan.extent).unwrap();
    for _ in 0..SETTLE_FRAMES {
        session.render(&case.plan).unwrap();
    }
    surface
}

const CASES: [&str; 14] = [
    "isolated_group_opacity",
    "glyph_run_one_to_one",
    "image_unscaled",
    "backdrop_blur_large_radius",
    "content_blur_crossfade_radius",
    "rounded_clip_basic",
    "color_matrix_grayscale_over_stripes",
    "color_matrix_offsets_and_alpha",
    "custom_invert_full_amount",
    "custom_invert_half_amount_fractional_bounds",
    "compute_row_box_blur",
    "slug_rotated_and_skewed",
    "quad_rotated_rounded_bordered",
    "image_rotated_rounded",
];

fn traced(name: &str, frame: impl FnOnce()) {
    SITES.lock().unwrap().clear();
    TRACING.store(true, Ordering::Relaxed);
    frame();
    TRACING.store(false, Ordering::Relaxed);
    for site in SITES.lock().unwrap().iter() {
        eprintln!("{name} site: {site}");
    }
}

#[test]
fn steady_offscreen_frames_allocate_nothing() {
    let mut over: Vec<String> = Vec::new();
    for preference in [AdapterPreference::Warp, AdapterPreference::HighPerformance] {
        let mut backend = OffscreenBackend::with_format(preference, SurfaceFormat::Rgba16Float)
            .unwrap_or_else(|error| panic!("adapter {preference:?} unavailable: {error}"));
        for name in CASES {
            let case = named(name);
            let surface = settle(&mut backend, &case);
            let inputs = FrameInputs { surface };
            if std::env::var("AQUA_HEAP_TRACE").is_ok() {
                traced(name, || {
                    backend.execute(&case.plan, &inputs).unwrap();
                });
            }
            let mut worst = (0, 0);
            for _ in 0..16 {
                let (result, allocations, bytes) = counted(|| backend.execute(&case.plan, &inputs));
                result.unwrap();
                worst = (worst.0.max(allocations), worst.1.max(bytes));
            }
            eprintln!(
                "{preference:?} {name}: {} allocations, {} bytes per steady frame",
                worst.0, worst.1
            );
            if worst != (0, 0) {
                over.push(format!(
                    "{preference:?} {name}: {} allocations / {} bytes in a steady frame, the gate is 0",
                    worst.0, worst.1
                ));
            }
        }
    }
    assert!(over.is_empty(), "{}", over.join("\n"));
}

fn window_scene(extent: Extent) -> aqua_graph::FramePlan {
    let quad = |x: f32, y: f32, w: f32, h: f32, hex: &str, radius: f32| {
        PaintCommand::Quad(Quad {
            bounds: Rect { x, y, w, h },
            radius,
            border_width: 0.0,
            fill: Oklch::from_hex(hex).unwrap(),
            border_color: Oklch::from_hex("#000000").unwrap(),
        })
    };
    let commands = [
        quad(8.0, 8.0, 200.0, 120.0, "#3b82f6", 0.0),
        PaintCommand::PushContentBlur {
            bounds: Rect {
                x: 100.0,
                y: 60.0,
                w: 240.0,
                h: 160.0,
            },
            radius: 6.0,
        },
        quad(120.0, 80.0, 180.0, 100.0, "#f97316", 12.0),
        PaintCommand::PopContentBlur,
    ];
    plan_paint(extent, Oklch::from_hex("#20242c").unwrap(), &commands).unwrap()
}

#[test]
fn steady_window_frames_allocate_nothing() {
    let mut over: Vec<String> = Vec::new();
    for preference in [AdapterPreference::Warp, AdapterPreference::HighPerformance] {
        let mut backend = WindowBackend::new(WindowOptions {
            device: DeviceOptions::default().with_preference(preference),
            mirror_textures: false,
        })
        .unwrap_or_else(|error| panic!("adapter {preference:?} unavailable: {error}"));
        let window = test_window::TestWindow::open(480, 320);
        let (width, height) = window.client();
        let extent = Extent { width, height };
        let surface = backend
            .create_surface(SurfaceTarget::Native(NativeSurface {
                window: WindowId::new(1).unwrap(),
                hwnd: window.raw() as u64,
                extent,
            }))
            .unwrap();
        let plan = window_scene(extent);
        let inputs = FrameInputs { surface };
        let damage = [Rect {
            x: 120.0,
            y: 80.0,
            w: 60.0,
            h: 40.0,
        }];
        let mut frame = |damaged: bool| {
            backend.wait_for_frame_slot(surface).unwrap();
            match damaged {
                true => {
                    backend.execute_damaged(&plan, &inputs, &damage).unwrap();
                    backend
                        .present_with_damage(surface, PresentHint::Vsync, &damage)
                        .unwrap();
                }
                false => {
                    backend.execute(&plan, &inputs).unwrap();
                    backend.present(surface, PresentHint::Vsync).unwrap();
                }
            }
        };
        for index in 0..SETTLE_FRAMES {
            frame(index % 2 == 0);
            window.pump();
        }
        if std::env::var("AQUA_HEAP_TRACE").is_ok() {
            traced("window", || frame(true));
        }
        let mut worst = (0, 0);
        for index in 0..16 {
            let ((), allocations, bytes) = counted(|| frame(index % 2 == 0));
            worst = (worst.0.max(allocations), worst.1.max(bytes));
            window.pump();
        }
        eprintln!(
            "{preference:?} window: {} allocations, {} bytes per steady frame",
            worst.0, worst.1
        );
        if worst != (0, 0) {
            over.push(format!(
                "{preference:?} window: {} allocations / {} bytes in a steady frame, the gate is 0",
                worst.0, worst.1
            ));
        }
    }
    assert!(over.is_empty(), "{}", over.join("\n"));
}
