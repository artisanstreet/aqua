//! Async resource lifecycle: generations, stale rejection, reload, cancel,
//! disposal, input retriggering, and worker end-to-end delivery.
use std::cell::RefCell;
use std::rc::Rc;

use aqua_reactive::{worker::Worker, CompleteOutcome, ResourceState, Runtime, StaleReason};

#[test]
fn resource_loads_and_commits_current_generation() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (query, _) = scope.signal("q".to_string());
    let res: aqua_reactive::Resource<String, String, String> =
        scope.resource((query.clone(),), |(q,)| q.clone());

    // Initial fetch queued on first flush; state is Loading{1}. Auto-flush
    // runs the creation effect synchronously: exactly one request.
    let reqs = res.take_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].ticket.generation, 1);
    assert_eq!(reqs[0].params, "q".to_string());
    assert_eq!(res.state().get(), ResourceState::Loading { generation: 1 });

    let outcome = res.complete(&reqs[0].ticket, Ok("data".to_string()));
    assert_eq!(outcome, CompleteOutcome::Committed);
    assert_eq!(
        res.state().get(),
        ResourceState::Ready {
            generation: 1,
            value: "data".to_string()
        }
    );
    assert!(!res.is_in_flight());
}

#[test]
fn slow_first_completion_is_superseded_without_flash() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (query, set_query) = scope.signal("a".to_string());
    let res: aqua_reactive::Resource<String, String, String> =
        scope.resource((query.clone(),), |(q,)| q.clone());

    // Observe every committed state through a UI binding.
    let seen = Rc::new(RefCell::new(Vec::new()));
    let seen_clone = seen.clone();
    let _binding = scope.subscribe(&res.state(), move |s| {
        seen_clone.borrow_mut().push(s);
    });
    let gen1 = res.take_requests().pop().unwrap().ticket;
    assert_eq!(
        *seen.borrow(),
        vec![ResourceState::Loading { generation: 1 }]
    );

    // Input change before gen1 completes: gen2 requested, gen1 doomed.
    set_query.set("b".to_string());
    let gen2 = res.take_requests().pop().unwrap().ticket;
    assert_eq!(gen2.generation, 2);
    assert_eq!(
        *seen.borrow(),
        vec![
            ResourceState::Loading { generation: 1 },
            ResourceState::Loading { generation: 2 },
        ]
    );

    // Slow gen1 completes after gen2 was requested: rejected, never flashed.
    let outcome = res.complete(&gen1, Ok("stale-data".to_string()));
    assert_eq!(outcome, CompleteOutcome::Stale(StaleReason::Superseded));
    // Gen2 completes: commits.
    let outcome = res.complete(&gen2, Ok("fresh".to_string()));
    assert_eq!(outcome, CompleteOutcome::Committed);

    // The observer never saw gen1's value — no flash of stale success.
    assert_eq!(
        *seen.borrow(),
        vec![
            ResourceState::Loading { generation: 1 },
            ResourceState::Loading { generation: 2 },
            ResourceState::Ready {
                generation: 2,
                value: "fresh".to_string()
            },
        ]
    );
    assert_eq!(rt.stats().stale_completions, 1);
}

#[test]
fn reload_refetches_and_duplicate_delivery_is_rejected() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (query, _) = scope.signal("x".to_string());
    let res: aqua_reactive::Resource<String, String, String> =
        scope.resource((query.clone(),), |(q,)| q.clone());

    let gen1 = res.take_requests().pop().unwrap().ticket;
    // Reload before gen1 resolves: gen2 supersedes gen1.
    assert!(res.reload());
    let gen2 = res.take_requests().pop().unwrap().ticket;
    assert_eq!(gen2.generation, 2);
    assert_eq!(
        res.complete(&gen1, Ok("old".to_string())),
        CompleteOutcome::Stale(StaleReason::Superseded)
    );
    assert_eq!(
        res.complete(&gen2, Ok("new".to_string())),
        CompleteOutcome::Committed
    );
    // Host retried delivery of gen2: duplicate, state untouched.
    assert_eq!(
        res.complete(&gen2, Ok("new-again".to_string())),
        CompleteOutcome::Stale(StaleReason::Duplicate)
    );
    assert_eq!(
        res.state().get(),
        ResourceState::Ready {
            generation: 2,
            value: "new".to_string()
        }
    );
    assert_eq!(rt.stats().stale_completions, 2);
}

