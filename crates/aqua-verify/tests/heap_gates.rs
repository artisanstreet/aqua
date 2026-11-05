use std::sync::atomic::{AtomicU64, Ordering};

use aqua::profile::recorder::{Recorder, Role};
use aqua_core::{DisplayItem, Oklch, Rect};
use aqua_protocol::probe::{self, Event, Metric, Phase};
use aqua_protocol::{Instant, RenderCommand, RenderInput, SceneDiff};
use aqua_runtime::{FrameWork, RenderSide, Runtime, emit_scene_diff};
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
use aqua_ui::style::{Axis, Style, px};
use aqua_verify::heap::{CountingAllocator, HeapGate, measure};
use aqua_verify::wake::WakeTally;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

static SPANS: AtomicU64 = AtomicU64::new(0);
static COUNTS: AtomicU64 = AtomicU64::new(0);

fn counting_sink(event: Event) {
    match event {
        Event::Enter(_) => SPANS.fetch_add(1, Ordering::Relaxed),
        Event::Exit(_) => 0,
        Event::Count(..) => COUNTS.fetch_add(1, Ordering::Relaxed),
    };
}

fn probed() {
    probe::install(counting_sink);
}

const PARAGRAPH: &str = "Aqua keeps retained text, layout and scene state between frames so that an unchanged transcript costs nothing, while a streaming message only reshapes the run that changed.";

fn row_text(index: usize) -> String {
    format!("Message {index}. {}", PARAGRAPH.repeat(1 + index % 4))
}

