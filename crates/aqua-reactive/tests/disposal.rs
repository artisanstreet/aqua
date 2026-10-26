//! Disposal, cascade detachment, runtime teardown, and retention baselines.
use std::cell::RefCell;
use std::rc::Rc;

use aqua_reactive::{GraphError, Runtime};

struct DropSpy(Rc<RefCell<usize>>);

impl Drop for DropSpy {
    fn drop(&mut self) {
        *self.0.borrow_mut() += 1;
    }
}

impl Clone for DropSpy {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

#[test]
fn scope_disposal_frees_members_and_runs_cleanups_lifo() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let order = Rc::new(RefCell::new(Vec::new()));
    let o1 = order.clone();
    let o2 = order.clone();
    scope.on_cleanup(move || o1.borrow_mut().push("first"));
    scope.on_cleanup(move || o2.borrow_mut().push("second"));

    let drops = Rc::new(RefCell::new(0usize));
    let (s, _) = scope.signal(DropSpy(drops.clone()));
    let d = scope.derive1(&s, |spy| *spy.0.borrow());
    let _e = scope.subscribe(&d, |_| {});
    assert!(rt.stats().sources > 0);
    let baseline = rt.stats();
    let drops_before = *drops.borrow(); // temporaries from setup already gone

    scope.dispose();
    // LIFO cleanups.
    assert_eq!(*order.borrow(), vec!["second", "first"]);
    // Member records, edges, thunks, and subscription queue entries gone.
    let stats = rt.stats();
    assert_eq!(stats.sources, baseline.sources - 1);
    assert_eq!(stats.derived, 0);
    assert_eq!(stats.effects, 0);
    assert_eq!(stats.edges, 0);
    assert_eq!(stats.queued_effects, 0);
    // Typed values dropped (the source cell's DropSpy; the derived cell holds
    // a plain usize, and setup temporaries were already gone).
    drop(s);
    drop(d);
    assert_eq!(*drops.borrow() - drops_before, 1);
}

#[test]
fn child_dispose_detaches_from_source_without_killing_it() {
    let rt = Runtime::new();
    let root = rt.create_scope();
    let (a, set_a) = root.signal(1i32);
    let child = root.child();
    let b = child.derive1(&a, |v| v + 1);
    assert_eq!(b.get(), 2);

    child.dispose();
    drop(b);
    // The source survives with no dangling dependents.
    let stats = rt.stats();
    assert_eq!(stats.sources, 1);
    assert_eq!(stats.derived, 0);
    assert_eq!(stats.edges, 0);
    set_a.set(10);
    assert_eq!(a.get(), 10);
}

#[test]
fn disposing_source_cascades_to_declared_dependents() {
    let rt = Runtime::new();
    let root = rt.create_scope();
    let (a, _) = root.signal(1i32);
    let child = root.child();
    let b = child.derive1(&a, |v| v + 1);
    assert_eq!(b.get(), 2);
    let b_handle = b.clone();
    drop(b);

    // Disposing the root cascades through the source into the child scope's
    // dependent: a computation over a dead input cannot keep producing values.
    root.dispose();
    assert!(b_handle.try_get().is_none());
    let stats = rt.stats();
    assert_eq!(
        (stats.sources, stats.derived, stats.edges, stats.live_scopes),
        (0, 0, 0, 0)
    );
}

#[test]
fn stale_handles_are_diagnosed_not_aliased() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (r, w) = scope.signal(1i32);
    scope.dispose();
    assert!(!r.is_live());
    assert!(r.try_get().is_none());
    assert!(matches!(w.try_set(2), Err(GraphError::Disposed(_))));
}

#[test]
fn runtime_drop_invalidates_handles_and_frees_graph() {
    let drops = Rc::new(RefCell::new(0usize));
    let (r, w, eff, rt) = {
        let rt = Runtime::new();
        let scope = rt.create_scope();
        let (r, w) = scope.signal(DropSpy(drops.clone()));
        let d = scope.derive1(&r, |spy| *spy.0.borrow());
        let e = scope.subscribe(&d, |_| {});
        (r, w, e, rt)
    };
    // Handles outlive the scope; the runtime is still alive via `rt`.
    assert!(r.try_get().is_some());
    let drops_before = *drops.borrow();
    drop(rt);
    // Everything diagnosed as gone.
    assert!(r.try_get().is_none());
    assert!(matches!(
        w.try_set(DropSpy(drops.clone())),
        Err(GraphError::RuntimeDropped)
    ));
    assert!(!eff.is_live());
    // No cycles: dropping the last user handles frees the stored value
    // (the source cell's DropSpy; +1 from the rejected try_set temporary).
    drop(r);
    drop(w);
    drop(eff);
    assert_eq!(*drops.borrow() - drops_before, 2);
}

