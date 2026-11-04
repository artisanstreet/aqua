use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

use aqua_core::{NodeId, Oklch};
use aqua_render::{Renderer, Scene};
use aqua_runtime::{FrameWork, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
use aqua_ui::style::{Axis, Style, px};
use aqua_ui::tree::NodeKind;

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    static BYTES: Cell<u64> = const { Cell::new(0) };
}

struct Counting;

fn count(size: usize) {
    let _ = ALLOCATIONS.try_with(|cell| cell.set(cell.get() + 1));
    let _ = BYTES.try_with(|cell| cell.set(cell.get() + size as u64));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[derive(Clone, Copy, Debug)]
struct Cost {
    micros: f64,
    allocations: u64,
    bytes: u64,
}

fn measure(work: impl FnOnce()) -> Cost {
    let allocations = ALLOCATIONS.with(Cell::get);
    let bytes = BYTES.with(Cell::get);
    let started = Instant::now();
    work();
    let micros = started.elapsed().as_secs_f64() * 1e6;
    Cost {
        micros,
        allocations: ALLOCATIONS.with(Cell::get) - allocations,
        bytes: BYTES.with(Cell::get) - bytes,
    }
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    let index = ((sorted.len() as f64 - 1.0) * fraction).round() as usize;
    sorted[index]
}

struct Summary {
    p50_us: f64,
    p99_us: f64,
    max_us: f64,
    allocations: u64,
    bytes: u64,
}

fn summarize(costs: &[Cost]) -> Summary {
    let mut times: Vec<f64> = costs.iter().map(|cost| cost.micros).collect();
    times.sort_by(f64::total_cmp);
    let count = costs.len() as u64;
    Summary {
        p50_us: percentile(&times, 0.5),
        p99_us: percentile(&times, 0.99),
        max_us: *times.last().unwrap(),
        allocations: costs.iter().map(|cost| cost.allocations).sum::<u64>() / count,
        bytes: costs.iter().map(|cost| cost.bytes).sum::<u64>() / count,
    }
}

const PARAGRAPH: &str = "Aqua keeps retained text, layout and scene state between frames so that an unchanged transcript costs nothing, while a streaming message only reshapes the run that changed.";

fn row_text(index: usize) -> String {
    let repeats = 1 + index % 4;
    format!("Message {index}. {}", PARAGRAPH.repeat(repeats))
}

fn background() -> Oklch {
    Oklch::from_hex("#1e1e2e").unwrap()
}

fn mount_transcript(runtime: &mut Runtime, rows: usize) {
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().axis(Axis::Column).width(px(760.0)))
                .children((0..rows).map(|index| {
                    UiBox::new()
                        .fill(background())
                        .radius(6.0)
                        .style(Style::new().width(px(760.0)))
                        .child(
                            UiText::builder()
                                .arg(row_text(index))
                                .wrap(true)
                                .style(Style::new().width(px(720.0)))
                                .build(),
                        )
                        .build()
                }))
                .build()
        })
        .unwrap();
}

fn text_nodes(runtime: &Runtime) -> Vec<NodeId> {
    runtime
        .tree()
        .traverse()
        .into_iter()
        .filter(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Text(_)))
        .collect()
}

fn fresh_runtime() -> Runtime {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(1200.0, 800.0, 1.0);
    runtime
}

struct Row {
    phase: &'static str,
    rows: usize,
    summary: Summary,
    work: FrameWork,
    scene_items: usize,
}

impl Row {
    fn line(&self) -> String {
        format!(
            "perf-scene rows={} phase={} p50_us={:.0} p99_us={:.0} max_us={:.0} allocs={} alloc_kb={} runs_shaped={} text_checked={} nodes_visited={} layout_measured={} ops_emitted={} scene_items={}",
            self.rows,
            self.phase,
            self.summary.p50_us,
            self.summary.p99_us,
            self.summary.max_us,
            self.summary.allocations,
            self.summary.bytes / 1024,
            self.work.text_runs_shaped,
            self.work.text_nodes_checked,
            self.work.tree_nodes_visited,
            self.work.layout_nodes_measured,
            self.work.tree_scene_ops_emitted,
            self.scene_items,
        )
    }
}