fn mounted_runtime(rows: usize) -> Runtime {
    probed();
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(1200.0, 800.0, 1.0);
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().axis(Axis::Column).width(px(760.0)))
                .children((0..rows).map(|index| {
                    UiBox::new()
                        .fill(Oklch::from_hex("#1e1e2e").unwrap())
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
    runtime
}

fn idle_cost(rows: usize) -> (aqua_verify::heap::Counts, FrameWork) {
    let mut runtime = mounted_runtime(rows);
    runtime.build_frame().unwrap();
    runtime.build_frame().unwrap();
    let mut worst = aqua_verify::heap::Counts::ZERO;
    for frame in 0..10 {
        runtime.advance_clock(frame as f64 / 60.0);
        let (_, counts) = measure(|| runtime.build_frame().unwrap());
        worst = aqua_verify::heap::Counts {
            allocations: worst.allocations.max(counts.allocations),
            reallocations: worst.reallocations.max(counts.reallocations),
            deallocations: worst.deallocations.max(counts.deallocations),
            bytes: worst.bytes.max(counts.bytes),
        };
    }
    (worst, runtime.frame_work())
}

#[test]
fn idle_runtime_frames_do_not_allocate_and_do_not_scale_with_rows() {
    let gate = HeapGate::ZERO;
    for rows in [100, 1000, 4000] {
        let (counts, work) = idle_cost(rows);
        if let Err(failure) = gate.check(counts) {
            panic!("{rows} rows: {failure}");
        }
        assert_eq!(work.text_runs_shaped, 0, "{rows} rows");
        assert_eq!(work.layout_nodes_measured, 0, "{rows} rows");
        assert_eq!(work.tree_scene_ops_emitted, 0, "{rows} rows");
    }
}

#[test]
fn cold_mount_shapes_each_run_exactly_once() {
    for rows in [50, 400] {
        let mut runtime = mounted_runtime(rows);
        runtime.build_frame().unwrap();
        let work = runtime.frame_work();
        assert_eq!(work.text_runs_shaped, rows, "{rows} rows");
        assert_eq!(work.layouts, 1, "{rows} rows");
        runtime.build_frame().unwrap();
        assert_eq!(runtime.frame_work().text_runs_shaped, 0, "{rows} rows");
    }
}

fn populated_render_side(items: usize) -> RenderSide {
    probed();
    let mut app_scene = Scene::new();
    for index in 0..items {
        app_scene.insert(DisplayItem::Rect {
            bounds: Rect {
                x: index as f32,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
            radius: 0.0,
            fill: Oklch::from_hex("#336699").unwrap(),
            border_width: 0.0,
            border_color: Oklch::from_hex("#000000").unwrap(),
            shadow: None,
        });
    }
    let mut side = RenderSide::new();
    let diff: SceneDiff = emit_scene_diff(&mut app_scene);
    side.step(RenderInput {
        commands: vec![RenderCommand::Scene(diff)],
        feedback: Vec::new(),
        present_at: Instant::ZERO,
    });
    side
}

#[test]
fn idle_render_side_steps_do_not_allocate_and_request_no_wake() {
    for items in [10, 5000] {
        let mut side = populated_render_side(items);
        let mut tally = WakeTally::default();
        let mut worst = aqua_verify::heap::Counts::ZERO;
        for frame in 1..=120u64 {
            let input = RenderInput {
                commands: Vec::new(),
                feedback: Vec::new(),
                present_at: Instant::from_nanos(frame * 16_666_667),
            };
            let (output, counts) = measure(|| side.step(input));
            tally.record(output.wake);
            assert!(output.frame.is_none() || frame == 1, "frame {frame}");
            worst.allocations = worst.allocations.max(counts.allocations);
            worst.reallocations = worst.reallocations.max(counts.reallocations);
        }
        assert_eq!(worst.heap_events(), 0, "{items} items");
        assert_eq!(tally.requested(), 0, "{items} items");
        assert_eq!(tally.idle, 120);
    }
}

#[test]
fn emitting_an_unchanged_scene_allocates_nothing() {
    let mut scene = Scene::new();
    for index in 0..2000 {
        scene.insert(DisplayItem::Rect {
            bounds: Rect {
                x: index as f32,
                y: 0.0,
                w: 4.0,
                h: 4.0,
            },
            radius: 0.0,
            fill: Oklch::from_hex("#336699").unwrap(),
            border_width: 0.0,
            border_color: Oklch::from_hex("#000000").unwrap(),
            shadow: None,
        });
    }
    let first = emit_scene_diff(&mut scene);
    assert!(!first.is_empty());
    let (second, counts) = measure(|| emit_scene_diff(&mut scene));
    assert!(second.is_empty());
    assert_eq!(counts.heap_events(), 0);
}

#[test]
fn an_active_probe_sink_adds_no_allocations_to_idle_frames() {
    let mut runtime = mounted_runtime(400);
    runtime.build_frame().unwrap();
    runtime.build_frame().unwrap();
    assert!(probe::active());
    let spans = SPANS.load(Ordering::Relaxed);
    let counts = COUNTS.load(Ordering::Relaxed);
    let mut worst = 0;
    for frame in 0..20 {
        runtime.advance_clock(frame as f64 / 60.0);
        let (_, measured) = measure(|| runtime.build_frame().unwrap());
        worst = worst.max(measured.heap_events());
    }
    assert!(
        SPANS.load(Ordering::Relaxed) > spans && COUNTS.load(Ordering::Relaxed) > counts,
        "the sink saw no spans or counts, so the gate measured nothing"
    );
    assert_eq!(worst, 0);
}

#[test]
fn the_profile_recorder_allocates_nothing_per_event_or_per_frame() {
    let mut recorder = Recorder::new();
    recorder.begin();
    recorder.event(Event::Enter(Phase::Build));
    recorder.event(Event::Exit(Phase::Build));
    drop(recorder.finish(Role::App, 0));
    let (finished, counts) = measure(|| {
        let mut last = None;
        for frame in 0..200u64 {
            recorder.begin();
            for phase in Phase::ALL {
                recorder.event(Event::Enter(phase));
                recorder.event(Event::Enter(Phase::Text));
                recorder.event(Event::Count(Metric::TextRunsShaped, 3));
                recorder.event(Event::Exit(Phase::Text));
                recorder.event(Event::Exit(phase));
            }
            last = Some(recorder.finish(Role::App, frame));
        }
        last.unwrap()
    });
    assert_eq!(counts.heap_events(), 0);
    assert!(finished.metrics[Metric::TextRunsShaped.index()] > 0);
}
