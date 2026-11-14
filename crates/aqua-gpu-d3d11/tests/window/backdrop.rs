use aqua_core::{Oklch, Rect};
use aqua_gpu_d3d11::hal::selection::AdapterPreference;
use aqua_gpu_d3d11::scene::{OffscreenBackend, SurfaceFormat};
use aqua_graph::{plan_paint, BackdropBlur, Extent, FramePlan, PaintCommand, Quad};
use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, PresentHint, SurfaceTarget};

use crate::backend::{backend, open};
use crate::test_window::TestWindow;

fn glass(extent: Extent) -> Rect {
    Rect {
        x: extent.width as f32 * 0.25,
        y: extent.height as f32 * 0.25,
        w: extent.width as f32 * 0.4,
        h: extent.height as f32 * 0.5,
    }
}

fn patches(extent: Extent) -> [Rect; 2] {
    [
        Rect {
            x: 20.0,
            y: 20.0,
            w: 20.0,
            h: 20.0,
        },
        Rect {
            x: extent.width as f32 - 40.0,
            y: extent.height as f32 * 0.5,
            w: 20.0,
            h: 20.0,
        },
    ]
}

#[derive(Clone, Copy)]
enum Scenario {
    Direct,
    RoundedLayer,
    Alternating,
}

fn scene(extent: Extent, step: usize, scenario: Scenario) -> FramePlan {
    let quad = |bounds, fill, radius| {
        PaintCommand::Quad(Quad {
            bounds,
            fill,
            radius,
            border_width: 0.0,
            border_color: Oklch::TRANSPARENT,
        })
    };
    let dark = Oklch::from_hex("#101010").unwrap();
    let color = |step| Oklch::from_hex(if step % 2 == 0 { "#224466" } else { "#663322" }).unwrap();
    let (first_step, second_step) = match scenario {
        Scenario::Alternating => ((step + 1) / 2, step / 2),
        _ => (step, step),
    };
    let [first, second] = patches(extent);
    let mut commands = vec![
        quad(extent.bounds(), dark, 0.0),
        quad(first, color(first_step), 0.0),
        quad(second, color(second_step), 0.0),
    ];
    if matches!(scenario, Scenario::RoundedLayer) {
        commands.push(PaintCommand::PushRoundedClip {
            rect: glass(extent).inflate(30.0),
            radius: 16.0,
        });
    }
    commands.extend([
        PaintCommand::BackdropBlur(BackdropBlur {
            bounds: glass(extent),
            radius: 12.0,
            corner_radius: 12.0,
            mask: None,
        }),
        quad(
            glass(extent),
            Oklch::from_hex("#ffffff").unwrap().with_alpha(0.12),
            12.0,
        ),
    ]);
    if matches!(scenario, Scenario::RoundedLayer) {
        commands.push(PaintCommand::PopClip);
    }
    plan_paint(extent, dark, &commands).unwrap()
}

fn stable_backdrop(preference: AdapterPreference, scenario: Scenario) {
    let mut backend = backend(preference);
    let window = TestWindow::open(800, 600);
    let (surface, extent) = open(&mut backend, &window);
    let mut reference =
        OffscreenBackend::with_format(preference, SurfaceFormat::Bgra8Unorm).unwrap();
    let reference_surface = reference
        .create_surface(SurfaceTarget::Offscreen(extent))
        .unwrap();
    reference
        .execute(
            &scene(extent, 0, scenario),
            &FrameInputs::new(reference_surface),
        )
        .unwrap();
    let expected = reference.read_surface(reference_surface).unwrap().rgba8;
    let bounds = glass(extent);
    for step in 0..8 {
        backend.wait_for_frame_slot(surface).unwrap();
        let patches = patches(extent);
        let damage = match scenario {
            Scenario::Alternating if step > 0 => &patches[(step + 1) % 2..][..1],
            _ => &patches[..],
        };
        backend
            .execute_damaged(
                &scene(extent, step, scenario),
                &FrameInputs::new(surface),
                damage,
            )
            .unwrap();
        let actual = backend.read_surface(surface).unwrap().rgba8;
        let worst = actual
            .chunks_exact(4)
            .zip(expected.chunks_exact(4))
            .enumerate()
            .filter(|(index, _)| {
                let x = (index % extent.width as usize) as f32;
                let y = (index / extent.width as usize) as f32;
                x >= bounds.x + 20.0
                    && x < bounds.right() - 20.0
                    && y >= bounds.y + 20.0
                    && y < bounds.bottom() - 20.0
            })
            .flat_map(|(_, (left, right))| left.iter().zip(right).map(|(a, b)| a.abs_diff(*b)))
            .max()
            .unwrap();
        assert!(
            worst <= 2,
            "step {step}: unrelated repaint changed the glass by {worst}"
        );
        backend.present(surface, PresentHint::Vsync).unwrap();
        window.pump();
    }
}

#[test]
fn hardware_backdrop_stays_stable_when_damage_bounds_cross_it() {
    stable_backdrop(AdapterPreference::HighPerformance, Scenario::Direct);
}

#[test]
fn warp_backdrop_stays_stable_when_damage_bounds_cross_it() {
    stable_backdrop(AdapterPreference::Warp, Scenario::Direct);
}

#[test]
fn hardware_backdrop_stays_stable_across_retained_buffer_damage() {
    stable_backdrop(AdapterPreference::HighPerformance, Scenario::Alternating);
}

#[test]
fn warp_backdrop_stays_stable_inside_a_rounded_snapshot() {
    stable_backdrop(AdapterPreference::Warp, Scenario::RoundedLayer);
}
