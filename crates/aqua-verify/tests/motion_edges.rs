use aqua_core::{DisplayItem, InputEvent, Oklch, Point, ScrollDelta};
use aqua_motion::Easing;
use aqua_protocol::{
    Instant, Machine, PixelSize, RenderCommand, RenderEvent, RenderInput, RuntimeInput, ShellEvent,
    WindowId,
};
use aqua_runtime::{PopupOptions, RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Scroll};
use aqua_ui::motion::{Animate, Motion, Presence};
use aqua_ui::overlay::OverlayId;
use aqua_ui::style::{px, Axis, Style};

const RATES: [f64; 3] = [60.0, 144.0, 240.0];
const REFERENCE: f64 = 1440.0;
const GRID: f64 = 720.0;
const EDGE: Motion = Motion::tween(0.3, Easing::EaseInOut);
const FADE: Motion = Motion::tween(0.25, Easing::Linear);
const NOTCH: f32 = 77.7;

fn window() -> WindowId {
    WindowId::new(1).unwrap()
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

struct Rig {
    runtime: Runtime,
    render: RenderSide,
}

impl Rig {
    fn new(runtime: Runtime) -> Self {
        Self {
            runtime,
            render: RenderSide::new(),
        }
    }

    fn set_spacer(&mut self, height: f32) {
        let root = self.runtime.tree().root().unwrap();
        let content = self.runtime.tree().get(root).unwrap().children[0];
        let spacer = self.runtime.tree().get(content).unwrap().children[0];
        let mut style = self.runtime.tree().get(spacer).unwrap().style.clone();
        style.height = px(height);
        self.runtime.tree_mut().set_style(spacer, style);
    }

    fn deliver(&mut self, commands: Vec<RenderCommand>, secs: f64) {
        let output = self.render.step(RenderInput {
            commands,
            feedback: Vec::new(),
            present_at: Instant::from_secs_f64(secs),
        });
        for event in output.events {
            if matches!(event, RenderEvent::Motion(_)) {
                self.runtime.step(RuntimeInput::Render(event));
            }
        }
    }

    fn app_frame(&mut self, secs: f64) {
        let out = self.runtime.step(RuntimeInput::Shell(ShellEvent::Frame {
            window: window(),
            presentation: Instant::from_secs_f64(secs),
        }));
        self.deliver(out.render, secs);
    }

    fn render_only(&mut self, secs: f64) {
        self.deliver(Vec::new(), secs);
    }

    fn wheel(&mut self, secs: f64) {
        self.runtime.step(RuntimeInput::Shell(ShellEvent::Input {
            window: window(),
            event: InputEvent::Scroll {
                at: Point::new(20.0, 20.0),
                delta: ScrollDelta { x: 0.0, y: 16.0 },
            },
        }));
        self.app_frame(secs);
    }

    fn resize(&mut self) {
        self.runtime.set_viewport(300.0, 200.0, 1.0);
        self.runtime.step(RuntimeInput::Shell(ShellEvent::Resized {
            window: window(),
            size: PixelSize {
                width: 300,
                height: 200,
            },
            scale: 1.0,
        }));
    }

    fn block(&self) -> Option<(f32, f32)> {
        let glide = self
            .render
            .scene()
            .ordered()
            .find_map(|(_, item)| match item {
                DisplayItem::PushTransform(t) => Some(t.ty),
                _ => None,
            })
            .unwrap_or(0.0);
        self.render
            .scene()
            .ordered()
            .find_map(|(_, item)| match item {
                DisplayItem::Rect { bounds, fill, .. } if fill.alpha > 0.0 => {
                    Some((bounds.y + glide, bounds.h))
                }
                _ => None,
            })
    }

    fn opacity(&self) -> Option<f32> {
        self.render
            .scene()
            .ordered()
            .find_map(|(_, item)| match item {
                DisplayItem::PushOpacity(o) => Some(*o),
                _ => None,
            })
    }
}

#[derive(Clone, Copy)]
struct Change {
    at: f64,
    value: f32,
}

fn changes(seed: u64, count: usize, lo: f32, hi: f32) -> Vec<Change> {
    let mut rng = Rng(seed);
    let mut tick = 720;
    (0..count)
        .map(|_| {
            tick += 20 + (rng.next() % 300);
            Change {
                at: tick as f64 / GRID,
                value: lo + (rng.next() % 1000) as f32 / 1000.0 * (hi - lo),
            }
        })
        .collect()
}

fn edge_rig(scrolled: bool) -> Rig {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime
        .mount(|_| {
            Scroll::new()
                .style(Style::new().width(px(200.0)).height(px(500.0)))
                .child(
                    UiBox::new()
                        .style(
                            Style::new()
                                .axis(Axis::Column)
                                .width(px(200.0))
                                .height(px(3000.0))
                                .flex_shrink(0.0),
                        )
                        .child(
                            UiBox::new()
                                .style(
                                    Style::new()
                                        .width(px(200.0))
                                        .height(px(100.0))
                                        .flex_shrink(0.0),
                                )
                                .build(),
                        )
                        .child(
                            UiBox::new()
                                .style(
                                    Style::new()
                                        .width(px(100.0))
                                        .height(px(40.0))
                                        .flex_shrink(0.0),
                                )
                                .fill(Oklch::from_hex("#ff0000").unwrap())
                                .animate(Animate::new().frame(EDGE))
                                .build(),
                        )
                        .build(),
                )
                .build()
        })
        .unwrap();
    let mut rig = Rig::new(runtime);
    rig.resize();
    rig.app_frame(0.0);
    rig.app_frame(0.016);
    if scrolled {
        rig.wheel(0.3);
        rig.render_only(0.9);
    }
    rig
}

fn edge_trace(seed: u64, scrolled: bool, rate: f64, end: f64) -> Vec<(f64, f32)> {
    let mut rig = edge_rig(scrolled);
    let plan = changes(seed, 6, 100.0, 400.0);
    let mut pending = plan.iter().peekable();
    let steps = (end * rate) as usize;
    let mut trace = Vec::new();
    for step in 1..=steps {
        let time = step as f64 / rate;
        while let Some(change) = pending.next_if(|change| change.at <= time) {
            rig.set_spacer(change.value);
            rig.app_frame(change.at);
        }
        rig.render_only(time);
        if time >= 1.0 {
            if let Some((top, _)) = rig.block() {
                trace.push((time, top));
            }
        }
    }
    trace
}

#[test]
fn layout_edge_motion_does_not_depend_on_the_presentation_rate_in_plain_and_scrolled_parents() {
    for scrolled in [false, true] {
        for seed in 1..=6 {
            let end = changes(seed, 6, 0.0, 1.0).last().unwrap().at + 1.5;
            let reference = edge_trace(seed, scrolled, REFERENCE, end);
            for rate in RATES {
                let stride = (REFERENCE / rate).round() as usize;
                let trace = edge_trace(seed, scrolled, rate, end);
                assert!(!trace.is_empty());
                for (time, top) in &trace {
                    let (_, expected) = reference
                        .iter()
                        .find(|(t, _)| (t - time).abs() < 1e-6)
                        .unwrap_or_else(|| panic!("no reference sample at {time} stride {stride}"));
                    assert!(
                        (top - expected).abs() < 0.1,
                        "scrolled {scrolled} seed {seed} {rate} Hz t={time}: {top} vs {expected}"
                    );
                }
            }
        }
    }
}

#[test]
fn layout_edge_motion_has_no_one_frame_jump_and_lands_on_the_final_layout() {
    for scrolled in [false, true] {
        for seed in 1..=6 {
            let plan = changes(seed, 6, 100.0, 400.0);
            let end = plan.last().unwrap().at + 1.5;
            let trace = edge_trace(seed, scrolled, REFERENCE, end);
            let travel = plan.iter().map(|c| c.value).fold(0.0f32, f32::max) * 2.0;
            let jumps: Vec<f64> = trace
                .windows(4)
                .filter(|w| {
                    let before = (w[1].1 - w[0].1).abs();
                    let step = (w[2].1 - w[1].1).abs();
                    let after = (w[3].1 - w[2].1).abs();
                    step > before.max(after) * 1.5 + travel * 1e-4
                })
                .map(|w| w[1].0)
                .collect();
            assert!(
                jumps.is_empty(),
                "scrolled {scrolled} seed {seed}: {jumps:?}"
            );
            let landed = trace.last().unwrap().1;
            let offset = if scrolled { NOTCH } else { 0.0 };
            let expected = plan.last().unwrap().value.round() - offset;
            assert!(
                (landed - expected).abs() < 1e-3,
                "scrolled {scrolled} seed {seed}: {landed} vs {expected}"
            );
        }
    }
}

fn presence_rig() -> Rig {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(300.0, 200.0, 1.0);
    runtime.mount(|_| UiBox::new().build()).unwrap();
    let mut rig = Rig::new(runtime);
    rig.resize();
    rig.runtime.build_frame().unwrap();
    rig.app_frame(0.0);
    rig
}

fn open(rig: &mut Rig) -> OverlayId {
    let owner = rig.runtime.tree().root().unwrap();
    rig.runtime
        .open_popup(
            owner,
            UiBox::new()
                .style(
                    Style::new()
                        .width(px(80.0))
                        .height(px(40.0))
                        .background(Oklch::from_hex("#334455").unwrap()),
                )
                .build(),
            PopupOptions::menu()
                .at(Point::new(10.0, 20.0))
                .presence(Presence::new().enter(FADE).exit(FADE)),
        )
        .unwrap()
}

fn presence_trace(seed: u64, rate: f64) -> (Vec<(f64, f32)>, f64) {
    let mut rng = Rng(seed);
    let open_at = 1.0;
    let close_after = (40 + rng.next() % 260) as f64 / GRID;
    let close_at = open_at + close_after;
    let end = close_at + 1.0;
    let mut rig = presence_rig();
    let mut popup = None;
    let mut closed = false;
    let mut trace = Vec::new();
    for step in 1..=((end * rate) as usize) {
        let time = step as f64 / rate;
        if popup.is_none() && time >= open_at {
            popup = Some(open(&mut rig));
            rig.app_frame(open_at);
        }
        if let (Some(id), false) = (popup, closed) {
            if time >= close_at {
                assert!(rig.runtime.close_popup(id));
                closed = true;
                rig.app_frame(close_at);
            }
        }
        rig.render_only(time);
        if time > open_at {
            trace.push((time, rig.opacity().unwrap_or(0.0)));
        }
    }
    (trace, close_at)
}

#[test]
fn presence_enter_and_exit_do_not_depend_on_the_presentation_rate() {
    for seed in 1..=10 {
        let (reference, _) = presence_trace(seed, REFERENCE);
        for rate in RATES {
            let (trace, _) = presence_trace(seed, rate);
            for (time, opacity) in &trace {
                let (_, expected) = reference
                    .iter()
                    .find(|(t, _)| (t - time).abs() < 1e-6)
                    .unwrap();
                assert!(
                    (opacity - expected).abs() < 0.02,
                    "seed {seed} {rate} Hz t={time}: {opacity} vs {expected}"
                );
            }
        }
    }
}

#[test]
fn closing_mid_enter_reverses_from_the_presented_opacity_without_a_jump() {
    for seed in 1..=10 {
        let (trace, close_at) = presence_trace(seed, REFERENCE);
        let peak = trace
            .iter()
            .filter(|(t, _)| *t <= close_at)
            .map(|(_, o)| *o)
            .fold(0.0f32, f32::max);
        let at_close = trace
            .iter()
            .position(|(t, _)| *t > close_at)
            .expect("samples past the close");
        let before = trace[at_close - 1].1;
        let after = trace[at_close].1;
        assert!(
            (after - before).abs() < 0.01,
            "seed {seed}: opacity jumped {before} -> {after} at the close"
        );
        let mut previous = after;
        for (time, opacity) in &trace[at_close..] {
            assert!(
                *opacity <= previous + 1e-4 && *opacity <= peak + 1e-4,
                "seed {seed} t={time}: exit must only fall ({previous} -> {opacity})"
            );
            previous = *opacity;
        }
        assert_eq!(trace.last().unwrap().1, 0.0, "seed {seed}");
    }
}
