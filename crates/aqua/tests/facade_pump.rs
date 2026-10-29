//! Verification: facade orchestration over real peer contracts.
//!
//! Headless tests against the public low-level `aqua` facade API (`Aqua`,
//! `AquaConfig`, `Application`, `AppCtx`): window registration without winit/GPU,
//! event/input dispatch driving the platform scheduler, close removing the
//! window, scene commits into the render-owned `Scene`, and config-to-
//! scheduler wiring. No duplicate scheduler/reactive/text/overlay systems
//! may live here — the facade consumes `aqua-platform` / `aqua-reactive` /
//! `aqua-render` types directly (compile-time proof: these tests name the
//! peer types through facade calls).

use aqua::{AppCtx, Application, Aqua, AquaConfig, Instant};
use aqua_core::{
    InputEvent, Key, KeyEvent, Modifiers, NamedKey, Oklch, Point, PointerButton, PointerEvent,
    PointerKind, Rect,
};
use aqua_platform::geometry::LogicalSize;
use aqua_platform::{AquaWindowId, FrameReason, ObservedWindowState, WindowEvent};
use aqua_render::Scene;

fn observed(id: u64) -> ObservedWindowState {
    ObservedWindowState {
        id: AquaWindowId::new(id).unwrap(),
        title: format!("test-{id}"),
        inner_size: LogicalSize::new(800.0, 600.0),
        scale_factor: 1.0,
        focused: false,
        visible: true,
        occluded: false,
    }
}

struct Probe {
    window_events: Vec<WindowEvent>,
    inputs: Vec<String>,
    scenes_built: u64,
    quit_after: Option<u64>,
    scene: Scene,
}

impl Probe {
    fn new() -> Self {
        Self {
            window_events: Vec::new(),
            inputs: Vec::new(),
            scenes_built: 0,
            quit_after: None,
            scene: Scene::new(),
        }
    }
}

impl Application for Probe {
    fn on_window_event(&mut self, _ctx: &mut AppCtx, _id: AquaWindowId, event: &WindowEvent) {
        self.window_events.push(event.clone());
    }
    fn on_input(&mut self, ctx: &mut AppCtx, _id: AquaWindowId, event: &InputEvent) {
        self.inputs.push(format!("{event:?}"));
        if let Some(n) = self.quit_after {
            if self.inputs.len() as u64 >= n {
                ctx.quit();
            }
        }
    }
    fn build_scene(&mut self, _window: &ObservedWindowState, _now: Instant) -> Option<Instant> {
        self.scenes_built += 1;
        let fill = Oklch::from_hex("#ff0000").unwrap();
        self.scene.insert(aqua_core::DisplayItem::Rect {
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
            radius: 0.0,
            fill,
            border_width: 0.0,
            border_color: fill,
            shadow: None,
        });
        None
    }
    fn scene_mut(&mut self, _id: AquaWindowId) -> Option<&mut Scene> {
        Some(&mut self.scene)
    }
}

fn press_at(x: f64, y: f64) -> InputEvent {
    InputEvent::PointerDown(
        PointerEvent {
            contact: 0,
            kind: PointerKind::Mouse,
            position: Point::new(x as f32, y as f32),
            buttons: 1,
        },
        PointerButton::Primary,
    )
}

#[test]
fn headless_window_dispatch_drives_scheduler() {
    let mut aqua = Aqua::new(AquaConfig::default()).unwrap();
    let mut app = Probe::new();
    let id = aqua.add_headless_window(observed(1));
    assert_eq!(aqua.window_count(), 1);

    aqua.dispatch_window_event(
        &mut app,
        id,
        &WindowEvent::Resized(id, LogicalSize::new(1024.0, 768.0)),
    );
    assert_eq!(app.window_events.len(), 1);
    assert!(aqua.scheduler().has_pending());
    assert_eq!(
        aqua.scheduler().pending_reasons(id).unwrap(),
        FrameReason::RESIZE
    );

    aqua.dispatch_input(&mut app, id, &press_at(10.0, 10.0));
    assert_eq!(app.inputs.len(), 1);
    assert!(aqua
        .scheduler()
        .pending_reasons(id)
        .unwrap()
        .contains(FrameReason::INPUT));
}

#[test]
fn close_requested_removes_window() {
    let mut aqua = Aqua::new(AquaConfig::default()).unwrap();
    let mut app = Probe::new();
    let id = aqua.add_headless_window(observed(2));
    aqua.dispatch_window_event(&mut app, id, &WindowEvent::CloseRequested(id));
    assert_eq!(aqua.window_count(), 0);
    assert_eq!(aqua.render_error_count(), 0);
}

#[test]
fn app_builds_scene_into_render_owned_store() {
    let mut app = Probe::new();
    let obs = observed(3);
    let deadline = app.build_scene(&obs, Instant::ZERO);
    assert_eq!(deadline, None, "settled app requests no deadline");
    assert_eq!(app.scenes_built, 1);
    assert_eq!(app.scene.len(), 1);
    let rev = app.scene.revision();
    let _ = app.build_scene(&obs, Instant::ZERO);
    let scene = &mut app.scene;
    let _ = scene.drain_changes();
    let target = scene.ordered().next().unwrap().0;
    let item = scene.get(target).unwrap().clone();
    assert!(scene.update(target, item));
    assert_eq!(scene.revision(), rev + 1, "only the insert bumped");
}

#[test]
fn key_input_reaches_app_as_canonical_event() {
    let mut aqua = Aqua::new(AquaConfig::default()).unwrap();
    let mut app = Probe::new();
    let id = aqua.add_headless_window(observed(4));
    let esc = InputEvent::KeyDown(KeyEvent {
        key: Key::Named(NamedKey::Escape),
        modifiers: Modifiers::NONE,
        repeat: false,
    });
    aqua.dispatch_input(&mut app, id, &esc);
    assert_eq!(app.inputs.len(), 1);
    assert!(app.inputs[0].contains("Escape"));
}

#[test]
fn config_fps_flows_straight_to_scheduler() {
    let mut cfg = AquaConfig::default();
    assert!(cfg.set_fps(0).is_err(), "zero target rejected");
    cfg.set_fps(120).unwrap();
    let aqua = Aqua::new(cfg).unwrap();
    assert_eq!(aqua.scheduler().fps_target().unwrap().get(), 120);
}

#[test]
fn invalid_window_size_rejected_before_any_window_exists() {
    let mut cfg = AquaConfig::default();
    cfg.window.inner_size = LogicalSize::new(0.0, 600.0);
    assert!(Aqua::new(cfg).is_err());
    assert!(Aqua::new(AquaConfig::default()).unwrap().window_count() == 0);
}
