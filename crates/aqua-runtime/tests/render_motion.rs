use aqua_core::{DisplayItem, InputEvent, NodeId, Point, ScrollDelta};
use aqua_motion::{Easing, MotionKind, MotionValue, Property};
use aqua_protocol::{Instant, Machine, RenderCommand, RenderInput, RuntimeInput, RuntimeOutput};
use aqua_runtime::{PopupOptions, RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Scroll};
use aqua_ui::overlay_host::PopupMotion;
use aqua_ui::style::{px, Axis, Style};

mod support;

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

    fn app_frame(&mut self, secs: f64) -> RuntimeOutput {
        let out = support::frame(&mut self.runtime, secs);
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
            self.runtime.step(RuntimeInput::Render(event));
        }
    }

    fn render_only(&mut self, secs: f64) {
        self.deliver(Vec::new(), secs);
    }

    fn wheel(&mut self, secs: f64, lines: f32) -> RuntimeOutput {
        let out = self.runtime.step(support_input(InputEvent::Scroll {
            at: Point::new(20.0, 20.0),
            delta: ScrollDelta {
                x: 0.0,
                y: 16.0 * lines,
            },
        }));
        self.app_frame(secs);
        out
    }

    fn translations(&self) -> Vec<f32> {
        self.render
            .scene()
            .ordered()
            .filter_map(|(_, item)| match item {
                DisplayItem::PushTransform(t) => Some(t.ty),
                _ => None,
            })
            .collect()
    }

    fn opacities(&self) -> Vec<f32> {
        self.render
            .scene()
            .ordered()
            .filter_map(|(_, item)| match item {
                DisplayItem::PushOpacity(o) => Some(*o),
                _ => None,
            })
            .collect()
    }
}

fn support_input(event: InputEvent) -> RuntimeInput {
    RuntimeInput::Shell(aqua_protocol::ShellEvent::Input {
        window: support::window(),
        event,
    })
}

fn scroller() -> Runtime {
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
                                .height(px(2000.0))
                                .flex_shrink(0.0),
                        )
                        .build(),
                )
                .build()
        })
        .unwrap();
    support::resize(&mut runtime, 300.0, 200.0, 1.0);
    runtime
}

fn offset(rig: &Rig) -> f32 {
    let root = rig.runtime.tree().root().unwrap();
    rig.runtime.tree().scroll_offset(root).unwrap().y
}

#[test]
fn mounting_starts_no_motion() {
    let mut rig = Rig::new(scroller());
    rig.app_frame(0.0);
    rig.app_frame(0.016);
    assert!(rig.render.tracks().is_idle());
    assert_eq!(rig.render.tracks().len(), 0);
    assert!(rig.translations().iter().all(|ty| *ty == 0.0));
    assert_eq!(rig.translations().len(), 2);
}

#[test]
fn wheel_moves_the_logical_offset_at_once_and_the_render_side_glides_alone() {
    let mut rig = Rig::new(scroller());
    rig.app_frame(0.0);
    rig.wheel(1.0, 1.0);
    assert!((offset(&rig) - 77.7).abs() < 1e-3);

    let mut previous = f32::MAX;
    let mut samples = Vec::new();
    for step in 0..240 {
        rig.render_only(1.0 + f64::from(step) * (1.0 / 120.0));
        let ty = rig.translations()[0];
        assert!(
            ty <= previous + 1e-4 && ty >= 0.0,
            "monotonic: {ty} after {previous}"
        );
        previous = ty;
        samples.push(ty);
    }
    assert!(
        samples[0] > 60.0,
        "starts near the full distance: {}",
        samples[0]
    );
    assert!(samples[10] < samples[0] * 0.8);
    assert_eq!(*samples.last().unwrap(), 0.0);
    assert!(rig.render.tracks().is_idle());
    assert!((offset(&rig) - 77.7).abs() < 1e-3);
}

#[test]
fn a_second_notch_mid_glide_continues_from_the_presented_position() {
    let mut rig = Rig::new(scroller());
    rig.app_frame(0.0);
    rig.wheel(1.0, 1.0);
    rig.deliver(
        vec![RenderCommand::Viewport {
            size: aqua_core::Size::new(300.0, 200.0),
            scale: 1.25,
        }],
        1.0,
    );
    for step in 1..=12 {
        rig.render_only(1.0 + f64::from(step) / 120.0);
    }
    let before = rig.translations()[0];
    assert!(before > 5.0 && before < 77.0);
    rig.wheel(1.1, 1.0);
    let after = rig.translations()[0];
    assert!(
        (after - (before + 77.7)).abs() < 1.0,
        "{before} then {after}: the presented content must not jump"
    );
    assert!((offset(&rig) - 155.4).abs() < 1e-2);
}

#[test]
fn reduced_motion_scrolls_without_a_glide() {
    let mut rig = Rig::new(scroller());
    rig.runtime.apply_startup_motion_policy(true);
    rig.app_frame(0.0);
    rig.wheel(1.0, 1.0);
    rig.render_only(1.0);
    assert_eq!(rig.translations()[0], 0.0);
    assert!(rig.render.tracks().is_idle());
    assert!((offset(&rig) - 77.7).abs() < 1e-3);
}

