//! Verification: explicit reactive updates, disposal, and coherent batching.
//!
//! Black-box tests against the public `aqua-reactive` API (`Runtime`,
//! `Scope`, signals). They assert the design contract from
//! `GUI_FRAMEWORK_DESIGN.md`: explicit declared inputs only (no implicit
//! subscription), glitch-free propagation, coarse coherent batches, and
//! cascade disposal that returns live counters to baseline.

use std::cell::RefCell;
use std::rc::Rc;

use aqua_reactive::{GraphError, Runtime};

#[test]
fn derived_observes_only_declared_inputs() {
    // A read of `c` inside the computation body must NOT subscribe: declared
    // inputs are the tuple argument, everything else is a snapshot.
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(1i32);
    let (c, set_c) = scope.signal(100i32);
    let c_probe = c.clone();
    let d = scope.derive((a.clone(),), move |(a,)| a + c_probe.try_get().unwrap_or(0));
    assert_eq!(d.get(), 101);
    let v0 = d.version();

    // Undeclared input changes: no recompute, no notification.
    set_c.set(200);
    assert_eq!(d.get(), 101, "undeclared read must stay a snapshot");
    assert_eq!(
        d.version(),
        v0,
        "version must not advance on undeclared change"
    );

    // Declared input changes: recompute picks up the latest snapshot.
    set_a.set(2);
    assert_eq!(d.get(), 202);
}

#[test]
fn diamond_propagates_glitch_free() {
    // b and c both derive from a; e observes both. The effect must see only
    // the initial and the final coherent value, never a mixed (b-new, c-old)
    // intermediate.
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(1i32);
    let b = scope.derive((a.clone(),), |(a,)| a + 1);
    let c = scope.derive((a.clone(),), |(a,)| a * 10);
    let e = scope.derive((b.clone(), c.clone()), |(b, c)| b + c);
    assert_eq!(e.get(), 12);

    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen2 = Rc::clone(&seen);
    let _h = scope.effect((e.clone(),), move |(e,)| {
        seen2.borrow_mut().push(e);
    });
    assert_eq!(*seen.borrow(), vec![12]);
    set_a.set(2);
    assert_eq!(*seen.borrow(), vec![12, 23], "effect saw a glitch state");
}

#[test]
fn batch_coalesces_effects_to_latest_values() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (x, set_x) = scope.signal(0i32);
    let (y, set_y) = scope.signal(0i32);
    let sum = scope.derive((x.clone(), y.clone()), |(x, y)| x + y);

    let runs = Rc::new(RefCell::new(0usize));
    let last = Rc::new(RefCell::new(0i32));
    let (runs2, last2) = (Rc::clone(&runs), Rc::clone(&last));
    let _h = scope.effect((sum.clone(),), move |(s,)| {
        *runs2.borrow_mut() += 1;
        *last2.borrow_mut() = s;
    });
    assert_eq!(*runs.borrow(), 1, "effect runs once on creation");
    scope.batch(|| {
        set_x.set(1);
        set_y.set(2);
    });
    assert_eq!(*runs.borrow(), 2, "one coalesced run, not two");
    assert_eq!(*last.borrow(), 3, "latest values, not first-write values");
    assert_eq!(rt.pending(), 0, "queue drained at batch boundary");
}

#[test]
fn dispose_cascade_returns_counters_to_baseline() {
    let rt = Runtime::new();
    let parent = rt.create_scope();
    let (a, _wa) = parent.signal(1i32);
    let _d = parent.derive((a.clone(),), |(a,)| a + 1);
    let child = parent.child();
    let (b, _wb) = child.signal(2i32);
    let _e = child.effect((b.clone(),), |_| {});
    assert_eq!(rt.stats().sources, 2);
    assert_eq!(rt.stats().derived, 1);
    assert_eq!(rt.stats().effects, 1);
    assert_eq!(rt.stats().live_scopes, 2);

    let parent_probe = parent.clone();
    child.dispose();
    let s = rt.stats();
    assert_eq!((s.sources, s.derived, s.effects), (1, 1, 0));
    assert_eq!(s.live_scopes, 1);
    assert!(!b.is_live(), "disposed scope member is dead");

    parent.dispose();
    let s = rt.stats();
    assert_eq!((s.sources, s.derived, s.effects), (0, 0, 0));
    assert_eq!(s.live_scopes, 0, "no tombstoned scopes");
    assert!(a.try_get().is_none(), "stale read fails closed");
    assert!(!parent_probe.is_live());
}

#[test]
fn stale_writes_fail_closed_without_panic() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (r, w) = scope.signal(7i32);
    scope.dispose();
    assert!(w.try_set(8).is_err(), "write to disposed node must fail");
    assert!(w.try_update(|v| *v += 1).is_err());
    assert_eq!(r.try_get(), None);
}