#[test]
fn derived_unsubscribe_on_scope_dispose() {
    let rt = Runtime::new();
    rt.set_auto_flush(false);
    let root = rt.create_scope();
    let (a, set_a) = root.signal(0i32);
    let child = root.child();
    let runs = Rc::new(RefCell::new(0usize));
    let runs_clone = runs.clone();
    let _e = child.effect1(&a, move |_| {
        *runs_clone.borrow_mut() += 1;
    });
    rt.flush();
    assert_eq!(*runs.borrow(), 1);
    child.dispose();
    set_a.set(1);
    rt.flush();
    // Disposed effect never ran again; stale queue entries were purged.
    assert_eq!(*runs.borrow(), 1);
    assert_eq!(rt.pending(), 0);
    let stats = rt.stats();
    assert_eq!(stats.effects, 0);
    assert_eq!(stats.edges, 0);
}

#[test]
fn cleanup_runs_exactly_once_across_double_dispose() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let scope2 = scope.clone();
    let count = Rc::new(RefCell::new(0usize));
    let count_clone = count.clone();
    scope.on_cleanup(move || {
        *count_clone.borrow_mut() += 1;
    });
    scope.dispose();
    assert_eq!(*count.borrow(), 1);
    // Second dispose through a cloned handle is a no-op: no second run.
    scope2.dispose();
    assert_eq!(*count.borrow(), 1);
}

#[test]
fn cleanup_executes_outside_graph_borrows() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let rt = Runtime::new();
    let rt_cleanup = rt.clone();
    let scope = rt.create_scope();
    let (dead_r, dead_w) = scope.signal(1i32);
    drop(dead_r);
    let probed = Rc::new(RefCell::new(false));
    let probed_clone = probed.clone();
    scope.on_cleanup(move || {
        // If cleanups ran under an Inner borrow, any of these would panic
        // with "already borrowed" instead of behaving normally.
        let _ = rt_cleanup.stats();
        let _ = rt_cleanup.flush();
        let err = dead_w.try_set(2).err();
        assert!(matches!(err, Some(GraphError::Disposed(_))));
        *probed_clone.borrow_mut() = true;
    });
    scope.dispose();
    assert!(*probed.borrow());
}

#[test]
fn runtime_drop_without_scope_dispose_runs_no_cleanups() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let drops = Rc::new(RefCell::new(0usize));
    let fired = Rc::new(RefCell::new(0usize));
    {
        let rt = Runtime::new();
        let scope = rt.create_scope();
        let (r, w) = scope.signal(DropSpy(drops.clone()));
        let fired_clone = fired.clone();
        scope.on_cleanup(move || {
            *fired_clone.borrow_mut() += 1;
        });
        // Teardown without disposing the scope: the documented
        // explicit-disposal rule — cleanups are owned by scopes and run on
        // Scope::dispose, never implicitly at Runtime drop.
        drop(rt);
        assert_eq!(*fired.borrow(), 0);
        drop(scope);
        drop(r);
        drop(w);
    }
    // Value storage freed through handle drops (DropSpy), but the cleanup
    // callback was merely dropped, never executed.
    assert_eq!(*drops.borrow(), 1);
    assert_eq!(*fired.borrow(), 0);
}

#[test]
fn create_dispose_churn_returns_to_baseline() {
    let rt = Runtime::new();
    let root = rt.create_scope();
    let (shared, _) = root.signal(0i32);
    let baseline = rt.stats();
    for i in 0..500 {
        let child = root.child();
        let (local, set_local) = child.signal(i);
        let d = child.derive((shared.clone(), local.clone()), |(s, l)| s + l);
        let _e = child.subscribe(&d, |_| {});
        set_local.set(i + 1);
        assert_eq!(d.get(), i + 1);
        child.dispose();
    }
    let stats = rt.stats();
    assert_eq!(stats.sources, baseline.sources);
    assert_eq!(stats.derived, baseline.derived);
    assert_eq!(stats.effects, baseline.effects);
    assert_eq!(stats.edges, baseline.edges);
    assert_eq!(stats.live_scopes, baseline.live_scopes);
    assert_eq!(stats.queued_effects, 0);
}
