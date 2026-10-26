//! Effects: external sync, writes from effects, stale disposal, flush bounds,
//! the derivation purity guard, and snapshot semantics.
use std::cell::RefCell;
use std::rc::Rc;

use aqua_reactive::{FlushReport, GraphError, Runtime};

#[test]
fn writes_from_effects_terminate_and_converge() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(1i32);
    let (b, set_b) = scope.signal(0i32);
    // Effect mirrors a doubled into b: a feedback-free forward write.
    let b_w = set_b.clone();
    let _e = scope.effect1(&a, move |v| {
        b_w.set(v * 2);
    });
    assert_eq!(b.get(), 2);
    set_a.set(5);
    assert_eq!(b.get(), 10);
    // No runaway: the flush drained.
    assert_eq!(rt.pending(), 0);
}

#[test]
fn feedback_loop_hits_flush_limit_instead_of_hanging() {
    let rt = Runtime::new();
    rt.set_auto_flush(false);
    rt.set_flush_limit(10);
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(0i32);
    // Pure feedback: every run writes its own input.
    let w = set_a.clone();
    let _e = scope.effect1(&a, move |v| {
        w.set(v + 1);
    });
    let report: FlushReport = rt.flush();
    assert!(report.truncated, "expected truncation, got {report:?}");
    assert_eq!(report.ran, 10);
    assert!(rt.stats().flush_truncations >= 1);
    // The graph is still usable afterwards with a sane limit.
    rt.set_flush_limit(1024);
    let report = rt.flush();
    assert!(!report.truncated || report.ran > 0);
}

#[test]
fn stale_queued_effect_never_runs_after_dispose() {
    let rt = Runtime::new();
    rt.set_auto_flush(false);
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(0i32);
    let ran = Rc::new(RefCell::new(0usize));
    let ran_clone = ran.clone();
    let handle = scope.effect1(&a, move |_| {
        *ran_clone.borrow_mut() += 1;
    });
    assert_eq!(rt.pending(), 1); // initial run queued
    set_a.set(1); // coalesced into the same queued entry
    assert_eq!(rt.pending(), 1);
    handle.dispose();
    let report = rt.flush();
    // Disposal purged the queued entry, so there is nothing to run or skip.
    assert_eq!(*ran.borrow(), 0);
    assert_eq!((report.ran, report.skipped), (0, 0));
    assert_eq!(rt.pending(), 0);
}

#[test]
fn reentrant_flush_is_a_noop() {
    let rt = Runtime::new();
    rt.set_auto_flush(false);
    let scope = rt.create_scope();
    let (a, _) = scope.signal(0i32);
    let inner_report = Rc::new(RefCell::new(None));
    let inner_report_clone = inner_report.clone();
    let scope_clone = scope.clone();
    let _e = scope.effect1(&a, move |_| {
        *inner_report_clone.borrow_mut() = Some(scope_clone.flush());
    });
    let outer = rt.flush();
    assert_eq!(outer.ran, 1);
    let inner: FlushReport = inner_report.borrow().clone().unwrap();
    assert!(inner.reentered);
    assert_eq!(inner.ran, 0);
}

#[test]
fn derive_rejects_writes_with_diagnostic() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(1i32);
    let (evil_target, evil_w) = scope.signal(0i32);
    let evil = evil_w.clone();
    // A derived computation that tries to write: rejected, not silently skipped.
    let d = scope.derive1(&a, move |v| {
        let _ = evil.try_set(v * 100);
        v + 1
    });
    assert_eq!(d.get(), 2);
    // try_set inside derivation failed: the target never changed...
    assert_eq!(evil_target.get(), 0);
    // ...and the graph stayed coherent for later updates.
    set_a.set(10);
    assert_eq!(d.get(), 11);
    assert_eq!(evil_target.get(), 0);
}

#[test]
fn derive_write_violation_surfaces_as_error() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, _) = scope.signal(1i32);
    let (_t, set_t) = scope.signal(0i32);
    let err = Rc::new(RefCell::new(None));
    let err_clone = err.clone();
    let _d = scope.derive1(&a, move |v| {
        *err_clone.borrow_mut() = set_t.try_set(v).err();
        v
    });
    let captured = err.borrow().clone().unwrap();
    assert!(
        matches!(captured, GraphError::DerivedWrite(_)),
        "expected DerivedWrite, got {captured:?}"
    );
}

#[test]
fn snapshot_reads_never_subscribe() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(1i32);
    let (b, _) = scope.signal(10i32);
    // Incidental reads of b inside a's derived computation are snapshots:
    // declared inputs alone determine the edges.
    let d = scope.derive((a.clone(),), move |(v,)| v + b.snapshot().unwrap_or(-1));
    assert_eq!(d.get(), 11);
    let edges_before = rt.stats().edges;
    assert_eq!(edges_before, 1); // only a -> d
    set_a.set(2);
    assert_eq!(d.get(), 12);
    assert_eq!(rt.stats().edges, 1);
}

#[test]
fn subscribe_is_the_ui_binding_primitive() {
    let rt = Runtime::new();
    rt.set_auto_flush(false);
    let scope = rt.create_scope();
    let (label, set_label) = scope.signal("hello".to_string());
    // Stand-in for a UI property sink.
    let painted = Rc::new(RefCell::new(Vec::new()));
    let painted_clone = painted.clone();
    let wake_count = Rc::new(RefCell::new(0usize));
    let wake_count_clone = wake_count.clone();
    rt.set_wake_callback(move || {
        *wake_count_clone.borrow_mut() += 1;
    });
    let _binding = scope.subscribe(&label, move |text| {
        painted_clone.borrow_mut().push(text);
    });
    assert_eq!(*wake_count.borrow(), 1); // scheduler notified, no auto flush
    assert!(painted.borrow().is_empty());
    rt.flush();
    assert_eq!(*painted.borrow(), vec!["hello".to_string()]);
    set_label.set("world".to_string());
    assert_eq!(*wake_count.borrow(), 2);
    rt.flush();
    assert_eq!(
        *painted.borrow(),
        vec!["hello".to_string(), "world".to_string()]
    );
}
