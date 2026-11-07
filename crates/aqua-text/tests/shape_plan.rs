#![cfg(windows)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use aqua_text::{FontManager, LineMode, TextLayout, TextStyle, WrapMode};

struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(size, Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

static SERIAL: Mutex<()> = Mutex::new(());

#[global_allocator]
static COUNTING: Counting = Counting;

fn laid_positions(fonts: &mut FontManager, text: &str) -> Vec<(u32, u32)> {
    let mut layout = TextLayout::new();
    layout.set_line_mode(LineMode::Multi);
    layout.set_wrap(WrapMode::Word);
    layout.set_max_width(Some(400.0));
    layout.set_text(text, &TextStyle::default());
    let laid = layout.layout(fonts);
    laid.lines
        .iter()
        .flat_map(|line| line.glyphs.iter())
        .map(|glyph| (glyph.x.to_bits(), glyph.y.to_bits()))
        .collect()
}

fn paragraph(index: usize) -> String {
    format!(
        "Paragraph {index} shapes fresh words like efficient office staff differ {}",
        index * 7919
    )
}

#[test]
fn reused_shape_plans_leave_glyph_positions_unchanged() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut warm = FontManager::new();
    laid_positions(&mut warm, &paragraph(0));
    let mut cold = FontManager::new();
    for index in 1..8 {
        let text = paragraph(index);
        assert_eq!(
            laid_positions(&mut warm, &text),
            laid_positions(&mut cold, &text)
        );
    }
}

#[test]
fn cold_shaping_allocations_stay_bounded_per_run() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut fonts = FontManager::new();
    laid_positions(&mut fonts, &paragraph(0));
    let texts: Vec<String> = (1..=38).map(paragraph).collect();
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    for text in &texts {
        laid_positions(&mut fonts, text);
    }
    let allocations = (ALLOCATIONS.load(Ordering::Relaxed) - before) / texts.len();
    assert!(allocations < 120, "{allocations} allocations per run");
}

fn styled_cost(fonts: &mut FontManager, style: &TextStyle, text: &str) -> (usize, usize) {
    let before = (
        ALLOCATIONS.load(Ordering::Relaxed),
        BYTES.load(Ordering::Relaxed),
    );
    let mut layout = TextLayout::new();
    layout.set_line_mode(LineMode::Multi);
    layout.set_wrap(WrapMode::Word);
    layout.set_max_width(Some(400.0));
    layout.set_text(text, style);
    layout.layout(fonts);
    (
        ALLOCATIONS.load(Ordering::Relaxed) - before.0,
        BYTES.load(Ordering::Relaxed) - before.1,
    )
}

#[test]
fn prewarmed_entries_pay_no_plan_cost_on_first_layout() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let style = TextStyle::default();
    let mut cold = FontManager::new();
    let cold_first = styled_cost(&mut cold, &style, "Hello world fn main() { 42 }");
    let mut warm = FontManager::new();
    for job in warm.prewarm_jobs(std::slice::from_ref(&style)) {
        if let Some(entry) = job.build() {
            warm.install_prewarmed(entry);
        }
    }
    let warm_first = styled_cost(&mut warm, &style, "Hello world fn main() { 42 }");
    assert!(warm_first.0 < cold_first.0);
}

#[test]
fn single_paragraph_reflow_stays_within_its_allocation_budget() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_line_mode(LineMode::Single);
    layout.set_wrap(WrapMode::None);
    layout.set_max_width(Some(400.0));
    layout.set_text(&paragraph(1).repeat(64), &TextStyle::default());
    layout.layout(&mut fonts);
    layout.layout(&mut fonts);
    let before_layout = layout.layout(&mut fonts);
    let before = BYTES.load(Ordering::Relaxed);
    layout.set_max_width(Some(420.0));
    let after_layout = layout.layout(&mut fonts);
    let allocated = BYTES.load(Ordering::Relaxed) - before;
    let glyphs = after_layout.lines[0].glyphs.len();
    let budget = glyphs * std::mem::size_of::<aqua_text::layout::LaidGlyph>() * 5 / 2;
    eprintln!("single paragraph glyphs={glyphs} allocated={allocated} budget={budget}");
    assert!(allocated < budget, "{allocated} bytes for {glyphs} glyphs");
    assert_eq!(before_layout.size, after_layout.size);
    assert_eq!(
        before_layout.first_baseline_y,
        after_layout.first_baseline_y
    );
    let positions = |laid: &aqua_text::layout::LaidOut| {
        laid.lines[0]
            .glyphs
            .iter()
            .map(|glyph| {
                (
                    glyph.glyph_id,
                    glyph.src_byte_start,
                    glyph.src_byte_end,
                    glyph.x.to_bits(),
                    glyph.y.to_bits(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(positions(&before_layout), positions(&after_layout));
}
