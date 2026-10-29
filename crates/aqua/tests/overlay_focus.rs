//! Verification: overlay lifecycle, focus save/restore, and motion deadlines.
//!
//! Black-box tests against the public `aqua-ui` overlay + motion APIs and
//! the `aqua-platform` scheduler they feed: placement fallback, stacking,
//! cascading dismissal, modality blocking, owner disposal with focus
//! restoration, and demand-driven animation deadlines (idle UI schedules no
//! frames; settled motion requests none).

use std::time::Duration;

use aqua::{Instant, Scheduler};
use aqua_core::id::RawId;
use aqua_core::{NodeId, Point, Rect, Size};
use aqua_platform::{AquaWindowId, FrameReason};
use aqua_ui::motion::{
    Animated, Completion, Easing, MotionController, MotionKind, MotionValue, ReducedMotion,
};
use aqua_ui::overlay::{
    CloseReason, DismissPolicy, FocusControl, InputOutcome, KeyOutcome, Modality, OverlayId,
    OverlayKind, OverlayRequest, OverlayService, Placement, PointerOutcome, ScrollOutcome, Side,
};

fn node(index: u32) -> NodeId {
    NodeId::from_raw(RawId {
        index,
        generation: 0,
    })
}

fn viewport() -> Rect {
    Rect::from_edges(0.0, 0.0, 800.0, 600.0)
}

fn anchor(x: f32, y: f32) -> Rect {
    Rect::from_edges(x, y, x + 100.0, y + 30.0)
}

fn size(w: f32, h: f32) -> Size {
    Size::new(w, h)
}

#[derive(Debug, Default)]
struct TestFocus {
    focused: Option<NodeId>,
}

impl FocusControl for TestFocus {
    fn focused(&self) -> Option<NodeId> {
        self.focused
    }
    fn set_focused(&mut self, node: Option<NodeId>) {
        self.focused = node;
    }
}

fn menu(owner: u32, at: Rect) -> OverlayRequest {
    OverlayRequest {
        owner: node(owner),
        parent: None,
        kind: OverlayKind::Menu,
        modality: Modality::NonModal,
        dismiss: DismissPolicy::LIGHT,
        anchor: at,
        size: size(160.0, 200.0),
        placement: Placement::below(),
        autofocus: None,
    }
}

// -- placement is anchoring with fallback, never a giant z -------------------

#[test]
fn overlay_falls_back_above_when_no_room_below() {
    let mut focus = TestFocus::default();
    let mut svc = OverlayService::new(viewport());
    let id = svc.open(&mut focus, menu(1, anchor(100.0, 560.0))).unwrap();
    assert_eq!(svc.side_used(id), Some(Side::Above));
    let b = svc.bounds(id).unwrap();
    assert!(b.bottom() <= 600.0 - 8.0 && b.y >= 8.0, "{b:?}");
}

#[test]
fn overlay_render_plan_stays_in_host_layer() {
    // Entries carry host-local stacking positions, not global z values.
    let mut focus = TestFocus::default();
    let mut svc = OverlayService::new(viewport());
    let a = svc.open(&mut focus, menu(1, anchor(10.0, 10.0))).unwrap();
    let b = svc.open(&mut focus, menu(2, anchor(300.0, 10.0))).unwrap();
    let plan = svc.render_plan();
    assert_eq!(plan.len(), 2);
    assert_eq!((plan[0].id, plan[1].id), (a, b));
    assert_eq!((plan[0].host_z, plan[1].host_z), (0, 1));
    assert!(!plan.iter().any(|e| e.modal));
}

// -- dismissal: explicit, nested, modal --------------------------------------

#[test]
fn nested_menus_close_with_parent_and_report_reasons() {
    let mut focus = TestFocus::default();
    let mut svc = OverlayService::new(viewport());
    let parent = svc.open(&mut focus, menu(1, anchor(10.0, 10.0))).unwrap();
    let mut child_req = menu(1, anchor(170.0, 40.0));
    child_req.parent = Some(parent);
    let child = svc.open(&mut focus, child_req).unwrap();

    let closed = svc.close(&mut focus, child, CloseReason::Requested);
    assert_eq!(closed.len(), 1);
    assert!(svc.is_open(parent), "closing a child keeps the parent");

    let mut child_req = menu(1, anchor(170.0, 40.0));
    child_req.parent = Some(parent);
    let child = svc.open(&mut focus, child_req).unwrap();
    let closed = svc.close(&mut focus, parent, CloseReason::OutsidePress);
    assert_eq!(closed.len(), 2);
    assert_eq!(closed[0].reason, CloseReason::OutsidePress);
    assert_eq!(
        closed[1],
        aqua_ui::overlay::ClosedOverlay {
            id: child,
            owner: node(1),
            reason: CloseReason::AncestorClosed,
        }
    );
    assert!(svc.is_empty());
}

