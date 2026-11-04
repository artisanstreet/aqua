use aqua::{px, Application, Aqua, AquaConfig, Easing, Instant, Motion, Style, UiApp};
use aqua_core::{DisplayItem, Oklch};
use aqua_motion::MotionCommand;
use aqua_protocol::geometry::{AquaWindowId, LogicalSize};
use aqua_protocol::window::{ObservedWindowState, WindowEvent};
use aqua_protocol::{RenderCommand, RenderEvent, RenderInput, RuntimeInput};
use aqua_runtime::{RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::Element;
use aqua_ui::style::Axis;

const FADE: Motion = Motion::tween(1.0, Easing::Linear);

fn observed(id: u64) -> ObservedWindowState {
    ObservedWindowState {
        id: AquaWindowId::new(id).unwrap(),
        title: format!("window-{id}"),
        inner_size: LogicalSize::new(300.0, 200.0),
        scale_factor: 1.0,
        focused: false,
        visible: true,
        occluded: false,
        maximized: false,
    }
}

fn red() -> Oklch {
    Oklch::from_hex("#ff0000").unwrap()
}

fn blue() -> Oklch {
    Oklch::from_hex("#0000ff").unwrap()
}

fn runtime() -> Runtime {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(300.0, 200.0, 1.0);
    runtime
}

fn root(children: impl IntoIterator<Item = Element>) -> Element {
    Aqua::Box()
        .style(
            Style::new()
                .axis(Axis::Column)
                .width(px(300.0))
                .height(px(200.0)),
        )
        .children(children)
        .build()
}

struct Rig {
    app: UiApp,
    render: RenderSide,
    window: AquaWindowId,
    commands: Vec<RenderCommand>,
}

impl Rig {
    fn new(app: UiApp) -> Self {
        Self {
            app,
            render: RenderSide::new(),
            window: AquaWindowId::new(1).unwrap(),
            commands: Vec::new(),
        }
    }

    fn frame(&mut self, secs: f64) {
        self.app
            .build_scene(&observed(self.window.raw()), Instant::from_secs_f64(secs));
        let output = self.app.take_output(self.window);
        self.commands.extend(output.render.clone());
        self.deliver(output.render, secs);
    }

    fn deliver(&mut self, commands: Vec<RenderCommand>, secs: f64) {
        let output = self.render.step(RenderInput {
            commands,
            feedback: Vec::new(),
            present_at: Instant::from_secs_f64(secs),
        });
        for event in output.events {
            if matches!(event, RenderEvent::Motion(_)) {
                self.app
                    .step_input(self.window, RuntimeInput::Render(event));
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

    fn retargets_since(&self, from: usize) -> usize {
        self.commands[from..]
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    RenderCommand::Motion(MotionCommand::SetTarget { .. })
                )
            })
            .count()
    }
}

#[test]
fn animate_retargets_once_and_the_render_side_interpolates() {
    let mut runtime = runtime();
    let (on, set_on) = runtime.scope().signal(false);
    runtime
        .mount_reactive((on,), |(on,)| {
            root([Aqua::Box()
                .style(Style::new().width(px(100.0)).height(px(40.0)))
                .fill(if on { blue() } else { red() })
                .animate(Aqua::Animate().fill(FADE))
                .build()])
        })
        .unwrap();
    let mut rig = Rig::new(UiApp::from_runtime(runtime));
    rig.frame(0.0);
    rig.frame(0.016);
    assert_eq!(rig.fills(), vec![red()]);
    assert!(rig.render.tracks().is_idle());

    set_on.set(true);
    let before = rig.commands.len();
    rig.frame(1.0);
    assert_eq!(rig.retargets_since(before), 1);

    rig.render_only(1.5);
    let mid = rig.fills()[0];
    assert!(mid != red() && mid != blue());

    let settled = rig.commands.len();
    rig.render_only(3.0);
    assert_eq!(rig.commands.len(), settled);
    assert_eq!(rig.fills(), vec![blue()]);
    assert!(rig.render.tracks().is_idle());
}

#[test]
fn nodes_without_animate_change_instantly() {
    let mut runtime = runtime();
    let (on, set_on) = runtime.scope().signal(false);
    runtime
        .mount_reactive((on,), |(on,)| {
            root([Aqua::Box()
                .style(Style::new().width(px(100.0)).height(px(40.0)))
                .fill(if on { blue() } else { red() })
                .build()])
        })
        .unwrap();
    let mut rig = Rig::new(UiApp::from_runtime(runtime));
    rig.frame(0.0);
    set_on.set(true);
    let before = rig.commands.len();
    rig.frame(1.0);
    assert_eq!(rig.retargets_since(before), 0);
    assert_eq!(rig.fills(), vec![blue()]);
}

fn list_app() -> (UiApp, aqua_reactive::WriteSignal<Vec<u32>>) {
    let mut runtime = runtime();
    let (rows, set_rows) = runtime.scope().signal(vec![1u32, 2, 3]);
    runtime
        .mount_reactive((rows,), |(rows,)| {
            root(rows.iter().map(|id| {
                Aqua::Box()
                    .style(Style::new().width(px(100.0)).height(px(20.0)))
                    .fill(red())
                    .presence(Aqua::Presence().enter(FADE).exit(FADE))
                    .build()
                    .with_key(*id)
            }))
        })
        .unwrap();
    (UiApp::from_runtime(runtime), set_rows)
}

#[test]
fn presence_keeps_exiting_rows_until_the_fade_settles() {
    let (app, set_rows) = list_app();
    let mut rig = Rig::new(app);
    rig.frame(0.0);
    rig.frame(0.016);
    assert_eq!(rig.fills().len(), 3);

    set_rows.set(vec![1, 3]);
    rig.frame(1.0);
    assert_eq!(rig.fills().len(), 3);
    assert!(rig.app.tree().exiting_nodes().len() == 1);
    rig.render_only(1.5);
    assert!(rig.opacities().iter().any(|o| *o > 0.0 && *o < 1.0));

    rig.render_only(3.0);
    rig.frame(3.1);
    rig.frame(3.2);
    assert_eq!(rig.fills().len(), 2);
    assert!(rig.app.tree().exiting_nodes().is_empty());
}

#[test]
fn presence_enter_skips_first_mount_and_fades_later_arrivals() {
    let (app, set_rows) = list_app();
    let mut rig = Rig::new(app);
    rig.frame(0.0);
    assert!(rig.opacities().iter().all(|o| *o == 1.0));

    set_rows.set(vec![1, 2, 3, 4]);
    rig.frame(1.0);
    assert_eq!(rig.opacities().iter().filter(|o| **o < 0.05).count(), 1);
    rig.render_only(3.0);
    assert!(rig.opacities().iter().all(|o| *o == 1.0));
}

#[test]
fn motion_kit_tokens_resolve_through_a_theme() {
    const SLOW: aqua::MotionToken = aqua::MotionToken::new("slow");
    let quick = Aqua::Motion().tween(0.1, Easing::EaseOut);
    let springy = Aqua::Motion().spring(aqua::SpringParams::SNAPPY);
    let theme = Aqua::Motion().theme(Motion::INSTANT).with(SLOW, quick);
    assert_eq!(theme.resolve(SLOW.into()), quick);
    assert_eq!(theme.resolve(springy.into()), springy);
    assert_eq!(Aqua::Motion().token("slow"), SLOW);
    let retuned = theme.clone().with(SLOW, springy);
    assert_eq!(retuned.resolve(SLOW.into()), springy);
    assert_ne!(theme, retuned);
}

#[test]
fn every_window_gets_its_own_runtime() {
    let first = AquaWindowId::new(1).unwrap();
    let second = AquaWindowId::new(2).unwrap();
    let mut first_runtime = runtime();
    first_runtime
        .mount(|_| root([Aqua::Text("first").build()]))
        .unwrap();
    let mut app = UiApp::from_runtime(first_runtime).with_window_factory(|_| {
        let mut runtime = runtime();
        runtime.mount(|_| root([Aqua::Text("second").build()]))?;
        Ok(runtime)
    });
    let mut aqua = Aqua::new(AquaConfig::default()).unwrap();
    aqua.add_headless_window(observed(1));
    aqua.add_headless_window(observed(2));
    aqua.dispatch_start(&mut app, first);
    aqua.dispatch_start(&mut app, second);
    assert_eq!(app.window_count(), 2);

    app.build_scene(&observed(1), Instant::from_secs_f64(0.0));
    app.build_scene(&observed(2), Instant::from_secs_f64(0.0));
    let texts = |id| -> Vec<String> {
        let runtime = app.runtime_for(id).unwrap();
        runtime
            .tree()
            .traverse()
            .into_iter()
            .filter_map(
                |node| match runtime.tree().get(node).map(|record| &record.kind) {
                    Some(aqua_ui::tree::NodeKind::Text(text)) => Some(text.text().to_string()),
                    _ => None,
                },
            )
            .collect()
    };
    assert_eq!(texts(first), vec!["first".to_string()]);
    assert_eq!(texts(second), vec!["second".to_string()]);
    assert!(!app.take_output(first).render.is_empty());
    assert!(!app.take_output(second).render.is_empty());
}

#[test]
fn destroying_a_window_disposes_only_its_runtime() {
    let first = AquaWindowId::new(1).unwrap();
    let second = AquaWindowId::new(2).unwrap();
    let mut first_runtime = runtime();
    first_runtime.mount(|_| root([])).unwrap();
    let mut app = UiApp::from_runtime(first_runtime).with_window_factory(|_| {
        let mut runtime = runtime();
        runtime.mount(|_| root([]))?;
        Ok(runtime)
    });
    let mut aqua = Aqua::new(AquaConfig::default()).unwrap();
    aqua.add_headless_window(observed(1));
    aqua.add_headless_window(observed(2));
    aqua.dispatch_start(&mut app, first);
    aqua.dispatch_start(&mut app, second);
    assert_eq!(app.window_count(), 2);

    aqua.dispatch_window_event(&mut app, second, &WindowEvent::CloseRequested(second));
    assert_eq!(app.window_count(), 1);
    assert!(app.runtime_for(second).is_none());
    assert!(app.runtime_for(first).is_some());

    aqua.dispatch_window_event(&mut app, first, &WindowEvent::Destroyed(first));
    assert_eq!(app.window_count(), 0);
}