#[test]
fn removing_the_scroll_node_disposes_its_track() {
    let mut rig = Rig::new(scroller());
    rig.app_frame(0.0);
    rig.wheel(1.0, 1.0);
    assert_eq!(rig.render.tracks().len(), 1);
    let root = rig.runtime.tree().root().unwrap();
    rig.runtime.tree_mut().remove(root);
    rig.app_frame(1.2);
    assert_eq!(rig.render.tracks().len(), 0);
}

fn popup_rig() -> (Rig, aqua_ui::overlay::OverlayId, NodeId) {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(300.0, 200.0, 1.0);
    runtime.mount(|_| UiBox::new().build()).unwrap();
    support::resize(&mut runtime, 300.0, 200.0, 1.0);
    runtime.build_frame().unwrap();
    let owner = runtime.tree().root().unwrap();
    let popup = runtime
        .open_popup(
            owner,
            UiBox::new()
                .style(
                    Style::new()
                        .width(px(80.0))
                        .height(px(40.0))
                        .background(aqua_core::Oklch::from_hex("#334455").unwrap()),
                )
                .build(),
            PopupOptions::menu().at(Point::new(10.0, 20.0)),
        )
        .unwrap();
    (Rig::new(runtime), popup, owner)
}

fn fade() -> PopupMotion {
    PopupMotion {
        property: Property::Opacity,
        from: MotionValue::Scalar(0.0),
        to: MotionValue::Scalar(1.0),
        kind: MotionKind::Tween {
            duration_secs: 0.2,
            easing: Easing::Linear,
        },
    }
}

#[test]
fn popup_fade_runs_on_the_render_side_and_completes_through_the_app() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let (mut rig, popup, _) = popup_rig();
    rig.app_frame(0.0);
    assert!(rig.runtime.animate_popup(popup, fade()));
    let done: Rc<RefCell<Vec<aqua_motion::Completion>>> = Rc::default();
    rig.app_frame(1.0);
    let track = rig
        .runtime
        .host()
        .popup_track(popup, Property::Opacity)
        .unwrap();
    let sink = Rc::clone(&done);
    rig.runtime
        .host_mut()
        .tracks_mut()
        .on_complete(track, Box::new(move |c| sink.borrow_mut().push(c)));
    rig.runtime
        .host_mut()
        .tracks_mut()
        .set_target(track, MotionValue::Scalar(1.0), 1.0)
        .unwrap();
    let out = rig.app_frame(1.0);
    assert!(
        out.wake_at.is_none(),
        "the app side schedules nothing while a popup fades"
    );

    assert_eq!(rig.opacities().len(), 1);
    let mut last = -1.0;
    for step in 1..=30 {
        rig.render_only(1.0 + f64::from(step) * 0.01);
        let opacity = rig.opacities()[0];
        assert!(opacity >= last, "monotonic fade");
        last = opacity;
    }
    assert!(last > 0.9);
    rig.render_only(1.5);
    assert_eq!(rig.opacities()[0], 1.0);
    assert_eq!(*done.borrow(), vec![aqua_motion::Completion::Completed]);
}

#[test]
fn popup_first_frame_shows_the_starting_value_not_a_flash_of_the_target() {
    let (mut rig, popup, _) = popup_rig();
    rig.app_frame(0.0);
    rig.runtime.animate_popup(popup, fade());
    rig.app_frame(1.0);
    assert_eq!(rig.opacities(), vec![0.0]);
}

#[test]
fn closing_a_popup_removes_its_track_and_group() {
    use aqua_motion::MotionCommand;

    let (mut rig, popup, _) = popup_rig();
    rig.app_frame(0.0);
    rig.runtime.animate_popup(popup, fade());
    rig.app_frame(1.0);
    assert_eq!(rig.render.tracks().len(), 1);
    let track = rig
        .runtime
        .host()
        .popup_track(popup, Property::Opacity)
        .unwrap();
    assert!(rig.runtime.close_popup(popup));
    assert_eq!(rig.runtime.host().popup_wraps().count(), 0);
    assert_eq!(
        rig.runtime.host_mut().tracks_mut().drain_commands(),
        vec![MotionCommand::Remove { track }]
    );
}

#[test]
fn a_wheel_notch_inside_the_overscan_window_patches_one_scene_item() {
    let mut rig = Rig::new(scroller());
    rig.app_frame(0.0);
    rig.app_frame(0.016);
    let out = rig.wheel(1.0, 1.0);
    let _ = out;
    assert!(rig.runtime.frame_work().tree_scene_reused);
    assert_eq!(rig.runtime.frame_work().scene_items_reconciled, 1);
    assert!((offset(&rig) - 77.7).abs() < 1e-3);
    assert!(rig.translations().contains(&-77.7));
}

#[test]
fn a_wheel_past_the_overscan_window_re_emits_the_scene() {
    let mut rig = Rig::new(scroller());
    rig.app_frame(0.0);
    rig.app_frame(0.016);
    for step in 0..6 {
        rig.wheel(1.0 + f64::from(step), 1.0);
    }
    assert!(!rig.runtime.frame_work().tree_scene_reused);
    assert!(rig
        .translations()
        .iter()
        .any(|ty| (*ty + 466.2).abs() < 1e-2));
}
