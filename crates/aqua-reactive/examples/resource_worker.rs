//! End-to-end `Resource` + `Worker` wiring a host would use.
//!
//! Run: `cargo run --release --example resource_worker`
//!
//! A search box (signal) drives a resource; the host drains requests after
//! each flush, runs the blocking fetch on the worker thread, and commits
//! results back on the owner thread. A mid-flight query change demonstrates
//! stale rejection: only the latest generation commits.

use std::time::Duration;

use aqua_reactive::{worker::Worker, CompleteOutcome, ResourceState, Runtime};

fn main() {
    let rt = Runtime::new();
    let scope = rt.create_scope();
    let (query, set_query) = scope.signal("ab".to_string());
    let search: aqua_reactive::Resource<Vec<String>, String, String> =
        scope.resource((query.clone(),), |(q,)| q.clone());

    let worker = Worker::new(|| {});
    let paint = |label: &str| {
        println!("{label:<28} state={:?}", search.state().get());
    };

    // Host pump: flush, dispatch new requests, drain completions, commit.
    let pump = |work: fn(String) -> Vec<String>| {
        rt.flush();
        for req in search.take_requests() {
            worker.dispatch(req.ticket, req.params, work);
        }
        std::thread::sleep(Duration::from_millis(20));
        for done in worker.take_completed() {
            let out = *done.payload.downcast::<Vec<String>>().unwrap();
            let outcome = search.complete(&done.ticket, Ok(out));
            assert_eq!(outcome, CompleteOutcome::Committed);
        }
        rt.flush();
    };

    fn fake_search(q: String) -> Vec<String> {
        std::thread::sleep(Duration::from_millis(5));
        vec![format!("{q}-result-1"), format!("{q}-result-2")]
    }

    pump(fake_search);
    paint("after first load");

    // Slow fetch for "abc"; change the query again before it lands.
    set_query.set("abc".to_string());
    rt.flush();
    let slow = search.take_requests();
    assert_eq!(slow.len(), 1);
    set_query.set("abcd".to_string());
    rt.flush();
    let fast = search.take_requests();
    assert_eq!(fast.len(), 1);

    // Deliver out of order: stale first.
    let stale = &slow[0];
    let outcome = search.complete(&stale.ticket, Ok(vec!["stale".to_string()]));
    println!(
        "stale gen{} completion: {outcome:?}",
        stale.ticket.generation
    );
    assert!(matches!(outcome, CompleteOutcome::Stale(_)));

    worker.dispatch(fast[0].ticket, fast[0].params.clone(), fake_search);
    std::thread::sleep(Duration::from_millis(20));
    for done in worker.take_completed() {
        let out = *done.payload.downcast::<Vec<String>>().unwrap();
        search.complete(&done.ticket, Ok(out));
    }
    rt.flush();
    paint("after superseding load");
    assert!(matches!(
        search.state().get(),
        ResourceState::Ready { value, .. } if value == vec!["abcd-result-1".to_string(), "abcd-result-2".to_string()]
    ));
    println!(
        "stale completions rejected: {}",
        rt.stats().stale_completions
    );
    println!("OK: only the latest generation committed; no stale flash.");
}
