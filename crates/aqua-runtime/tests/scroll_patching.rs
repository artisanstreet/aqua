use std::cell::{Cell, RefCell};
use std::rc::Rc;

use aqua_core::{DisplayItem, Oklch, Point, Rect};
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Canvas, Scroll};
use aqua_ui::element::Element;
use aqua_ui::style::{px, Axis, Style};
use aqua_ui::tree::NodeKind;

#[derive(Clone, Copy)]
enum Probe {
    Legacy,
    ScrollStable,
    Observer,
}

fn fill() -> DisplayItem {
    DisplayItem::Rect {
        bounds: Rect::from_edges(0.0, 0.0, 10.0, 10.0),
        radius: 0.0,
        fill: Oklch::WHITE,
        border_width: 0.0,
        border_color: Oklch::TRANSPARENT,
        shadow: None,
    }
}

fn content(probe: Probe, seen: Rc<RefCell<Vec<Rect>>>, calls: Rc<Cell<u32>>) -> Element {
    let canvas = Canvas::new().style(Style::new().width(px(40.0)).height(px(40.0)));
    let canvas = match probe {
        Probe::Legacy => canvas.on_paint(|_| vec![fill()]),
        Probe::ScrollStable => canvas.scroll_stable().on_paint(|_| vec![fill()]),
        Probe::Observer => canvas.on_observe(move |tree, node| {
            calls.set(calls.get() + 1);
            if let Some(rect) = tree.pre_transform_rect(node) {
                seen.borrow_mut().push(rect);
            }
        }),
    };
    Scroll::new()
        .style(Style::new().width(px(200.0)).height(px(100.0)))
        .child(
            UiBox::new()
                .style(
                    Style::new()
                        .axis(Axis::Column)
                        .width(px(200.0))
                        .height(px(1000.0))
                        .flex_shrink(0.0),
                )
                .child(canvas.build())
                .build(),
        )
        .build()
}

struct Rig {
    runtime: Runtime,
    scroll: aqua_core::NodeId,
    seen: Rc<RefCell<Vec<Rect>>>,
    calls: Rc<Cell<u32>>,
}

impl Rig {
    fn new(probe: Probe) -> Self {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let calls = Rc::new(Cell::new(0));
        let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
        runtime.set_viewport(200.0, 100.0, 1.0);
        let (seen_in, calls_in) = (seen.clone(), calls.clone());
        runtime
            .mount(move |_| content(probe, seen_in, calls_in))
            .unwrap();
        runtime.build_frame().unwrap();
        let scroll = runtime
            .tree()
            .traverse()
            .into_iter()
            .find(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Scroll(_)))
            .unwrap();
        Self {
            runtime,
            scroll,
            seen,
            calls,
        }
    }

    fn scroll_to(&mut self, y: f32) -> bool {
        let before = self.runtime.tree().scene_version();
        self.runtime
            .tree_mut()
            .scroll_to(self.scroll, Point::new(0.0, y));
        self.runtime.build_frame().unwrap();
        self.runtime.tree().scene_version() != before
    }

    fn reemit_at_current_offset(&mut self) {
        let root = self.runtime.tree().root().unwrap();
        let mut paint = self.runtime.tree().paint(root).unwrap();
        paint.opacity = Some(paint.opacity.map_or(0.99, |value| value - 0.01));
        self.runtime.tree_mut().set_paint(root, paint);
        self.runtime.build_frame().unwrap();
    }

    fn rescrolls_without_reemit(&mut self) -> bool {
        self.scroll_to(10.0);
        self.reemit_at_current_offset();
        !self.scroll_to(20.0)
    }
}

#[test]
fn legacy_canvas_forces_reemit_on_every_scroll() {
    assert!(!Rig::new(Probe::Legacy).rescrolls_without_reemit());
}

#[test]
fn scroll_stable_canvas_keeps_scroll_patching() {
    assert!(Rig::new(Probe::ScrollStable).rescrolls_without_reemit());
}

#[test]
fn observer_canvas_keeps_scroll_patching_and_sees_every_scroll() {
    let mut rig = Rig::new(Probe::Observer);
    assert!(rig.rescrolls_without_reemit());
    let seen = rig.seen.borrow().clone();
    let last = seen.last().copied().unwrap();
    assert_eq!(last.y, 0.0 - 20.0);
    let calls = rig.calls.get();
    rig.scroll_to(30.0);
    assert_eq!(rig.calls.get(), calls + 1);
    assert_eq!(rig.seen.borrow().last().unwrap().y, -30.0);
}

#[test]
fn observers_stay_quiet_when_nothing_changed() {
    let mut rig = Rig::new(Probe::Observer);
    rig.runtime.build_frame().unwrap();
    let calls = rig.calls.get();
    rig.runtime.build_frame().unwrap();
    assert_eq!(rig.calls.get(), calls);
}

fn clipped_rows() -> Element {
    let rows = (0..30).map(|_| {
        UiBox::new()
            .style(
                Style::new()
                    .width(px(200.0))
                    .height(px(20.0))
                    .flex_shrink(0.0),
            )
            .fill(Oklch::WHITE)
            .build()
    });
    UiBox::new()
        .style(Style::new().width(px(200.0)).height(px(100.0)).clip(true))
        .child(
            Scroll::new()
                .style(Style::new().width(px(200.0)).height(px(100.0)))
                .child(
                    UiBox::new()
                        .style(
                            Style::new()
                                .axis(Axis::Column)
                                .width(px(200.0))
                                .flex_shrink(0.0),
                        )
                        .children(rows)
                        .build(),
                )
                .build(),
        )
        .build()
}

#[test]
fn a_patched_scroll_under_a_clipping_parent_still_paints_the_rows_it_reveals() {
    let mut runtime = Runtime::new(SharedFonts::new(), SharedRuns::new());
    runtime.set_viewport(200.0, 100.0, 1.0);
    runtime.mount(|_| clipped_rows()).unwrap();
    runtime.build_frame().unwrap();
    let scroll = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Scroll(_)))
        .unwrap();
    let before = runtime.tree().scene_version();
    runtime.tree_mut().scroll_to(scroll, Point::new(0.0, 60.0));
    runtime.build_frame().unwrap();
    assert_eq!(runtime.tree().scene_version(), before);
    let painted: Vec<f32> = runtime
        .scene()
        .ordered()
        .filter_map(|(_, item)| match item {
            DisplayItem::Rect { bounds, fill, .. } if *fill == Oklch::WHITE => Some(bounds.y),
            _ => None,
        })
        .collect();
    assert!(painted.contains(&140.0), "{painted:?}");
}
