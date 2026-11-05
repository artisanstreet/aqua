use aqua_core::Oklch;
use aqua_core::geometry::Rect;
use aqua_gpu_d3d11::hal::selection::AdapterPreference;
use aqua_gpu_d3d11::window::WindowBackend;
use aqua_gpu_d3d11::window::damage::{Damage, PixelRect};
use aqua_gpu_ref::RefBackend;
use aqua_graph::{Extent, FramePlan, PaintCommand, Quad, Shadow, ShadowCast, plan_paint};
use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, PresentHint, SurfaceId, SurfaceTarget};

use crate::backend::{backend, open};
use crate::test_window::TestWindow;

const PATCH: Rect = Rect {
    x: 40.0,
    y: 40.0,
    w: 96.0,
    h: 80.0,
};

fn quad(x: f32, y: f32, w: f32, h: f32, hex: &str, radius: f32) -> PaintCommand {
    PaintCommand::Quad(Quad {
        bounds: Rect { x, y, w, h },
        radius,
        border_width: 0.0,
        fill: Oklch::from_hex(hex).unwrap(),
        border_color: Oklch::from_hex("#000000").unwrap(),
    })
}

fn scene(extent: Extent, footer_x: f32, step: u32) -> FramePlan {
    let travel = (step % 8) as f32 * 6.0;
    let commands = [
        quad(8.0, 8.0, 160.0, 200.0, "#3b82f6", 0.0),
        quad(200.0, 60.0, 120.0, 120.0, "#f97316", 16.0),
        quad(
            footer_x,
            extent.height as f32 - 40.0,
            90.0,
            24.0,
            "#22c55e",
            0.0,
        ),
        PaintCommand::PushOpacity(0.8),
        PaintCommand::Shadow(Shadow {
            bounds: Rect {
                x: 52.0 + travel,
                y: 56.0,
                w: 28.0,
                h: 28.0,
            },
            radius: 6.0,
            offset: [0.0, 2.0],
            sigma: 3.0,
            spread: 0.0,
            color: Oklch::from_hex("#000000").unwrap().with_alpha(0.6),
            cast: ShadowCast::Whole,
        }),
        quad(52.0 + travel, 56.0, 28.0, 28.0, "#e11d48", 6.0),
        PaintCommand::PopOpacity,
    ];
    plan_paint(extent, Oklch::from_hex("#20242c").unwrap(), &commands).unwrap()
}

fn reference(plan: &FramePlan) -> Vec<u8> {
    let mut reference = RefBackend::new();
    let id = reference
        .create_surface(SurfaceTarget::Offscreen(plan.extent))
        .unwrap();
    reference.execute(plan, &FrameInputs::new(id)).unwrap();
    reference.read_surface(id).unwrap().rgba8
}

const LIMIT: u8 = 12;

struct Compared {
    over: usize,
    total: usize,
}

impl Compared {
    fn agrees(&self) -> bool {
        self.over * 300 <= self.total
    }
}

fn compare(
    left: &[u8],
    right: &[u8],
    keep: impl Fn(usize, usize) -> bool,
    width: usize,
) -> Compared {
    left.as_chunks::<4>()
        .0
        .iter()
        .zip(right.as_chunks::<4>().0.iter())
        .enumerate()
        .filter(|(index, _)| keep(index % width, index / width))
        .flat_map(|(_, (a, b))| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)))
        .fold(Compared { over: 0, total: 0 }, |acc, diff| Compared {
            over: acc.over + (diff > LIMIT) as usize,
            total: acc.total + 1,
        })
}

fn pixel_rect(damage: &Damage) -> PixelRect {
    damage
        .present_rects()
        .iter()
        .copied()
        .reduce(PixelRect::union)
        .expect("a bounded repaint region")
}

fn warmed(backend: &mut WindowBackend, window: &TestWindow, id: SurfaceId, extent: Extent) {
    for step in 0..6 {
        backend.wait_for_frame_slot(id).unwrap();
        backend
            .execute_damaged(&scene(extent, 220.0, step), &FrameInputs::new(id), &[PATCH])
            .unwrap();
        backend.present(id, PresentHint::Vsync).unwrap();
        window.pump();
    }
}