#[test]
fn cancel_resets_to_idle_and_rejects_late_success() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (query, _) = scope.signal("x".to_string());
    let res: aqua_reactive::Resource<String, String, String> =
        scope.resource((query.clone(),), |(q,)| q.clone());

    let gen1 = res.take_requests().pop().unwrap().ticket;
    assert!(res.is_in_flight());
    assert!(res.cancel());
    assert!(!res.is_in_flight());
    assert!(!res.cancel()); // nothing in flight: no-op
    assert_eq!(res.state().get(), ResourceState::Idle);
    // Late success for the cancelled generation: rejected, no flash.
    assert_eq!(
        res.complete(&gen1, Ok("too-late".to_string())),
        CompleteOutcome::Stale(StaleReason::Cancelled)
    );
    assert_eq!(res.state().get(), ResourceState::Idle);
    assert_eq!(rt.stats().stale_completions, 1);
}

#[test]
fn scope_disposal_rejects_late_completions() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (query, _) = scope.signal("x".to_string());
    let res: aqua_reactive::Resource<String, String, String> =
        scope.resource((query.clone(),), |(q,)| q.clone());
    let ticket = res.take_requests().pop().unwrap().ticket;

    scope.dispose();
    assert_eq!(
        res.complete(&ticket, Ok("orphan".to_string())),
        CompleteOutcome::Stale(StaleReason::Disposed)
    );
    assert!(res.take_requests().is_empty());
    assert_eq!(rt.stats().stale_completions, 1);
}

#[test]
fn input_changes_retrigger_with_latest_params_and_coalesce() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (query, set_query) = scope.signal(0i32);
    let res: aqua_reactive::Resource<String, String, i32> =
        scope.resource((query.clone(),), |(q,)| q);
    // Creation effect already queued gen1.
    let _ = res.take_requests();

    scope.batch(|| {
        set_query.set(1);
        set_query.set(2);
        set_query.set(3);
    });
    // Coalesced: exactly one request with the latest params.
    let reqs = res.take_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].params, 3);
    assert_eq!(
        res.state().get(),
        ResourceState::Loading {
            generation: reqs[0].ticket.generation
        }
    );
}

#[test]
fn failed_fetch_is_observable_and_retryable() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (query, _) = scope.signal("x".to_string());
    let res: aqua_reactive::Resource<String, String, String> =
        scope.resource((query.clone(),), |(q,)| q.clone());
    let t1 = res.take_requests().pop().unwrap().ticket;
    assert_eq!(
        res.complete(&t1, Err("boom".to_string())),
        CompleteOutcome::Committed
    );
    assert_eq!(
        res.state().get(),
        ResourceState::Failed {
            generation: 1,
            error: "boom".to_string()
        }
    );
    // Retry via reload recovers.
    assert!(res.reload());
    let t2 = res.take_requests().pop().unwrap().ticket;
    assert_eq!(
        res.complete(&t2, Ok("recovered".to_string())),
        CompleteOutcome::Committed
    );
    assert_eq!(
        res.state().get(),
        ResourceState::Ready {
            generation: 2,
            value: "recovered".to_string()
        }
    );
}

#[test]
fn worker_dispatch_delivers_completion_to_owner_thread() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (query, _) = scope.signal(7i32);
    let res: aqua_reactive::Resource<i32, String, i32> = scope.resource((query.clone(),), |(q,)| q);

    let wakes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let wakes_clone = wakes.clone();
    let worker = Worker::new(move || {
        wakes_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });

    // Host wiring: drain outbox after flush, run on worker thread.
    for req in res.take_requests() {
        worker.dispatch(req.ticket, req.params, |p: i32| p * 2);
    }
    // Wait for the background result (bounded; failure to arrive is a bug).
    let mut spins = 0;
    while worker.pending() == 0 && spins < 10_000 {
        std::thread::sleep(std::time::Duration::from_millis(1));
        spins += 1;
    }
    assert!(worker.pending() > 0, "worker never delivered");
    assert!(
        wakes.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "wake hook never fired"
    );

    // Owner-thread commit: downcast and complete.
    for done in worker.take_completed() {
        let value = *done.payload.downcast::<i32>().expect("output type");
        let outcome = res.complete(&done.ticket, Ok(value));
        assert_eq!(outcome, CompleteOutcome::Committed);
    }
    assert_eq!(
        res.state().get(),
        ResourceState::Ready {
            generation: 1,
            value: 14
        }
    );
}

#[test]
fn notify_hook_fires_without_polling() {
    let rt = Runtime::new();
    rt.set_auto_flush(false);
    let scope = rt.create_scope();
    let (query, set_query) = scope.signal(0i32);
    let res: aqua_reactive::Resource<String, String, i32> =
        scope.resource((query.clone(),), |(q,)| q);
    let fires = Rc::new(RefCell::new(0usize));
    let fires_clone = fires.clone();
    res.set_notify(move || {
        *fires_clone.borrow_mut() += 1;
    });
    rt.flush(); // initial fetch
    assert_eq!(*fires.borrow(), 1);
    assert_eq!(res.take_requests().len(), 1);
    set_query.set(1);
    rt.flush();
    assert_eq!(*fires.borrow(), 2);
}
