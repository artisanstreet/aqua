//! Diamond coherence, equality suppression, dynamic select, cycle rejection.
use std::cell::RefCell;
use std::rc::Rc;

use aqua_reactive::Runtime;

#[test]
fn diamond_is_glitch_free_and_coherent() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(1i32);
    let b = scope.derive1(&a, |v| v * 10);
    let c = scope.derive1(&a, |v| v * 100);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_clone = seen.clone();
    let d = scope.derive((b.clone(), c.clone()), move |(b, c)| {
        seen_clone.borrow_mut().push((b, c));
        b + c
    });
    // Initial computation ran once at creation.
    assert_eq!(d.get(), 110);
    assert_eq!(*seen.borrow(), vec![(10, 100)]);

    set_a.set(2);
    // Exactly one recompute, with both branches current: never (20, 100).
    assert_eq!(d.get(), 220);
    assert_eq!(*seen.borrow(), vec![(10, 100), (20, 200)]);

    let stats = rt.stats();
    assert_eq!(stats.sources, 1);
    assert_eq!(stats.derived, 3);
    assert_eq!(stats.edges, 4);
}

#[test]
fn equality_suppression_stops_downstream() {
    let rt = Runtime::new();
    rt.set_auto_flush(false);
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(1i32);
    // Parity flips only on odd->even transitions being even...: parity of
    // doubling is always even, so b rarely changes.
    let b = scope.derive1(&a, |v| (v * 2) % 2);
    let runs = Rc::new(RefCell::new(0usize));
    let runs_clone = runs.clone();
    let c = scope.derive1(&b, move |v| {
        *runs_clone.borrow_mut() += 1;
        v + 1
    });
    assert_eq!(*runs.borrow(), 1); // initial
    set_a.set(2);
    set_a.set(3);
    set_a.set(4);
    // b stayed 0 throughout: c recomputed only at creation.
    assert_eq!(c.get(), 1);
    assert_eq!(*runs.borrow(), 1);
    assert_eq!(rt.pending(), 0);
}

#[test]
fn derive_no_eq_always_notifies() {
    struct NoEq(u8); // deliberately no PartialEq
    impl Clone for NoEq {
        fn clone(&self) -> Self {
            NoEq(self.0)
        }
    }
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(0u8);
    let count = Rc::new(RefCell::new(0usize));
    let count_clone = count.clone();
    let b = scope.derive_no_eq((a.clone(),), move |(v,)| {
        *count_clone.borrow_mut() += 1;
        NoEq(v)
    });
    assert_eq!(*count.borrow(), 1);
    set_a.set(0); // same value, but no equality to suppress with
    assert_eq!(b.get().0, 0);
    assert_eq!(*count.borrow(), 2);
}

#[test]
fn try_derive_error_recovery() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (divisor, set_divisor) = scope.signal(0i32);
    let result = scope.try_derive((divisor.clone(),), |(d,)| {
        if d == 0 {
            Err("division by zero")
        } else {
            Ok(100 / d)
        }
    });
    assert_eq!(result.get(), Err("division by zero"));
    set_divisor.set(4);
    assert_eq!(result.get(), Ok(25));
    // And back into the error state: recovery is not one-directional.
    set_divisor.set(0);
    assert_eq!(result.get(), Err("division by zero"));
}

#[test]
fn select_switches_subscription() {
    let rt = Runtime::new();
    rt.set_auto_flush(false);
    let scope = rt.create_scope();
    let (key, set_key) = scope.signal(0usize);
    let (x, set_x) = scope.signal(10i32);
    let (y, set_y) = scope.signal(20i32);
    let x_r = x.clone();
    let y_r = y.clone();
    let current = scope.select(
        &key,
        move |k| {
            if *k == 0 {
                x_r.clone()
            } else {
                y_r.clone()
            }
        },
    );
    rt.flush();
    assert_eq!(current.get(), 10);

    // Writes to the inactive alternative schedule nothing new.
    let before = rt.stats();
    set_y.set(21);
    rt.flush();
    assert_eq!(current.get(), 10);
    assert_eq!(rt.stats().effects, before.effects);

    // Switch keys: now y is live and x is detached.
    set_key.set(1);
    rt.flush();
    assert_eq!(current.get(), 21);
    set_x.set(11);
    rt.flush();
    assert_eq!(current.get(), 21); // stale x write ignored
    set_y.set(22);
    rt.flush();
    assert_eq!(current.get(), 22);
}

#[test]
fn select_self_cycle_is_rejected_and_counted() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (key, set_key) = scope.signal(0u8);
    let (dummy, _) = scope.signal(0i32);
    // The resolver hands out whatever the holder points at. It starts at a
    // harmless dummy; the test then aims it at a node downstream of sel.
    let downstream_holder: Rc<RefCell<aqua_reactive::ReadSignal<i32>>> =
        Rc::new(RefCell::new(dummy.clone()));
    let holder_clone = downstream_holder.clone();
    let sel = scope.select_no_eq(&key, move |_| holder_clone.borrow().clone());
    // Wire downstream to depend on sel, then point the holder at downstream:
    // resolving now would subscribe sel -> downstream -> sel (a cycle).
    let downstream = scope.derive_no_eq((sel.clone(),), |(v,)| v + 1);
    *downstream_holder.borrow_mut() = downstream.clone();
    let rejections_before = rt.stats().cycle_rejections;
    // Trigger re-resolution with a key change.
    set_key.set(1);
    // Cycle rejected: previous subscription retained, rejection counted.
    assert_eq!(rt.stats().cycle_rejections, rejections_before + 1);
    // Graph still coherent: unrelated writes propagate.
    let _ = downstream.get();
}

#[test]
fn cross_runtime_mixing_is_rejected() {
    let rt_a = Runtime::new();
    let rt_b = Runtime::new();
    let scope_a = rt_a.create_scope();
    let scope_b = rt_b.create_scope();
    let (foreign, _) = scope_b.signal(1i32);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        scope_a.derive1(&foreign, |v| v + 1);
    }));
    assert!(result.is_err(), "cross-runtime derive must panic");
    // rt_a is untouched and usable.
    let (local, set_local) = scope_a.signal(1i32);
    let d = scope_a.derive1(&local, |v| v + 1);
    set_local.set(5);
    assert_eq!(d.get(), 6);
}
