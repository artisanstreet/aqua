use aqua_core::{DisplayItem, InputEvent, Oklch, Point, Rect, ScrollDelta};
use aqua_motion::Easing;
use aqua_protocol::{Instant, Machine, RenderCommand, RenderEvent, RenderInput, RuntimeInput};
use aqua_runtime::{RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Scroll};
use aqua_ui::motion::{Animate, Motion};
use aqua_ui::style::{px, Axis, Style};

mod support;

const SLIDE: Motion = Motion::tween(1.0, Easing::Linear);
const NOTCH: f32 = 77.7;
const BEFORE: f32 = 100.0;
const AFTER: f32 = 160.0;

struct Rig {
    runtime: Runtime,
    render: RenderSide,
}

impl Rig {
    fn new() -> Self {
        let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
        runtime
            .mount(|_| {
                Scroll::new()
                    .style(Style::new().width(px(200.0)).height(px(150.0)))
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
                                            .height(px(BEFORE))
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
                                    .animate(Animate::new().frame(SLIDE))
                                    .build(),
                            )
                            .build(),
                    )
                    .build()
            })
            .unwrap();
        support::resize(&mut runtime, 300.0, 200.0, 1.0);
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
        let out = support::frame(&mut self.runtime, secs);
        self.deliver(out.render, secs);
    }

    fn render_only(&mut self, secs: f64) {
        self.deliver(Vec::new(), secs);
    }

    fn wheel(&mut self, secs: f64) {
        self.runtime
            .step(RuntimeInput::Shell(aqua_protocol::ShellEvent::Input {
                window: support::window(),
                event: InputEvent::Scroll {
                    at: Point::new(20.0, 20.0),
                    delta: ScrollDelta { x: 0.0, y: 16.0 },
                },
            }));
        self.app_frame(secs);
    }

    fn block(&self) -> Rect {
        self.render
            .scene()
            .ordered()
            .find_map(|(_, item)| match item {
                DisplayItem::Rect { bounds, fill, .. } if fill.alpha > 0.0 => Some(*bounds),
                _ => None,
            })
            .unwrap_or_else(|| {
                let tree = self.runtime.tree();
                panic!(
                    "{:?}",
                    tree.traverse()
                        .into_iter()
                        .map(|n| format!("{:?} {:?}", n, tree.rect(n)))
                        .collect::<Vec<_>>()
                )
            })
    }

    fn content_glide(&self) -> f32 {
        self.render
            .scene()
            .ordered()
            .find_map(|(_, item)| match item {
                DisplayItem::PushTransform(t) => Some(t.ty),
                _ => None,
            })
            .unwrap_or(0.0)
    }

    fn scroll_translate(&self) -> f32 {
        self.render
            .scene()
            .ordered()
            .filter_map(|(_, item)| match item {
                DisplayItem::PushTransform(t) => Some(t.ty),
                _ => None,
            })
            .nth(1)
            .unwrap_or(0.0)
    }

    fn visual_top(&self) -> f32 {
        self.block().y + self.content_glide() + self.scroll_translate()
    }
}

fn scrolled_rig() -> Rig {
    let mut rig = Rig::new();
    rig.app_frame(0.0);
    rig.app_frame(0.016);
    rig.wheel(0.5);
    rig.render_only(5.0);
    assert!(rig.render.tracks().is_idle());
    assert!(
        (rig.visual_top() - (BEFORE - NOTCH)).abs() < 1e-3,
        "{}",
        rig.visual_top()
    );
    rig
}

#[test]
fn an_edge_that_moves_inside_a_scrolled_ancestor_animates_in_scrolled_coordinates() {
    let mut rig = scrolled_rig();
    rig.set_spacer(AFTER);
    rig.app_frame(10.0);
    let start = rig.block();
    assert!((start.y - BEFORE).abs() < 1e-3, "{start:?}");
    rig.render_only(10.5);
    let mid = rig.block();
    let expected = (BEFORE + AFTER) / 2.0;
    assert!((mid.y - expected).abs() < 1e-2, "{mid:?} vs {expected}");
    assert!((mid.h - 40.0).abs() < 1e-3, "height stays put: {mid:?}");
    rig.render_only(12.0);
    let end = rig.block();
    assert!((end.y - AFTER).abs() < 1e-3, "{end:?}");
    assert!(rig.render.tracks().is_idle());
}

#[test]
fn scrolling_during_an_edge_animation_keeps_the_block_continuous_and_lands_exactly() {
    let mut rig = scrolled_rig();
    rig.set_spacer(AFTER);
    rig.app_frame(10.0);
    rig.render_only(10.4);
    let before = rig.visual_top();
    rig.wheel(10.4);
    let after = rig.visual_top();
    assert!(
        (after - before).abs() < 1.0,
        "a wheel notch must not jump the block: {before} -> {after}"
    );
    let mut previous = after;
    for step in 1..=60 {
        rig.render_only(10.4 + f64::from(step) / 60.0);
        let top = rig.visual_top();
        assert!(
            (top - previous).abs() < 15.0,
            "step {step}: {previous} -> {top}"
        );
        previous = top;
    }
    let end = rig.block();
    assert!((end.y - AFTER).abs() < 1e-2, "{end:?}");
    assert_eq!(rig.content_glide(), 0.0);
    assert!(rig.render.tracks().is_idle());
}