fn damage_limited(preference: AdapterPreference, label: &str) {
    let mut backend = backend(preference);
    let window = TestWindow::open(420, 300);
    let (id, extent) = open(&mut backend, &window);
    let width = extent.width as usize;

    backend.wait_for_frame_slot(id).unwrap();
    backend
        .execute_damaged(&scene(extent, 220.0, 0), &FrameInputs::new(id), &[])
        .unwrap();
    backend.present(id, PresentHint::Vsync).unwrap();
    assert_eq!(
        backend.stats().scissored_frames,
        0,
        "unknown buffers repaint in full"
    );

    let mut previous = None;
    for step in 1..16u32 {
        let plan = scene(extent, 220.0, step);
        backend.wait_for_frame_slot(id).unwrap();
        let region = backend.repaint_region(id, &[PATCH]).unwrap();
        backend
            .execute_damaged(&plan, &FrameInputs::new(id), &[PATCH])
            .unwrap();
        let actual = backend.read_surface(id).unwrap().rgba8;
        let expected = reference(&plan);
        let compared = compare(&expected, &actual, |_, _| true, width);
        assert!(
            compared.agrees(),
            "[{label}] step {step}: {} of {} values differ from the reference",
            compared.over,
            compared.total
        );
        previous = Some(region);
        backend.present(id, PresentHint::Vsync).unwrap();
        window.pump();
    }
    let stats = backend.stats();
    eprintln!("[{label}] {stats:?}");
    assert!(stats.scissored_frames >= 10, "{stats:?}");
    assert!(stats.skipped_pixels > 10 * (width as u64 * extent.height as u64) / 2);
    assert!(matches!(previous, Some(Damage::Rects(_))));
    assert!(
        stats.last_repaint_area * 4 < width as i64 * extent.height as i64,
        "repaint area {} of {}",
        stats.last_repaint_area,
        width * extent.height as usize
    );
}

#[test]
fn hardware_scissored_repaint_matches_the_full_reference_every_frame() {
    damage_limited(AdapterPreference::HighPerformance, "hardware");
}

#[test]
fn warp_scissored_repaint_matches_the_full_reference_every_frame() {
    damage_limited(AdapterPreference::Warp, "warp");
}

fn pixels_outside_the_region_are_not_touched(preference: AdapterPreference, label: &str) {
    let mut backend = backend(preference);
    let window = TestWindow::open(420, 300);
    let (id, extent) = open(&mut backend, &window);
    let width = extent.width as usize;
    warmed(&mut backend, &window, id, extent);

    let region = backend.repaint_region(id, &[PATCH]).unwrap();
    let bounds = pixel_rect(&region);
    let step = 6;
    let old = scene(extent, 220.0, step);
    let changed = scene(extent, 300.0, step);
    backend.wait_for_frame_slot(id).unwrap();
    backend
        .execute_damaged(&changed, &FrameInputs::new(id), &[PATCH])
        .unwrap();
    let actual = backend.read_surface(id).unwrap().rgba8;
    let before = reference(&old);
    let after = reference(&changed);

    let outside = |x: usize, y: usize| {
        (x as i32) < bounds.left
            || (x as i32) >= bounds.right
            || (y as i32) < bounds.top
            || (y as i32) >= bounds.bottom
    };
    let inside = |x: usize, y: usize| !outside(x, y);
    let moved = compare(&before, &after, outside, width);
    assert!(
        moved.over > 500,
        "[{label}] the probe must change pixels outside the region"
    );
    assert!(
        compare(&before, &actual, outside, width).agrees(),
        "[{label}] pixels outside the repaint region must keep the previous frame"
    );
    assert!(
        compare(&after, &actual, inside, width).agrees(),
        "[{label}] pixels inside the repaint region must show the new frame"
    );
    backend.present(id, PresentHint::Vsync).unwrap();
    window.pump();
}

#[test]
fn hardware_pixels_outside_the_repaint_region_are_skipped() {
    pixels_outside_the_region_are_not_touched(AdapterPreference::HighPerformance, "hardware");
}

#[test]
fn warp_pixels_outside_the_repaint_region_are_skipped() {
    pixels_outside_the_region_are_not_touched(AdapterPreference::Warp, "warp");
}

#[test]
fn a_resize_or_an_occluded_history_forces_a_full_repaint() {
    let mut backend = backend(AdapterPreference::Warp);
    let window = TestWindow::open(420, 300);
    let (id, extent) = open(&mut backend, &window);
    warmed(&mut backend, &window, id, extent);
    assert!(matches!(
        backend.repaint_region(id, &[PATCH]).unwrap(),
        Damage::Rects(_)
    ));
    window.resize(500, 340);
    let (width, height) = window.client();
    let next = Extent { width, height };
    backend.resize_surface(id, next).unwrap();
    assert_eq!(backend.repaint_region(id, &[PATCH]).unwrap(), Damage::Full);
    let before = backend.stats().scissored_frames;
    backend.wait_for_frame_slot(id).unwrap();
    backend
        .execute_damaged(&scene(next, 220.0, 0), &FrameInputs::new(id), &[PATCH])
        .unwrap();
    assert_eq!(backend.stats().scissored_frames, before);
    let actual = backend.read_surface(id).unwrap().rgba8;
    let expected = reference(&scene(next, 220.0, 0));
    assert!(compare(&expected, &actual, |_, _| true, width as usize).agrees());
}
