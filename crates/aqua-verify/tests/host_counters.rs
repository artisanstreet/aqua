use aqua_core::{InputEvent, Point, ScrollDelta};
use aqua_protocol::{
    Instant, Machine, PixelSize, RenderCommand, RenderInput, RenderWake, RuntimeInput, ShellEvent,
    WindowId,
};
use aqua_runtime::{RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Scroll};
use aqua_ui::style::{px, Axis, Style};
use aqua_verify::heap::{measure, CountingAllocator, Counts};
use aqua_verify::wake::WakeTally;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn window() -> WindowId {
    WindowId::new(1).unwrap()
}

fn frame(secs: f64) -> RuntimeInput {
    RuntimeInput::Shell(ShellEvent::Frame {
        window: window(),
        presentation: Instant::from_secs_f64(secs),
    })
}

fn mounted() -> Runtime {
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
                                .height(px(4000.0))
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
    runtime
}

struct Host {
    runtime: Runtime,
    side: RenderSide,
    app_wakes: WakeTally,
    render_wakes: WakeTally,
    app_heap: Counts,
    render_heap: Counts,
    presented: u64,
    clock: f64,
}

impl Host {
    fn new() -> Self {
        let mut host = Self {
            runtime: mounted(),
            side: RenderSide::new(),
            app_wakes: WakeTally::default(),
            render_wakes: WakeTally::default(),
            app_heap: Counts::ZERO,
            render_heap: Counts::ZERO,
            presented: 0,
            clock: 0.0,
        };
        host.cycle(Vec::new());
        host.cycle(Vec::new());
        host.reset();
        host
    }

    fn reset(&mut self) {
        self.app_wakes = WakeTally::default();
        self.render_wakes = WakeTally::default();
        self.app_heap = Counts::ZERO;
        self.render_heap = Counts::ZERO;
        self.presented = 0;
    }

    fn cycle(&mut self, inputs: Vec<RuntimeInput>) -> RenderWake {
        self.clock += 1.0 / 120.0;
        let mut commands: Vec<RenderCommand> = Vec::new();
        inputs.into_iter().for_each(|input| {
            let output = self.runtime.step(input);
            commands.extend(output.render);
        });
        let clock = self.clock;
        let (output, counts) = measure(|| self.runtime.step(frame(clock)));
        self.app_heap = add(self.app_heap, counts);
        self.app_wakes.record_deadline(output.wake_at);
        commands.extend(output.render);
        let input = RenderInput {
            commands,
            feedback: Vec::new(),
            present_at: Instant::from_secs_f64(self.clock),
        };
        let (rendered, counts) = measure(|| self.side.step(input));
        self.render_heap = add(self.render_heap, counts);
        self.render_wakes.record(rendered.wake);
        self.presented += u64::from(rendered.frame.is_some());
        rendered.events.into_iter().for_each(|event| {
            self.runtime.step(RuntimeInput::Render(event));
        });
        rendered.wake
    }
}

fn add(a: Counts, b: Counts) -> Counts {
    Counts {
        allocations: a.allocations + b.allocations,
        reallocations: a.reallocations + b.reallocations,
        deallocations: a.deallocations + b.deallocations,
        bytes: a.bytes + b.bytes,
    }
}

fn notch(lines: f32) -> RuntimeInput {
    RuntimeInput::Shell(ShellEvent::Input {
        window: window(),
        event: InputEvent::Scroll {
            at: Point::new(20.0, 20.0),
            delta: ScrollDelta {
                x: 0.0,
                y: 16.0 * lines,
            },
        },
    })
}

#[test]
fn an_idle_host_cycle_makes_no_allocation_no_wake_and_presents_nothing() {
    let mut host = Host::new();
    (0..240).for_each(|_| {
        host.cycle(Vec::new());
    });
    assert_eq!(host.app_heap.heap_events(), 0);
    assert_eq!(host.render_heap.heap_events(), 0);
    assert_eq!(host.app_wakes.requested(), 0);
    assert_eq!(host.app_wakes.idle, 240);
    assert_eq!(host.render_wakes.requested(), 0);
    assert_eq!(host.render_wakes.idle, 240);
    assert_eq!(host.presented, 0);
}

#[test]
fn a_wheel_notch_wakes_the_render_side_exactly_for_the_glide_and_never_the_app() {
    let mut host = Host::new();
    host.cycle(vec![notch(1.0)]);
    host.reset();
    let mut settled_after = None;
    (0..480).for_each(|step| {
        let wake = host.cycle(Vec::new());
        if wake == RenderWake::Idle && settled_after.is_none() {
            settled_after = Some(step + 1);
        }
    });
    let glide = settled_after.expect("the glide settles");
    assert_eq!(host.presented, glide as u64);
    assert_eq!(host.render_wakes.next_frame, glide as u64 - 1);
    assert_eq!(host.render_wakes.timed, 0);
    assert_eq!(host.render_wakes.idle, 480 - (glide as u64 - 1));
    assert_eq!(host.app_wakes.requested(), 0);
    assert_eq!(host.app_heap.heap_events(), 0);
    assert!(
        glide as f64 / 120.0 < 1.0,
        "the glide settles within a second"
    );
    let per_frame = host.render_heap.heap_events() as f64 / host.presented as f64;
    assert!(per_frame <= 6.0, "{per_frame} heap events per glide frame");
}

#[test]
fn a_settled_glide_leaves_the_host_exactly_as_idle_as_before_it() {
    let mut host = Host::new();
    host.cycle(vec![notch(2.0)]);
    (0..400).for_each(|_| {
        host.cycle(Vec::new());
    });
    host.reset();
    (0..240).for_each(|_| {
        host.cycle(Vec::new());
    });
    assert_eq!(host.render_heap.heap_events(), 0);
    assert_eq!(host.app_heap.heap_events(), 0);
    assert_eq!(host.render_wakes.requested(), 0);
    assert_eq!(host.presented, 0);
    assert!(host.side.tracks().is_idle());
}
