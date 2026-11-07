use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::hint::black_box;
use std::rc::Rc;
use std::time::Instant;

use aqua_core::Oklch;
use aqua_ui::element::{lower_specs, ui, Element};
use aqua_ui::style::{px, Style};

#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    allocations: usize,
    reallocations: usize,
    allocated_bytes: usize,
    freed_bytes: usize,
    moved_reallocations: usize,
    moved_capacity_bytes: usize,
    largest_allocation: usize,
}

thread_local! {
    static COUNTS: Cell<Option<Counts>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn record(update: impl FnOnce(&mut Counts)) {
    let _ = COUNTS.try_with(|slot| {
        if let Some(mut counts) = slot.get() {
            update(&mut counts);
            slot.set(Some(counts));
        }
    });
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let result = unsafe { System.alloc(layout) };
        if !result.is_null() {
            record(|counts| {
                counts.allocations += 1;
                counts.allocated_bytes += layout.size();
                counts.largest_allocation = counts.largest_allocation.max(layout.size());
            });
        }
        result
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let result = unsafe { System.alloc_zeroed(layout) };
        if !result.is_null() {
            record(|counts| {
                counts.allocations += 1;
                counts.allocated_bytes += layout.size();
                counts.largest_allocation = counts.largest_allocation.max(layout.size());
            });
        }
        result
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(ptr, layout, size) };
        if !result.is_null() {
            record(|counts| {
                counts.reallocations += 1;
                counts.allocated_bytes += size.saturating_sub(layout.size());
                counts.largest_allocation = counts.largest_allocation.max(size);
                if result != ptr {
                    counts.moved_reallocations += 1;
                    counts.moved_capacity_bytes += layout.size().min(size);
                }
            });
        }
        result
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record(|counts| counts.freed_bytes += layout.size());
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn count<T>(build: impl FnOnce() -> T) -> (T, Counts) {
    COUNTS.with(|slot| slot.set(Some(Counts::default())));
    let result = build();
    let counts = COUNTS.with(|slot| slot.replace(None).unwrap());
    (result, counts)
}

#[test]
fn bounded_lazy_children_preserve_order_and_iterator_effects() {
    let visited = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&visited);
    let element = ui::Box::new()
        .child(ui::Box::new().build().with_key(99u32))
        .children((0..65u32).scan(0, move |sum, value| {
            observed.borrow_mut().push(value);
            *sum += value;
            Some(ui::Box::new().build().with_key(*sum))
        }))
        .child(ui::Box::new().build().with_key(100u32))
        .build();
    let spec = lower_specs(&element).unwrap().pop().unwrap();
    let expected = std::iter::once(99u32)
        .chain((0..65u32).scan(0, |sum, value| {
            *sum += value;
            Some(*sum)
        }))
        .chain(std::iter::once(100u32))
        .map(|value| Some(aqua_ui::tree::hash_key(aqua_ui::element::Key::U32(value))))
        .collect::<Vec<_>>();
    assert_eq!(
        spec.children
            .iter()
            .map(|child| child.key)
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(*visited.borrow(), (0..65u32).collect::<Vec<_>>());
}

#[test]
fn children_support_early_end_and_appending_more_values() {
    let visited = Cell::new(0);
    let element = ui::Box::new()
        .children((0..100u32).scan((), |_, value| {
            visited.set(visited.get() + 1);
            (value < 7).then(|| ui::Box::new().build().with_key(value))
        }))
        .children([ui::Box::new().build().with_key(50u32)])
        .build();
    let spec = lower_specs(&element).unwrap().pop().unwrap();
    let expected = (0..7u32)
        .chain(std::iter::once(50u32))
        .map(|value| Some(aqua_ui::tree::hash_key(aqua_ui::element::Key::U32(value))))
        .collect::<Vec<_>>();
    assert_eq!(
        spec.children
            .iter()
            .map(|child| child.key)
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(visited.get(), 8);
}

#[test]
fn empty_filtered_children_do_not_allocate_a_child_buffer() {
    let builder = ui::Box::new();
    let (builder, counts) =
        count(|| builder.children((0..100_000).filter_map(|_| None::<Element>)));
    assert_eq!(counts.allocations, 0);
    assert_eq!(counts.reallocations, 0);
    assert!(lower_specs(&builder.build()).unwrap()[0]
        .children
        .is_empty());
}

const PAGE: Style = Style::new().width(px(1280.0)).height(px(800.0)).column();
const ROW: Style = Style::new()
    .width(px(1280.0))
    .height(px(1.0))
    .row()
    .flex_shrink(0.0);
const CELL: Style = Style::new().width(px(1.0)).height(px(1.0)).flex_shrink(0.0);

fn original_declarations(frame: u32, built: &Cell<usize>, rows: &Cell<usize>) -> Element {
    ui::Box::new()
        .style(PAGE)
        .children((0..800u32).map(|y| {
            rows.set(rows.get() + 1);
            ui::Box::new()
                .style(ROW)
                .children((0..1280u32).scan([0u8; 3], move |previous, x| {
                    built.set(built.get() + 1);
                    let red = (x.wrapping_pow(2).wrapping_add(frame.wrapping_mul(3)) & 255) as u8;
                    let green = (y.wrapping_pow(3).wrapping_add(frame.wrapping_mul(5)) & 255) as u8;
                    let blue = (x
                        .wrapping_mul(y)
                        .wrapping_pow(2)
                        .wrapping_add(frame.wrapping_mul(7))
                        & 255) as u8;
                    *previous = [
                        ((u16::from(red) + u16::from(previous[0])) / 2) as u8,
                        green,
                        blue ^ (previous[0] >> 3),
                    ];
                    Some(
                        ui::Box::new()
                            .style(CELL.background(Oklch::from_srgb8(
                                previous[0],
                                previous[1],
                                previous[2],
                                1.0,
                            )))
                            .build(),
                    )
                }))
                .build()
        }))
        .build()
}

#[test]
#[ignore]
fn pixel_storm_original_construction_report() {
    let built = Cell::new(0);
    let rows = Cell::new(0);
    drop(original_declarations(0, &built, &rows));
    for frame in 1..=5 {
        built.set(0);
        rows.set(0);
        let begin = Instant::now();
        let (element, counts) = count(|| original_declarations(frame, &built, &rows));
        let elapsed = begin.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(rows.get(), 800);
        assert_eq!(built.get(), 1_024_000);
        black_box(&element);
        println!("original-construction frame={frame} ms={elapsed:.3} element_bytes={} allocations={} reallocations={} allocated_bytes={} freed_bytes={} moved_reallocations={} moved_capacity_bytes={} largest_allocation={}", std::mem::size_of::<Element>(), counts.allocations, counts.reallocations, counts.allocated_bytes, counts.freed_bytes, counts.moved_reallocations, counts.moved_capacity_bytes, counts.largest_allocation);
        drop(element);
    }
}