fn run_phases(rows: usize, repetitions: usize) -> Vec<Row> {
    let mut report = Vec::new();

    let mut cold = Vec::new();
    let mut work = FrameWork::default();
    let mut scene_items = 0;
    for _ in 0..repetitions.min(5) {
        let mut runtime = fresh_runtime();
        cold.push(measure(|| {
            mount_transcript(&mut runtime, rows);
            runtime.build_frame().unwrap();
        }));
        work = runtime.frame_work();
        scene_items = runtime.scene().ordered().count();
    }
    report.push(Row {
        phase: "cold",
        rows,
        summary: summarize(&cold),
        work,
        scene_items,
    });

    let mut runtime = fresh_runtime();
    mount_transcript(&mut runtime, rows);
    runtime.build_frame().unwrap();
    let nodes = text_nodes(&runtime);
    let scene_items = runtime.scene().ordered().count();

    let mut idle = Vec::new();
    for frame in 0..repetitions {
        runtime.advance_clock(frame as f64 / 60.0);
        idle.push(measure(|| {
            runtime.build_frame().unwrap();
        }));
    }
    report.push(Row {
        phase: "idle",
        rows,
        summary: summarize(&idle),
        work: runtime.frame_work(),
        scene_items,
    });

    let last = *nodes.last().unwrap();
    let mut message = row_text(rows - 1);
    let mut stream = Vec::new();
    for _ in 0..repetitions {
        message.push_str(" another streamed token");
        let text = message.clone();
        stream.push(measure(|| {
            runtime.tree_mut().set_text(last, text);
            runtime.build_frame().unwrap();
        }));
    }
    report.push(Row {
        phase: "stream",
        rows,
        summary: summarize(&stream),
        work: runtime.frame_work(),
        scene_items,
    });

    let mut resize = Vec::new();
    for step in 0..repetitions.min(10) {
        let width = 1200.0 + (step % 2) as f32 * 100.0;
        resize.push(measure(|| {
            runtime.set_viewport(width, 800.0, 1.0);
            runtime.build_frame().unwrap();
        }));
    }
    report.push(Row {
        phase: "resize",
        rows,
        summary: summarize(&resize),
        work: runtime.frame_work(),
        scene_items,
    });

    report
}

fn render_report(rows: usize, repetitions: usize) -> Option<Row> {
    let mut renderer = match pollster::block_on(Renderer::headless(1200, 800)) {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("perf-scene render skipped: {error:?}");
            return None;
        }
    };
    let mut runtime = fresh_runtime();
    mount_transcript(&mut runtime, rows);
    runtime.build_frame().unwrap();
    let scene_items = runtime.scene().ordered().count();
    let mut frames = Vec::new();
    for _ in 0..repetitions.min(20) {
        let mut scene = Scene::new();
        for (_, item) in runtime.scene().ordered() {
            scene.insert(item.clone());
        }
        frames.push(measure(|| {
            let plan = renderer.prepare(&mut scene);
            renderer.draw(&plan).unwrap();
        }));
    }
    Some(Row {
        phase: "render_full",
        rows,
        summary: summarize(&frames),
        work: runtime.frame_work(),
        scene_items,
    })
}

#[test]
#[ignore]
fn perf_scene_report() {
    for rows in [100, 1000, 5000] {
        for row in run_phases(rows, 40) {
            println!("{}", row.line());
        }
        if let Some(row) = render_report(rows, 20) {
            println!("{}", row.line());
        }
    }
}

#[test]
fn idle_frames_do_not_allocate_per_row() {
    let mut runtime = fresh_runtime();
    mount_transcript(&mut runtime, 1000);
    runtime.build_frame().unwrap();
    runtime.build_frame().unwrap();
    let mut worst = 0;
    for frame in 0..10 {
        runtime.advance_clock(frame as f64 / 60.0);
        worst = worst.max(
            measure(|| {
                runtime.build_frame().unwrap();
            })
            .allocations,
        );
    }
    assert!(worst == IDLE_FRAME_ALLOCATION_BUDGET, "{worst}");
}

#[test]
fn streaming_allocations_do_not_scale_with_transcript_length() {
    let streaming_cost = |rows: usize| {
        let mut runtime = fresh_runtime();
        mount_transcript(&mut runtime, rows);
        runtime.build_frame().unwrap();
        let last = *text_nodes(&runtime).last().unwrap();
        let mut message = row_text(rows - 1);
        let mut costs = Vec::new();
        for _ in 0..5 {
            message.push_str(" another streamed token");
            let text = message.clone();
            costs.push(measure(|| {
                runtime.tree_mut().set_text(last, text);
                runtime.build_frame().unwrap();
            }));
        }
        costs.last().unwrap().allocations
    };
    let small = streaming_cost(200);
    let large = streaming_cost(2000);
    assert!(
        large <= small * STREAMING_SCALING_LIMIT,
        "{small} allocations at 200 rows, {large} at 2000 rows"
    );
}

const IDLE_FRAME_ALLOCATION_BUDGET: u64 = 0;
const STREAMING_SCALING_LIMIT: u64 = 100;
