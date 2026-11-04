use aqua::{AppCtx, Application, Aqua, AquaConfig, Instant};
use aqua_core::InputEvent;
use aqua_protocol::geometry::{AquaWindowId, LogicalSize};
use aqua_protocol::window::ObservedWindowState;
use aqua_protocol::window::WindowEvent;
use aqua_protocol::FrameReason;
use aqua_ui::style::LayoutStyle;
use aqua_ui::tree::{BoxContent, NodeKind, UiTree};

struct Probe {
    tree: UiTree,
    node: aqua_core::NodeId,
    mode: u8,
    other: AquaWindowId,
}

impl Application for Probe {
    fn input_invalidation_epoch(&self, _id: AquaWindowId) -> u64 {
        self.tree.content_version()
    }

    fn on_window_event(&mut self, _: &mut AppCtx, _: AquaWindowId, _: &WindowEvent) {}

    fn on_input(&mut self, ctx: &mut AppCtx, id: AquaWindowId, _: &InputEvent) {
        match self.mode {
            1 => self.tree.mark_layout_dirty(self.node),
            2 => ctx.request_frame(id, FrameReason::EXPLICIT),
            3 => ctx.request_frame(self.other, FrameReason::EXPLICIT),
            _ => {}
        }
    }

    fn on_files_dropped_at(
        &mut self,
        ctx: &mut AppCtx,
        id: AquaWindowId,
        _: &[std::path::PathBuf],
        _: Option<aqua_core::Point>,
    ) {
        self.on_input(ctx, id, &InputEvent::Focused(false));
    }

    fn build_scene(&mut self, _: &ObservedWindowState, _: Instant) -> Option<Instant> {
        None
    }
}

#[test]
fn input_reports_new_invalidations_even_when_tree_is_already_dirty() {
    let mut aqua = Aqua::new(AquaConfig::default()).unwrap();
    let id = AquaWindowId::new(1).unwrap();
    let other = AquaWindowId::new(2).unwrap();
    for id in [id, other] {
        aqua.add_headless_window(ObservedWindowState {
            id,
            title: String::new(),
            inner_size: LogicalSize::new(100.0, 100.0),
            scale_factor: 1.0,
            focused: false,
            visible: true,
            occluded: false,
            maximized: false,
        });
    }
    let mut tree = UiTree::new();
    let node = tree.insert(
        None,
        NodeKind::Box(BoxContent::default()),
        LayoutStyle::default(),
    );
    let mut probe = Probe {
        tree,
        node,
        mode: 0,
        other,
    };
    let event = InputEvent::Focused(true);
    assert!(!aqua.dispatch_input(&mut probe, id, &event));
    probe.mode = 1;
    assert!(aqua.dispatch_input(&mut probe, id, &event));
    assert!(aqua.dispatch_input(&mut probe, id, &event));
    probe.mode = 2;
    assert!(aqua.dispatch_input(&mut probe, id, &event));
    probe.mode = 3;
    assert!(!aqua.dispatch_input(&mut probe, id, &event));
    let paths = [std::path::PathBuf::from("example.txt")];
    probe.mode = 0;
    assert!(!aqua.dispatch_file_drop(&mut probe, id, &paths));
    probe.mode = 1;
    assert!(aqua.dispatch_file_drop(&mut probe, id, &paths));
    probe.mode = 2;
    assert!(aqua.dispatch_file_drop(&mut probe, id, &paths));
    probe.mode = 3;
    assert!(!aqua.dispatch_file_drop(&mut probe, id, &paths));
}
