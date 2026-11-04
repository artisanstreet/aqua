use aqua::worker::Worker;
use aqua_reactive::{CompleteOutcome, ResourceState, Runtime};

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
    for req in res.take_requests() {
        worker.dispatch(req.ticket, req.params, |p: i32| p * 2);
    }
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
