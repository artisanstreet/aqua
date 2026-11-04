use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

use aqua_core::{geometry::Rect, Oklch};
use aqua_graph::{plan_paint, validate, Extent, PaintCommand, Quad};

struct Counting;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

fn quad(x: f32) -> PaintCommand {
    PaintCommand::Quad(Quad {
        bounds: Rect {
            x,
            y: 0.0,
            w: 30.0,
            h: 30.0,
        },
        radius: 0.0,
        border_width: 0.0,
        fill: Oklch::WHITE,
        border_color: Oklch::WHITE,
    })
}

#[test]
fn validating_a_layered_plan_allocates_nothing() {
    let mut commands = Vec::new();
    for group in 0..80 {
        commands.push(PaintCommand::PushOpacity(0.5));
        commands.push(quad(group as f32));
        commands.push(quad(group as f32 + 10.0));
        commands.push(PaintCommand::PopOpacity);
    }
    let plan = plan_paint(
        Extent {
            width: 128,
            height: 128,
        },
        Oklch::TRANSPARENT,
        &commands,
    )
    .unwrap();
    assert!(plan.textures.len() > 2);
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let result = validate(&plan);
    let after = ALLOCATIONS.load(Ordering::Relaxed);
    assert_eq!(result, Ok(()));
    assert_eq!(after - before, 0);
}
