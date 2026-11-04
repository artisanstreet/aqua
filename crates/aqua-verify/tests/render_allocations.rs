use aqua_core::{InputEvent, Point, ScrollDelta};
use aqua_protocol::{
    Instant, Machine, PixelSize, RenderInput, RenderWake, RuntimeInput, ShellEvent, WindowId,
};
use aqua_runtime::{RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Scroll};
use aqua_ui::style::{px, Axis, Style};
use aqua_verify::heap::{measure, CountingAllocator};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn window() -> WindowId {
    WindowId::new(1).unwrap()
}

fn step(
    render: &mut RenderSide,
    commands: Vec<aqua_protocol::RenderCommand>,
    secs: f64,
) -> RenderWake {
    render
        .step(RenderInput {
            commands,
            feedback: Vec::new(),
            present_at: Instant::from_secs_f64(secs),
        })
        .wake
}

fn gliding_rig() -> (RenderSide, f64) {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            Scroll::new()
                .style(Style::new().width(px(200.0)).height(px(100.0)))
                .child(
                    UiBox::new()
                        .style(
                            Style::new()
                                .axis(Axis::Column)
                                .width(px(200.0))
                                .height(px(20000.0))
                                .flex_shrink(0.0),
                        )
                        .build(),
                )
                .build()
        })
        .unwrap();
    runtime.step(RuntimeInput::Shell(ShellEvent::Resized {
        window: window(),
        size: PixelSize {
            width: 300,
            height: 200,
        },
        scale: 1.0,
    }));
    let mut render = RenderSide::new();
    let frame = |runtime: &mut Runtime, render: &mut RenderSide, secs: f64| {
        let out = runtime.step(RuntimeInput::Shell(ShellEvent::Frame {
            window: window(),
            presentation: Instant::from_secs_f64(secs),
        }));
        step(render, out.render, secs);
    };
    frame(&mut runtime, &mut render, 0.0);
    runtime.step(RuntimeInput::Shell(ShellEvent::Input {
        window: window(),
        event: InputEvent::Scroll {
            at: Point::new(20.0, 20.0),
            delta: ScrollDelta { x: 0.0, y: 48.0 },
        },
    }));
    frame(&mut runtime, &mut render, 0.5);
    (render, 0.5)
}

#[test]
fn glide_frames_on_the_render_side_do_not_allocate() {
    let (mut render, start) = gliding_rig();
    for warm in 1..=4 {
        step(&mut render, Vec::new(), start + f64::from(warm) / 240.0);
    }
    let mut worst = 0;
    let mut frames = 0;
    for index in 5..40 {
        let secs = start + f64::from(index) / 240.0;
        let (wake, counts) = measure(|| step(&mut render, Vec::new(), secs));
        assert_eq!(wake, RenderWake::NextFrame, "frame {index} is mid-glide");
        worst = worst.max(counts.heap_events());
        frames += 1;
    }
    assert!(frames > 0);
    assert_eq!(worst, 0, "heap events on the worst glide frame");
}
