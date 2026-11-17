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
            mask: None,
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

#[test]
fn hidden_surface_settle_preserves_the_other_window() {
    use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, NativeSurface, SurfaceTarget};
    use aqua_shell_win32::{Window, WindowOptions};

    for preference in [AdapterPreference::Warp, AdapterPreference::HighPerformance] {
        let mut backend = backend(preference);
        let windows = std::array::from_fn::<_, 2, _>(|_| {
            Window::create(&WindowOptions {
                visible: false,
                logical_size: (240.0, 160.0),
                ..Default::default()
            })
            .unwrap()
        });
        let surfaces = windows.map(|window| {
            let (width, height) = window.inner_size();
            let extent = Extent { width, height };
            let id = backend
                .create_surface(SurfaceTarget::Native(NativeSurface {
                    window: window.id(),
                    hwnd: window.raw_hwnd() as u64,
                    extent,
                }))
                .unwrap();
            (id, extent)
        });
        let exact = swapchain_bytes(&backend);
        let mut extra = [0; 2];
        for (index, (id, extent)) in surfaces.iter().enumerate() {
            let before = swapchain_bytes(&backend);
            backend
                .resize_surface(
                    *id,
                    Extent {
                        width: extent.width + 80,
                        height: extent.height + 80,
                    },
                )
                .unwrap();
            backend.resize_surface(*id, *extent).unwrap();
            extra[index] = swapchain_bytes(&backend) - before;
            assert!(extra[index] > 0);
        }
        let (first, first_extent) = surfaces[0];
        let (second, second_extent) = surfaces[1];
        backend
            .execute(&blurred(second_extent), &FrameInputs::new(second))
            .unwrap();
        let before = backend.read_surface(second).unwrap();
        assert!(backend.settle_surface(first).unwrap());
        assert_eq!(swapchain_bytes(&backend), exact + extra[1]);
        assert!(!backend.settle_surface(first).unwrap());
        let after = backend.read_surface(second).unwrap();
        assert_eq!(before.rgba8, after.rgba8);
        matches_reference(
            &mut backend,
            first,
            &blurred(first_extent),
            "settled-hidden",
        );
        assert!(backend.settle_surface(second).unwrap());
        assert_eq!(swapchain_bytes(&backend), exact);
        matches_reference(
            &mut backend,
            second,
            &blurred(second_extent),
            "other-settled-hidden",
        );
        for (id, _) in surfaces {
            backend.destroy_surface(id).unwrap();
        }
        for window in windows {
            window.destroy();
        }
    }
}