#[test]
fn modal_blocks_press_and_scroll_but_not_inside_itself() {
    let mut focus = TestFocus::default();
    let mut svc = OverlayService::new(viewport());
    let modal = svc
        .open(
            &mut focus,
            OverlayRequest {
                owner: node(9),
                parent: None,
                kind: OverlayKind::Dialog,
                modality: Modality::Modal,
                dismiss: DismissPolicy::ESCAPE_ONLY,
                anchor: viewport(),
                size: size(300.0, 200.0),
                placement: Placement::centered_dialog(),
                autofocus: Some(node(90)),
            },
        )
        .unwrap();
    assert_eq!(
        svc.handle_pointer_down(&mut focus, Point::new(700.0, 590.0)),
        PointerOutcome::BlockedByModal { overlay: modal }
    );
    assert_eq!(
        svc.route_scroll(Point::new(700.0, 590.0)),
        ScrollOutcome::BlockedByModal { overlay: modal }
    );
    let inside = svc.bounds(modal).unwrap();
    assert_eq!(
        svc.handle_pointer_down(&mut focus, Point::new(inside.x + 4.0, inside.y + 4.0)),
        PointerOutcome::Captured { overlay: modal }
    );
}

#[test]
fn escape_dismisses_only_escape_closable_topmost() {
    use aqua_core::{Key, KeyEvent, Modifiers, NamedKey};
    let mut focus = TestFocus::default();
    let mut svc = OverlayService::new(viewport());
    let bottom = svc.open(&mut focus, menu(1, anchor(10.0, 10.0))).unwrap();
    let mut sticky = menu(2, anchor(300.0, 10.0));
    sticky.dismiss = DismissPolicy::MANUAL;
    let top = svc.open(&mut focus, sticky).unwrap();
    let esc = KeyEvent {
        key: Key::Named(NamedKey::Escape),
        modifiers: Modifiers::NONE,
        repeat: false,
    };
    assert_eq!(svc.handle_key(&mut focus, &esc), KeyOutcome::Ignored);
    svc.close(&mut focus, top, CloseReason::Requested);
    match svc.handle_key(&mut focus, &esc) {
        KeyOutcome::Dismissed { closed } => {
            assert_eq!(closed.len(), 1);
            assert_eq!(closed[0].id, bottom);
            assert_eq!(closed[0].reason, CloseReason::Escape);
        }
        other => panic!("expected dismissal, got {other:?}"),
    }
}

// -- focus lifecycle ----------------------------------------------------------

#[test]
fn owner_disposal_closes_chain_and_restores_focus() {
    let mut focus = TestFocus {
        focused: Some(node(5)),
    };
    let mut svc = OverlayService::new(viewport());
    let parent = svc
        .open(
            &mut focus,
            OverlayRequest {
                autofocus: Some(node(11)),
                ..menu(1, anchor(10.0, 10.0))
            },
        )
        .unwrap();
    assert_eq!(focus.focused(), Some(node(11)));
    let closed = svc.dispose_owner(&mut focus, node(1));
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].id, parent);
    assert_eq!(closed[0].reason, CloseReason::OwnerRemoved);
    assert_eq!(
        focus.focused(),
        Some(node(5)),
        "focus restored to saved node"
    );
    assert!(!svc.is_open(parent), "stale handle resolves to nothing");
}

#[test]
fn focus_user_moved_is_never_yanked_back() {
    let mut focus = TestFocus {
        focused: Some(node(5)),
    };
    let mut svc = OverlayService::new(viewport());
    let modal = svc
        .open(
            &mut focus,
            OverlayRequest {
                autofocus: Some(node(90)),
                ..menu(9, viewport())
            },
        )
        .unwrap();
    focus.set_focused(Some(node(77))); // user tabs away
    svc.close(&mut focus, modal, CloseReason::Requested);
    assert_eq!(focus.focused(), Some(node(77)));
}

#[test]
fn closed_handles_never_alias_later_overlays() {
    let mut focus = TestFocus::default();
    let mut svc = OverlayService::new(viewport());
    let first = svc.open(&mut focus, menu(1, anchor(10.0, 10.0))).unwrap();
    svc.close(&mut focus, first, CloseReason::Requested);
    let second = svc.open(&mut focus, menu(1, anchor(10.0, 10.0))).unwrap();
    assert_ne!(first, second, "generations must differ on slot reuse");
    assert!(!svc.is_open(first));
    // Unknown handles close nothing and report nothing.
    let ghost = OverlayId::from_raw(RawId {
        index: 99,
        generation: 0,
    });
    assert!(svc
        .close(&mut focus, ghost, CloseReason::Requested)
        .is_empty());
    let _ = second;
}

