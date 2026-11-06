use aqua_core::{geometry::Rect, Oklch};
use aqua_gpu_ref::{conformance, RefBackend};
use aqua_graph::{plan_paint, Extent, PaintCommand, Quad};
use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, SurfaceTarget};

fn pixel(pixels: &aqua_protocol::SurfacePixels, x: usize, y: usize) -> [u8; 4] {
    let index = (y * pixels.extent.width as usize + x) * 4;
    pixels.rgba8[index..index + 4].try_into().unwrap()
}

#[test]
fn group_opacity_composites_isolated_layer_not_each_child() {
    let extent = Extent {
        width: 32,
        height: 32,
    };
    let quad = |x: f32, color: &str| {
        PaintCommand::Quad(Quad {
            bounds: Rect {
                x,
                y: 0.0,
                w: 20.0,
                h: 32.0,
            },
            radius: 0.0,
            border_width: 0.0,
            fill: Oklch::from_hex(color).unwrap(),
            border_color: Oklch::TRANSPARENT,
        })
    };
    let plan = plan_paint(
        extent,
        Oklch::TRANSPARENT,
        &[
            PaintCommand::PushOpacity(0.5),
            quad(0.0, "#ff0000"),
            quad(8.0, "#0000ff"),
            PaintCommand::PopOpacity,
        ],
    )
    .unwrap();
    let mut backend = RefBackend::new();
    let surface = backend
        .create_surface(SurfaceTarget::Offscreen(extent))
        .unwrap();
    backend.execute(&plan, &FrameInputs { surface }).unwrap();
    let pixels = backend.read_surface(surface).unwrap();
    let overlap = pixel(&pixels, 12, 16);
    assert_eq!(overlap[0], 0);
    assert_eq!(overlap[3], 128);
    assert!(overlap[2] > 100);
}

#[test]
fn reference_matches_itself_on_every_case() {
    let mut a = RefBackend::new();
    let mut b = RefBackend::new();
    for case in conformance::cases() {
        let comparison =
            conformance::run_case(&mut a, &mut b, &case, conformance::Tolerance::default())
                .unwrap();
        assert!(comparison.passed(), "{}", case.name);
        assert_eq!(comparison.interior_max, 0);
    }
}
