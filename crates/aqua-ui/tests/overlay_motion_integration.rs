//! Combined overlay + motion scenario: a menu opens with an animated
//! presentation value, an outside press dismisses the overlay chain, and the
//! owning node's motions are disposed without firing stale completions.
//!
//! This is the runtime wiring the facade will perform every frame:
//! overlays own placement/dismissal/focus, the motion controller owns
//! animated presentation values and scheduler deadlines.

use aqua_core::id::RawId;
use aqua_core::{NodeId, Point, Rect, Size};
use aqua_ui::motion::{Easing, MotionController, MotionKind, MotionValue, ReducedMotion, Seconds};
use aqua_ui::overlay::{
    Align, DismissPolicy, FocusControl, Modality, OverlayKind, OverlayRequest, OverlayService,
    Placement, PointerOutcome, Side,
};
use std::cell::RefCell;
use std::rc::Rc;

fn node(index: u32) -> NodeId {
    NodeId::from_raw(RawId {
        index,
        generation: 0,
    })
}

#[derive(Default)]
struct Focus {
    current: Option<NodeId>,
}

impl FocusControl for Focus {
    fn focused(&self) -> Option<NodeId> {
        self.current
    }

    fn set_focused(&mut self, node: Option<NodeId>) {
        self.current = node;
    }
}

fn menu(owner: u32, parent: Option<aqua_ui::overlay::OverlayId>) -> OverlayRequest {
    OverlayRequest {
        owner: node(owner),
        parent,
        kind: OverlayKind::Menu,
        modality: Modality::NonModal,
        dismiss: DismissPolicy::LIGHT,
        anchor: Rect::from_edges(100.0, 100.0, 200.0, 130.0),
        size: Size::new(160.0, 200.0),
        placement: Placement {
            preferred: Side::Below,
            align: Align::Start,
            gap: 4.0,
            margin: 8.0,
            flip: true,
            centered: false,
        },
        autofocus: Some(node(50 + owner)),
    }
}

/// Fade/slide-in driven by a fixed-step frame loop, dismissed by an outside
/// press mid-flight: the overlay closes, focus restores, and the owner's
/// motion is disposed silently (no stale completion removes anything).
#[test]
fn menu_opens_animated_and_dismisses_outside_without_stale_completion() {
    let viewport = Rect::from_edges(0.0, 0.0, 800.0, 600.0);
    let mut focus = Focus {
        current: Some(node(1)),
    };
    let mut overlays = OverlayService::new(viewport);
    let mut motion = MotionController::new(ReducedMotion::Full);

    // Open + start a 0.16 s fade/slide at t = 0.
    let menu_id = overlays.open(&mut focus, menu(7, None)).unwrap();
    assert_eq!(focus.focused(), Some(node(57)));
    let fade = motion.spawn(
        Some(node(7)),
        MotionValue::Scalar(0.0),
        MotionKind::Tween {
            duration_secs: 0.16,
            easing: Easing::EaseOut,
        },
        0.0,
    );
    let fired = Rc::new(RefCell::new(Vec::new()));
    let fired_inner = Rc::clone(&fired);
    assert!(motion.on_complete(fade, Box::new(move |c| fired_inner.borrow_mut().push(c)),));
    motion
        .set_target(fade, MotionValue::Scalar(1.0), 0.0)
        .unwrap();

    // Fixed-step frames: advance only while the controller asks for one.
    let mut now: Seconds = 0.0;
    while motion.next_deadline(now).is_some() && now < 0.08 {
        for handle in motion.advance(now) {
            assert_eq!(handle, fade);
        }
        now += 1.0 / 120.0;
    }
    let mid = motion.sample(fade, now).expect("live");
    assert!(matches!(mid, MotionValue::Scalar(v) if v > 0.0 && v < 1.0));

    // Outside press: the menu light-dismisses and focus is restored.
    match overlays.handle_pointer_down(&mut focus, Point::new(790.0, 590.0)) {
        PointerOutcome::Dismissed { closed } => assert_eq!(closed.len(), 1),
        other => panic!("expected dismissal, got {other:?}"),
    }
    assert!(!overlays.is_open(menu_id));
    assert_eq!(focus.focused(), Some(node(1)));

    // Owner teardown: motions disposed silently — the in-flight completion
    // never fires, so it cannot remove a reopened component later.
    let removed = motion.dispose_owner(node(7));
    assert_eq!(removed.len(), 1);
    assert!(fired.borrow().is_empty());
    assert_eq!(motion.next_deadline(now), None);
}
