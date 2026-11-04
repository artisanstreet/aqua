use aqua_core::{DisplayItem, Oklch};
use aqua_motion::{Easing, Property};
use aqua_protocol::{
    Instant, Machine, RenderCommand, RenderEvent, RenderInput, RuntimeInput, RuntimeOutput,
};
use aqua_runtime::{RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::Box as UiBox;
use aqua_ui::motion::{Animate, Motion, Presence};
use aqua_ui::style::{px, Axis, Style};

mod support;

const FADE: Motion = Motion::tween(1.0, Easing::Linear);

struct Rig {
    runtime: Runtime,
    render: RenderSide,
    commands: Vec<RenderCommand>,
}

impl Rig {
    fn new(runtime: Runtime) -> Self {
        Self {
            runtime,
            render: RenderSide::new(),
            commands: Vec::new(),
        }
    }

    fn app_frame(&mut self, secs: f64) -> RuntimeOutput {
        let out = support::frame(&mut self.runtime, secs);
        self.commands.extend(out.render.clone());
        self.deliver(out.render.clone(), secs);
        out
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

    fn render_only(&mut self, secs: f64) {
        self.deliver(Vec::new(), secs);
    }

    fn fills(&self) -> Vec<Oklch> {
        self.render
            .scene()
            .ordered()
            .filter_map(|(_, item)| match item {
                DisplayItem::Rect { fill, .. } if fill.alpha > 0.0 => Some(*fill),
                _ => None,
            })
            .collect()
    }

    fn opacities(&self) -> Vec<f32> {
        self.render
            .scene()
            .ordered()
            .filter_map(|(_, item)| match item {
                DisplayItem::PushOpacity(opacity) => Some(*opacity),
                _ => None,
            })
            .collect()
    }

    fn rect_widths(&self) -> Vec<f32> {
        self.render
            .scene()
            .ordered()
            .filter_map(|(_, item)| match item {
                DisplayItem::Rect { bounds, .. } => Some(bounds.w),
                _ => None,
            })
            .collect()
    }
}

fn red() -> Oklch {
    Oklch::from_hex("#ff0000").unwrap()
}

fn blue() -> Oklch {
    Oklch::from_hex("#0000ff").unwrap()
}

fn animated_fill_runtime() -> (Runtime, aqua_reactive::WriteSignal<bool>) {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(300.0, 200.0, 1.0);
    let (on, set_on) = runtime.scope().signal(false);
    runtime
        .mount_reactive((on,), |(on,)| {
            UiBox::new()
                .style(
                    Style::new()
                        .axis(Axis::Column)
                        .width(px(300.0))
                        .height(px(200.0)),
                )
                .child(
                    UiBox::new()
                        .style(Style::new().width(px(100.0)).height(px(40.0)))
                        .fill(if on { blue() } else { red() })
                        .animate(Animate::new().fill(FADE))
                        .build(),
                )
                .build()
        })
        .unwrap();
    (runtime, set_on)
}

#[test]
fn fill_changes_are_interpolated_by_the_render_side_from_one_retarget() {
    let (runtime, set_on) = animated_fill_runtime();
    let mut rig = Rig::new(runtime);
    rig.app_frame(0.0);
    rig.app_frame(0.016);
    assert_eq!(rig.fills(), vec![red()]);
    assert!(rig.render.tracks().is_idle());

    set_on.set(true);
    let sent_before = rig.commands.len();
    rig.app_frame(1.0);
    let retargets = rig.commands[sent_before..]
        .iter()
        .filter(|command| {
            matches!(
                command,
                RenderCommand::Motion(aqua_motion::MotionCommand::SetTarget { .. })
            )
        })
        .count();
    assert_eq!(retargets, 1);

    rig.render_only(1.5);
    let mid = rig.fills()[0];
    assert!(mid != red() && mid != blue());
    assert!((mid.l - red().lerp(blue(), 0.5).l).abs() < 1e-3);

    let count = rig.commands.len();
    for step in 0..30 {
        rig.render_only(1.5 + f64::from(step) * 0.02);
    }
    assert_eq!(rig.commands.len(), count);

    rig.render_only(3.0);
    assert_eq!(rig.fills(), vec![blue()]);
    assert!(rig.render.tracks().is_idle());
}

#[test]
fn bindings_are_sent_as_scene_nodes_not_tree_nodes() {
    let (runtime, set_on) = animated_fill_runtime();
    let mut rig = Rig::new(runtime);
    rig.app_frame(0.0);
    set_on.set(true);
    rig.app_frame(1.0);
    let binds: Vec<_> = rig
        .commands
        .iter()
        .filter_map(|command| match command {
            RenderCommand::Motion(aqua_motion::MotionCommand::Bind { binding, .. }) => {
                Some(binding.property)
            }
            _ => None,
        })
        .collect();
    assert_eq!(binds, vec![Property::Fill]);
    let spawn_bindings: Vec<_> = rig
        .commands
        .iter()
        .filter_map(|command| match command {
            RenderCommand::Motion(aqua_motion::MotionCommand::Spawn { binding, .. }) => {
                Some(*binding)
            }
            _ => None,
        })
        .collect();
    assert!(spawn_bindings.iter().all(Option::is_none));
}

fn list_runtime() -> (Runtime, aqua_reactive::WriteSignal<Vec<u32>>) {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(300.0, 200.0, 1.0);
    let (rows, set_rows) = runtime.scope().signal(vec![1u32, 2, 3]);
    runtime
        .mount_reactive((rows,), |(rows,)| {
            UiBox::new()
                .style(
                    Style::new()
                        .axis(Axis::Column)
                        .width(px(300.0))
                        .height(px(200.0)),
                )
                .children(rows.iter().map(|id| {
                    UiBox::new()
                        .style(Style::new().width(px(100.0)).height(px(20.0)))
                        .fill(red())
                        .presence(Presence::new().enter(FADE).exit(FADE))
                        .build()
                        .with_key(*id)
                }))
                .build()
        })
        .unwrap();
    (runtime, set_rows)
}

#[test]
fn exiting_rows_stay_in_the_scene_until_their_fade_settles() {
    let (runtime, set_rows) = list_runtime();
    let mut rig = Rig::new(runtime);
    rig.app_frame(0.0);
    rig.app_frame(0.016);
    assert_eq!(rig.rect_widths().len(), 3);

    set_rows.set(vec![1, 3]);
    rig.app_frame(1.0);
    assert_eq!(rig.rect_widths().len(), 3);
    rig.render_only(1.5);
    let faded: Vec<f32> = rig.opacities();
    assert!(faded.iter().any(|o| *o > 0.0 && *o < 1.0));
    rig.app_frame(1.6);
    assert_eq!(rig.rect_widths().len(), 3);

    rig.render_only(3.0);
    rig.app_frame(3.1);
    rig.app_frame(3.2);
    assert_eq!(rig.rect_widths().len(), 2);
    assert!(rig.runtime.tree().exiting_nodes().is_empty());
}

#[test]
fn arriving_rows_fade_in_and_first_mount_does_not() {
    let (runtime, set_rows) = list_runtime();
    let mut rig = Rig::new(runtime);
    rig.app_frame(0.0);
    assert!(rig.opacities().iter().all(|o| *o == 1.0));

    set_rows.set(vec![1, 2, 3, 4]);
    rig.app_frame(1.0);
    let fresh: Vec<f32> = rig.opacities();
    assert_eq!(fresh.len(), 4);
    assert!(fresh.iter().filter(|o| **o < 0.05).count() == 1);
    rig.render_only(3.0);
    assert!(rig.opacities().iter().all(|o| *o == 1.0));
}

fn popup_runtime() -> Runtime {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(300.0, 200.0, 1.0);
    runtime
        .mount(|_| {
            UiBox::new()
                .style(
                    Style::new()
                        .axis(Axis::Column)
                        .width(px(300.0))
                        .height(px(200.0)),
                )
                .build()
        })
        .unwrap();
    runtime
}

fn popup_content() -> aqua_ui::element::Element {
    UiBox::new()
        .style(Style::new().width(px(80.0)).height(px(40.0)))
        .fill(red())
        .build()
}

#[test]
fn popups_fade_in_and_stay_mounted_until_their_exit_settles() {
    let mut rig = Rig::new(popup_runtime());
    rig.app_frame(0.0);
    let owner = rig.runtime.tree().root().unwrap();
    let popup = rig
        .runtime
        .open_popup(
            owner,
            popup_content(),
            aqua_runtime::PopupOptions::menu()
                .at(aqua_core::Point::new(10.0, 10.0))
                .presence(Presence::new().enter(FADE).exit(FADE)),
        )
        .unwrap();
    rig.app_frame(1.0);
    assert!(rig.opacities().iter().any(|o| *o < 0.1));
    rig.render_only(1.5);
    let mid = rig.opacities();
    assert!(mid.iter().any(|o| *o > 0.2 && *o < 0.8));
    rig.render_only(2.5);
    assert!(rig.opacities().iter().all(|o| *o == 1.0));

    assert!(rig.runtime.close_popup(popup));
    rig.app_frame(3.0);
    assert_eq!(rig.fills().len(), 1);
    assert_eq!(rig.runtime.host().lingering_popups(), 1);
    rig.render_only(3.5);
    assert!(rig.opacities().iter().any(|o| *o > 0.2 && *o < 0.8));
    assert_eq!(rig.fills().len(), 1);

    rig.render_only(5.0);
    rig.app_frame(5.1);
    rig.app_frame(5.2);
    assert_eq!(rig.runtime.host().lingering_popups(), 0);
    assert!(rig.fills().is_empty());
    assert!(rig.render.tracks().is_idle());
}

#[test]
fn popups_without_presence_close_at_once() {
    let mut rig = Rig::new(popup_runtime());
    rig.app_frame(0.0);
    let owner = rig.runtime.tree().root().unwrap();
    let popup = rig
        .runtime
        .open_popup(
            owner,
            popup_content(),
            aqua_runtime::PopupOptions::menu().at(aqua_core::Point::new(10.0, 10.0)),
        )
        .unwrap();
    rig.app_frame(1.0);
    assert_eq!(rig.fills().len(), 1);
    assert!(rig.runtime.close_popup(popup));
    rig.app_frame(2.0);
    assert!(rig.fills().is_empty());
    assert_eq!(rig.runtime.host().lingering_popups(), 0);
}

#[test]
fn frame_animation_moves_only_the_edge_that_moved_on_the_render_side() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(300.0, 200.0, 1.0);
    let (wide, set_wide) = runtime.scope().signal(false);
    runtime
        .mount_reactive((wide,), |(wide,)| {
            UiBox::new()
                .style(
                    Style::new()
                        .axis(Axis::Column)
                        .width(px(300.0))
                        .height(px(200.0)),
                )
                .child(
                    UiBox::new()
                        .style(
                            Style::new()
                                .width(px(if wide { 200.0 } else { 100.0 }))
                                .height(px(40.0)),
                        )
                        .fill(red())
                        .animate(Animate::new().frame(FADE))
                        .build(),
                )
                .build()
        })
        .unwrap();
    let mut rig = Rig::new(runtime);
    rig.app_frame(0.0);
    rig.app_frame(0.016);
    assert_eq!(rig.rect_widths(), vec![100.0]);

    set_wide.set(true);
    rig.app_frame(1.0);
    rig.render_only(1.5);
    let mid = rig.rect_widths()[0];
    assert!(mid > 120.0 && mid < 180.0, "{mid}");
    let edges: Vec<_> = rig
        .commands
        .iter()
        .filter(|command| {
            matches!(
                command,
                RenderCommand::Motion(aqua_motion::MotionCommand::Bind { .. })
            )
        })
        .collect();
    assert_eq!(edges.len(), 1);
    rig.render_only(3.0);
    assert_eq!(rig.rect_widths(), vec![200.0]);
    assert!(rig.render.tracks().is_idle());
}

#[test]
fn translate_and_text_color_changes_are_presented_by_the_render_side() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(300.0, 200.0, 1.0);
    let (moved, set_moved) = runtime.scope().signal(false);
    runtime
        .mount_reactive((moved,), |(moved,)| {
            let shift = if moved { 80.0 } else { 0.0 };
            UiBox::new()
                .style(
                    Style::new()
                        .axis(Axis::Column)
                        .width(px(300.0))
                        .height(px(200.0))
                        .transform(aqua_core::Transform2D::translate(shift, 0.0)),
                )
                .animate(Animate::new().translate(FADE))
                .child(
                    aqua_ui::element::ui::Text::new("hello")
                        .color(if moved { blue() } else { red() })
                        .animate(Animate::new().text_color(FADE))
                        .build(),
                )
                .build()
        })
        .unwrap();
    let mut rig = Rig::new(runtime);
    rig.app_frame(0.0);
    rig.app_frame(0.016);
    let translations = |rig: &Rig| -> Vec<f32> {
        rig.render
            .scene()
            .ordered()
            .filter_map(|(_, item)| match item {
                DisplayItem::PushTransform(t) => Some(t.tx),
                _ => None,
            })
            .collect()
    };
    let text_colors = |rig: &Rig| -> Vec<Oklch> {
        rig.render
            .scene()
            .ordered()
            .filter_map(|(_, item)| match item {
                DisplayItem::Text { color, .. } => Some(*color),
                _ => None,
            })
            .collect()
    };
    assert_eq!(translations(&rig), vec![0.0]);
    assert_eq!(text_colors(&rig), vec![red()]);

    set_moved.set(true);
    rig.app_frame(1.0);
    rig.render_only(1.5);
    let mid = translations(&rig)[0];
    assert!(mid > 20.0 && mid < 60.0, "{mid}");
    let color = text_colors(&rig)[0];
    assert!(color != red() && color != blue());
    rig.render_only(3.0);
    assert_eq!(translations(&rig), vec![80.0]);
    assert_eq!(text_colors(&rig), vec![blue()]);
}

#[test]
fn rows_with_presence_fade_in_even_when_the_first_frame_had_none() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(300.0, 200.0, 1.0);
    let (rows, set_rows) = runtime.scope().signal(Vec::<u32>::new());
    runtime
        .mount_reactive((rows,), |(rows,)| {
            UiBox::new()
                .style(
                    Style::new()
                        .axis(Axis::Column)
                        .width(px(300.0))
                        .height(px(200.0)),
                )
                .children(rows.iter().map(|id| {
                    UiBox::new()
                        .style(Style::new().width(px(100.0)).height(px(20.0)))
                        .fill(red())
                        .presence(Presence::new().enter(FADE))
                        .build()
                        .with_key(*id)
                }))
                .build()
        })
        .unwrap();
    let mut rig = Rig::new(runtime);
    rig.app_frame(0.0);
    set_rows.set(vec![1]);
    rig.app_frame(1.0);
    assert!(rig.opacities().iter().all(|o| *o < 0.05));
    rig.render_only(3.0);
    assert!(rig.opacities().iter().all(|o| *o == 1.0));
}
