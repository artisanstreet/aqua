use aqua::render::host::FrameTarget;
use aqua::{AppCtx, Application, Aqua, AquaConfig};
use aqua_core::{DisplayItem, Oklch, Rect};
use aqua_protocol::geometry::LogicalSize;
use aqua_protocol::window::ObservedWindowState;
use aqua_protocol::{Feedback, FrameId, FrameReason, Instant, WindowId};
use aqua_runtime::RenderSide;
use aqua_scene::Scene;
use std::time::{Duration, Instant as WallInstant};

struct Animation {
    frames: usize,
    scene: Scene,
}

impl Application for Animation {
    fn on_window_event(
        &mut self,
        _ctx: &mut AppCtx<'_>,
        _id: WindowId,
        _event: &aqua_protocol::window::WindowEvent,
    ) {
    }
    fn on_input(&mut self, _ctx: &mut AppCtx<'_>, _id: WindowId, _event: &aqua_core::InputEvent) {}
    fn build_scene(&mut self, _window: &ObservedWindowState, _now: Instant) -> Option<Instant> {
        None
    }

    fn on_start(&mut self, ctx: &mut AppCtx<'_>, id: WindowId) {
        ctx.request_frame(id, FrameReason::ANIMATION);
    }

    fn on_animation_frame(&mut self, ctx: &mut AppCtx<'_>, id: WindowId, _now: Instant) {
        self.frames += 1;
        if self.frames < 8 {
            ctx.request_frame(id, FrameReason::ANIMATION);
        }
    }

    fn scene_mut(&mut self, _id: WindowId) -> Option<&mut Scene> {
        Some(&mut self.scene)
    }
}

struct Target;

impl FrameTarget for Target {
    fn wait_frame(&mut self) {}
    fn present(&mut self, _side: &mut RenderSide, frame: FrameId) -> Vec<Feedback> {
        vec![Feedback::Presented(frame)]
    }
}

#[test]
fn unchanged_scenes_do_not_stop_application_animation() {
    let mut aqua = Aqua::new(AquaConfig::default()).unwrap();
    let id = aqua.add_headless_window(ObservedWindowState {
        id: WindowId::new(1).unwrap(),
        title: String::new(),
        inner_size: LogicalSize::new(100.0, 100.0),
        scale_factor: 1.0,
        focused: true,
        visible: true,
        occluded: false,
        maximized: false,
    });
    let mut scene = Scene::new();
    scene.insert(DisplayItem::Rect {
        bounds: Rect::from_edges(0.0, 0.0, 10.0, 10.0),
        radius: 0.0,
        fill: Oklch::WHITE,
        border_width: 0.0,
        border_color: Oklch::TRANSPARENT,
        shadow: None,
    });
    let mut app = Animation { frames: 0, scene };
    assert!(aqua.attach_render(
        id,
        Box::new(|_| Box::new(|| { Ok(Box::new(Target) as Box<dyn FrameTarget>) })),
        Duration::ZERO
    ));
    aqua.dispatch_start(&mut app, id);
    let deadline = WallInstant::now() + Duration::from_millis(500);
    while app.frames < 8 && WallInstant::now() < deadline {
        aqua.pump_inbox(&mut app);
        aqua.draw_ready(&mut app, aqua.clock());
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        app.frames, 8,
        "animation must advance without mouse input or scene mutations"
    );
}