#[test]
fn input_router_covers_press_and_ignores_unrelated() {
    use aqua_core::{InputEvent, PointerButton, PointerEvent, PointerKind};
    let mut focus = TestFocus::default();
    let mut svc = OverlayService::new(viewport());
    let m = svc.open(&mut focus, menu(1, anchor(10.0, 10.0))).unwrap();
    let b = svc.bounds(m).unwrap();
    let press = InputEvent::PointerDown(
        PointerEvent {
            contact: 0,
            kind: PointerKind::Mouse,
            position: Point::new(b.x + 2.0, b.y + 2.0),
            buttons: 1,
        },
        PointerButton::Primary,
    );
    assert_eq!(
        svc.handle_input(&mut focus, &press),
        InputOutcome::Captured { overlay: m }
    );
}

// -- motion: one model, scheduler-driven deadlines ----------------------------

fn tween(secs: f64) -> MotionKind {
    MotionKind::Tween {
        duration_secs: secs,
        easing: Easing::Linear,
    }
}

#[test]
fn motion_deadline_drives_scheduler_wakeup_then_goes_idle() {
    let mut ctl = MotionController::new(ReducedMotion::Full);
    let h = ctl.spawn(None, MotionValue::Scalar(0.0), tween(0.2), 0.0);
    ctl.set_target(h, MotionValue::Scalar(1.0), 0.0).unwrap();

    let mut sched = Scheduler::new();
    let win = AquaWindowId::new(1).unwrap();
    let now = Instant::from_nanos(1_000_000_000);
    let deadline_secs = ctl.next_deadline(0.0).expect("unsettled needs a frame");
    let deadline = now + Duration::from_secs_f64(deadline_secs.max(0.0));
    sched.request_animation_deadline(win, deadline);
    assert_eq!(sched.next_wakeup(now), Some(deadline));
    let mut t = 0.0;
    while ctl.next_deadline(t).is_some() && t < 5.0 {
        ctl.advance(t);
        t += 0.016;
    }
    assert_eq!(ctl.sample(h, t), Some(MotionValue::Scalar(1.0)));
    assert_eq!(ctl.next_deadline(t), None, "settled: no wakeups requested");
}

#[test]
fn reduced_motion_collapses_inflight_to_snap() {
    let mut ctl = MotionController::new(ReducedMotion::Full);
    let h = ctl.spawn(Some(node(3)), MotionValue::Scalar(0.0), tween(1.0), 0.0);
    ctl.set_target(h, MotionValue::Scalar(9.0), 0.0).unwrap();
    ctl.advance(0.4);
    assert_ne!(ctl.sample(h, 0.4), Some(MotionValue::Scalar(9.0)));
    ctl.set_reduced_motion(ReducedMotion::Reduce, 0.4);
    assert_eq!(ctl.sample(h, 0.4), Some(MotionValue::Scalar(9.0)));
    assert_eq!(ctl.next_deadline(0.4), None);
}

#[test]
fn motion_owner_disposal_is_silent_and_scoped() {
    let mut ctl = MotionController::new(ReducedMotion::Full);
    let owned = ctl.spawn(Some(node(1)), MotionValue::Scalar(0.0), tween(1.0), 0.0);
    ctl.set_target(owned, MotionValue::Scalar(5.0), 0.0)
        .unwrap();
    let other = ctl.spawn(Some(node(2)), MotionValue::Scalar(0.0), tween(1.0), 0.0);
    ctl.set_target(other, MotionValue::Scalar(7.0), 0.0)
        .unwrap();
    assert_eq!(ctl.dispose_owner(node(1)), vec![owned]);
    assert_eq!(ctl.sample(owned, 0.5), None);
    assert!(
        ctl.next_deadline(0.5).is_some(),
        "survivor still needs frames"
    );
}

#[test]
fn motion_completion_fires_once_and_supersede_is_silent() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mut anim = Animated::new(
        MotionValue::Scalar(0.0),
        tween(0.1),
        ReducedMotion::Full,
        0.0,
    );
    let seen2 = Rc::clone(&seen);
    anim.on_complete(Box::new(move |c| seen2.borrow_mut().push(c)));
    anim.set_target(MotionValue::Scalar(10.0), 0.0).unwrap();
    anim.sample(0.05);
    assert!(seen.borrow().is_empty());
    anim.sample(0.1);
    assert_eq!(*seen.borrow(), vec![Completion::Completed]);
    anim.sample(1.0);
    assert_eq!(seen.borrow().len(), 1, "completion fires exactly once");
}

#[test]
fn animation_only_reason_does_not_draw_before_deadline() {
    let mut sched = Scheduler::new();
    let win = AquaWindowId::new(7).unwrap();
    let now = Instant::from_nanos(1_000_000_000);
    sched.request_animation_deadline(win, now + Duration::from_millis(100));
    assert!(sched.ready_windows(now).is_empty());
    assert_eq!(
        sched.next_wakeup(now),
        Some(now + Duration::from_millis(100))
    );
    sched.request_frame(win, FrameReason::DAMAGE);
    assert_eq!(sched.ready_windows(now).len(), 1);
}
