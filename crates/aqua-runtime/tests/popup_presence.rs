use aqua_core::{DisplayItem, NodeId, Point};
use aqua_motion::Easing;
use aqua_protocol::{Instant, Machine, RenderCommand, RenderInput, RuntimeInput};
use aqua_runtime::{PopupOptions, RenderSide, Runtime};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::Box as UiBox;
use aqua_ui::motion::{Motion, Presence};
use aqua_ui::style::{px, Style};

mod support;

const SLIDE: f32 = 12.0;
const TWEEN: Motion = Motion::tween(0.2, Easing::Linear);

struct Rig {
    runtime: Runtime,
    render: RenderSide,
}

impl Rig {
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

    fn app_frame(&mut self, secs: f64) {
        let out = support::frame(&mut self.runtime, secs);
        self.deliver(out.render, secs);
    }

    fn render_only(&mut self, secs: f64) {
        self.deliver(Vec::new(), secs);
    }

    fn shown(&self) -> (Vec<f32>, Vec<f32>) {
        let items: Vec<&DisplayItem> = self.render.scene().ordered().map(|(_, i)| i).collect();
        let opacity = items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::PushOpacity(o) => Some(*o),
                _ => None,
            })
            .collect();
        let slide = items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::PushTransform(t) => Some(t.ty),
                _ => None,
            })
            .collect();
        (opacity, slide)
    }

    fn exposed(&self, node: NodeId) -> bool {
        let (_, _, nodes) = self.runtime.a11y_nodes().unwrap();
        nodes
            .iter()
            .any(|a| a.semantic.node == node || a.children.contains(&node))
    }
}

fn rig_with_popup() -> (Rig, aqua_ui::overlay::OverlayId, NodeId) {
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
            PopupOptions::menu()
                .at(Point::new(10.0, 20.0))
                .presence(Presence::new().enter(TWEEN).exit(TWEEN).slide(0.0, SLIDE)),
        )
        .unwrap();
    let root = runtime.host().popup_root(popup).unwrap();
    (
        Rig {
            runtime,
            render: RenderSide::new(),
        },
        popup,
        root,
    )
}

fn close(enter_time: f64) -> (Rig, NodeId) {
    let (mut rig, popup, root) = rig_with_popup();
    rig.app_frame(0.0);
    rig.app_frame(enter_time);
    rig.render_only(enter_time + 0.5);
    assert!(rig.runtime.close_popup(popup));
    (rig, root)
}

#[test]
fn opening_slides_and_fades_in_together_and_lands_exactly() {
    let (mut rig, _, _) = rig_with_popup();
    rig.app_frame(0.0);
    assert_eq!(rig.shown(), (vec![0.0], vec![SLIDE]));
    rig.render_only(0.1);
    let (opacity, slide) = rig.shown();
    assert!((opacity[0] - 0.5).abs() < 1e-3, "{opacity:?}");
    assert!((slide[0] - SLIDE / 2.0).abs() < 1e-3, "{slide:?}");
    rig.render_only(0.5);
    assert_eq!(rig.shown(), (vec![1.0], vec![0.0]));
    assert!(rig.render.tracks().is_idle());
}

#[test]
fn closing_slides_and_fades_out_then_removes_the_popup() {
    let (mut rig, root) = close(1.0);
    rig.app_frame(2.0);
    rig.render_only(2.1);
    let (opacity, slide) = rig.shown();
    assert!((opacity[0] - 0.5).abs() < 1e-3, "{opacity:?}");
    assert!((slide[0] - SLIDE / 2.0).abs() < 1e-3, "{slide:?}");
    rig.render_only(2.5);
    rig.app_frame(2.6);
    assert!(rig.runtime.tree().get(root).is_none());
    assert_eq!(rig.shown(), (vec![], vec![]));
    assert!(rig.render.tracks().is_empty());
}

#[test]
fn a_closing_popup_leaves_the_accessibility_tree_at_once() {
    let (mut rig, popup, root) = rig_with_popup();
    rig.app_frame(0.0);
    rig.app_frame(1.0);
    rig.render_only(1.5);
    assert!(rig.exposed(root));
    assert!(rig.runtime.close_popup(popup));
    assert!(rig.runtime.tree().get(root).is_some());
    assert!(!rig.exposed(root));
    rig.app_frame(2.0);
    assert!(rig.runtime.tree().get(root).is_some());
    assert!(!rig.exposed(root));
}