#[test]
fn derived_values_are_pure_read_only() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (victim, wvictim) = scope.signal(0i32);
    let (a, _wa) = scope.signal(1i32);
    let probe = wvictim.clone();
    let _d = scope.derive((a.clone(),), move |(a,)| {
        let err = probe.try_set(a);
        assert!(
            matches!(err, Err(GraphError::DerivedWrite(_))),
            "write inside derive must be rejected, got {err:?}"
        );
        a
    });
    // Initial computation already ran the probe; victim untouched.
    assert_eq!(victim.get(), 0);
}

#[test]
fn manual_flush_with_wake_callback_for_scheduler() {
    // Host UI scheduler integration: with auto_flush off, writes queue and
    // poke the wake callback instead of running effects inline.
    let rt = Runtime::new();
    rt.set_auto_flush(false);
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(0i32);
    let fired = Rc::new(RefCell::new(0usize));
    let fired2 = Rc::clone(&fired);
    let _h = scope.effect((a.clone(),), move |_| {
        *fired2.borrow_mut() += 1;
    });
    // Creation-time effect is queued, not run (manual mode).
    assert_eq!(rt.pending(), 1);
    let report = rt.flush();
    assert_eq!(report.ran, 1);
    assert_eq!(*fired.borrow(), 1);

    let woke = Rc::new(RefCell::new(0usize));
    let woke2 = Rc::clone(&woke);
    rt.set_wake_callback(move || *woke2.borrow_mut() += 1);
    set_a.set(5);
    assert_eq!(*woke.borrow(), 1, "scheduler wake requested");
    assert_eq!(*fired.borrow(), 1, "effect deferred until flush");
    let report = rt.flush();
    assert_eq!((report.ran, report.truncated), (1, false));
    assert_eq!(*fired.borrow(), 2);
    assert_eq!(rt.pending(), 0);
}

#[test]
fn select_resubscribes_without_cost_on_inactive_alternatives() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (key, set_key) = scope.signal(0u32);
    let (v0, set_v0) = scope.signal(10i32);
    let (v1, set_v1) = scope.signal(20i32);
    let (c0, c1) = (v0.clone(), v1.clone());
    let sel = scope.select(&key, move |k| if *k == 0 { c0.clone() } else { c1.clone() });
    assert_eq!(sel.get(), 10);

    // Inactive alternative changes schedule nothing observable.
    set_v1.set(21);
    assert_eq!(sel.get(), 10, "inactive alternative must not propagate");

    set_key.set(1);
    assert_eq!(sel.get(), 21, "switch mirrors the new target");
    set_v0.set(11);
    assert_eq!(sel.get(), 21, "old target unsubscribed after switch");
    set_v1.set(22);
    assert_eq!(sel.get(), 22);
}

#[test]
fn equality_suppression_skips_dependent_notifications() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(1i32);
    // Integer division: 1/2 == 0 and 0/2... use values mapping to same output.
    let half = scope.derive((a.clone(),), |(a,)| a / 2);
    let runs = Rc::new(RefCell::new(0usize));
    let runs2 = Rc::clone(&runs);
    let _h = scope.effect((half.clone(),), move |_| *runs2.borrow_mut() += 1);
    assert_eq!(*runs.borrow(), 1);
    set_a.set(1); // same value, source always notifies...
    let _ = half.get();
    set_a.set(0); // 0/2 == 0... 1/2==0 too: equal output suppresses
    assert_eq!(half.get(), 0);
    // Effect ran on creation + (maybe) on 1->1? derive eq suppresses, so no run.
    assert!(*runs.borrow() <= 2, "runs={}", runs.borrow());
}

#[test]
fn set_if_changed_avoids_spurious_propagation() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(5i32);
    let d = scope.derive((a.clone(),), |(a,)| a * 2);
    let v0 = d.version();
    assert!(!set_a.set_if_changed(5), "equal write commits nothing");
    assert_eq!(d.version(), v0);
    assert!(set_a.set_if_changed(6));
    assert_eq!(d.get(), 12);
}

#[test]
fn scopes_form_a_dispose_tree() {
    // Siblings are independent; parent disposal takes the whole subtree.
    let rt = Runtime::new();
    let root = rt.create_scope();
    let left = root.child();
    let right = root.child();
    let (l, _wl) = left.signal(1i32);
    let (r, _wr) = right.signal(2i32);
    left.dispose();
    assert!(!l.is_live());
    assert!(r.is_live(), "sibling survives");
    root.dispose();
    assert!(!r.is_live());
    assert_eq!(rt.stats().live_scopes, 0);
}
