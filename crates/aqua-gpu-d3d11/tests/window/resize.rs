use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_gpu_d3d11::hal::selection::AdapterPreference;
use aqua_graph::{plan_paint, BackdropBlur, Extent, FramePlan, PaintCommand, Quad};

use crate::backend::{backend, frame, matches_reference, open};
use crate::test_window::TestWindow;

const STEPS: [(i32, i32); 7] = [
    (540, 350),
    (580, 372),
    (520, 340),
    (610, 400),
    (700, 420),
    (690, 410),
    (520, 340),
];

fn blurred(extent: Extent) -> FramePlan {
    let width = extent.width as f32;
    let height = extent.height as f32;
    let quad = |x: f32, y: f32, w: f32, h: f32, hex: &str| {
        PaintCommand::Quad(Quad {
            bounds: Rect { x, y, w, h },
            radius: 0.0,
            border_width: 0.0,
            fill: Oklch::from_hex(hex).unwrap(),
            border_color: Oklch::from_hex("#000000").unwrap(),
        })
    };
    let commands = [
        quad(0.0, 0.0, width * 0.5, height, "#f4f4f5"),
        quad(width * 0.5, 0.0, width * 0.5, height, "#1d4ed8"),
        quad(width - 40.0, height - 40.0, 40.0, 40.0, "#ef4444"),
        PaintCommand::BackdropBlur(BackdropBlur {
            bounds: Rect {
                x: width * 0.25,
                y: height * 0.25,
                w: width * 0.75,
                h: height * 0.75,
            },
            radius: 16.0,
            corner_radius: 12.0,
        }),
    ];
    plan_paint(extent, Oklch::from_hex("#20242c").unwrap(), &commands).unwrap()
}

fn swapchain_bytes(backend: &aqua_gpu_d3d11::window::WindowBackend) -> u64 {
    backend.memory_breakdown().unwrap().swapchain_bytes
}

fn stepped_resize(preference: AdapterPreference, label: &str) {
    let mut backend = backend(preference);
    let window = TestWindow::open(520, 340);
    let (id, extent) = open(&mut backend, &window);
    frame(&mut backend, &window, id, &blurred(extent));
    let exact_start = swapchain_bytes(&backend);
    let mut peak = exact_start;
    for (width, height) in STEPS {
        window.resize(width, height);
        let (client_width, client_height) = window.client();
        let next = Extent {
            width: client_width,
            height: client_height,
        };
        aqua_protocol::GpuBackend::resize_surface(&mut backend, id, next).expect("resize");
        matches_reference(&mut backend, id, &blurred(next), label);
        frame(&mut backend, &window, id, &blurred(next));
        peak = peak.max(swapchain_bytes(&backend));
    }
    assert!(peak > exact_start);
    backend.settle_surfaces().expect("settle");
    assert_eq!(swapchain_bytes(&backend), exact_start);
    let (client_width, client_height) = window.client();
    let settled = Extent {
        width: client_width,
        height: client_height,
    };
    matches_reference(&mut backend, id, &blurred(settled), label);
    frame(&mut backend, &window, id, &blurred(settled));
}

#[test]
fn hardware_resizes_inside_the_swapchain_capacity() {
    stepped_resize(AdapterPreference::HighPerformance, "hardware-steps");
}

#[test]
fn warp_resizes_inside_the_swapchain_capacity() {
    stepped_resize(AdapterPreference::Warp, "warp-steps");
}
