//! Batching, nesting, panic cleanup, and read-your-writes.
use std::cell::RefCell;
use std::rc::Rc;

use aqua_reactive::Runtime;

#[test]
fn batch_coalesces_effects_and_keeps_reads_fresh() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(0i32);
    let doubled = scope.derive1(&a, |v| v * 2);
    let runs = Rc::new(RefCell::new(Vec::new()));
    let runs_clone = runs.clone();
    let _effect = scope.effect((doubled.clone(),), move |(v,)| {
        runs_clone.borrow_mut().push(v);
    });
    assert_eq!(*runs.borrow(), vec![0]); // initial run

    scope.batch(|| {
        set_a.set(1);
        // Fresh read inside the batch: propagation is eager.
        assert_eq!(doubled.get(), 2);
        set_a.set(2);
        assert_eq!(doubled.get(), 4);
        set_a.set(3);
    });
    // One coalesced run with the latest value.
    assert_eq!(*runs.borrow(), vec![0, 6]);
}

#[test]
fn nested_batches_flush_once_at_outermost_exit() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(0i32);
    let count = Rc::new(RefCell::new(0usize));
    let count_clone = count.clone();
    let _effect = scope.effect1(&a, move |_| {
        *count_clone.borrow_mut() += 1;
    });
    assert_eq!(*count.borrow(), 1);
    scope.batch(|| {
        set_a.set(1);
        scope.batch(|| {
            set_a.set(2);
        });
        // Inner exit flushed nothing; nothing ran yet.
        assert_eq!(*count.borrow(), 1);
        set_a.set(3);
    });
    assert_eq!(*count.borrow(), 2);
}

#[test]
fn failed_inner_batch_preserves_outer_coherence() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(0i32);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_clone = seen.clone();
    let _effect = scope.effect1(&a, move |v| {
        seen_clone.borrow_mut().push(v);
    });
    assert_eq!(*seen.borrow(), vec![0]);

    let outer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        scope.batch(|| {
            set_a.set(1);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scope.batch(|| {
                    set_a.set(2);
                    panic!("inner batch failure");
                });
            }));
            // Writes committed by the failed batch persist (no rollback...
            // batches are coalescing, not transactions), and the outer batch
            // continues to work.
            assert_eq!(a.get(), 2);
            set_a.set(3);
        });
    }));
    assert!(outer.is_ok());
    // Effects queued by the failed inner batch were truncated; the outer
    // batch's final value flushed exactly once.
    assert_eq!(*seen.borrow(), vec![0, 3]);

    // Later updates outside any batch still work.
    set_a.set(4);
    assert_eq!(*seen.borrow(), vec![0, 3, 4]);
    let stats = rt.stats();
    assert_eq!(stats.sources, 1);
    assert_eq!(stats.effects, 1);
}

#[test]
fn failed_batch_releases_scheduling_links() {
    let rt = Runtime::new();
    rt.set_auto_flush(false);
    let scope = rt.create_scope();
    let (a, set_a) = scope.signal(0i32);
    let _effect = scope.effect1(&a, |_| {});
    rt.flush();
    assert_eq!(rt.pending(), 0);

    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rt.batch(|| {
            set_a.set(1);
            assert_eq!(rt.pending(), 1);
            panic!("boom");
        });
    }));
    // The failed batch truncated its queued work; depth restored.
    assert_eq!(rt.pending(), 0);
    set_a.set(2);
    assert_eq!(rt.pending(), 1);
    let report = rt.flush();
    assert_eq!((report.ran, report.truncated), (1, false));
}
