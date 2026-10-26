//! Host-side background worker for [`Resource`](crate::Resource) fetches.
//!
//! The worker owns a single background thread that runs blocking fetch
//! closures and reports type-erased completions. The owner thread drains
//! them with [`take_completed`](Worker::take_completed) — typically right
//! after each flush — and commits them via `Resource::complete`. A
//! [`wake`](Worker::new) hook fires on every arrival so the host event loop
//! wakes without polling an idle queue.

use std::any::Any;
use std::collections::VecDeque;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use crate::resource::FetchTicket;

/// One finished fetch, awaiting owner-thread commitment. Downcast `payload`
/// to the resource's output type (`R` in the matching
/// [`dispatch`](Worker::dispatch) call) and pass it to `Resource::complete`.
pub struct Completed {
    pub ticket: FetchTicket,
    pub payload: Box<dyn Any + Send>,
}

type Job = Box<dyn FnOnce() -> Completed + Send>;

/// Single-threaded FIFO background executor for blocking fetch work.
///
/// Created with a `Send + Sync` wake hook (called on arrival, from the worker
/// thread — keep it cheap: set a flag, wake the event loop). Drop shuts the
/// thread down cleanly after in-flight work.
pub struct Worker {
    tx: Option<mpsc::Sender<Job>>,
    done: Arc<Mutex<VecDeque<Completed>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Worker {
    /// Spawn the worker thread. `wake` runs on the worker thread after every
    /// finished job.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let done = Arc::new(Mutex::new(VecDeque::new()));
        let done_clone = done.clone();
        let wake = Arc::new(wake);
        let thread = thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                let completed = job();
                done_clone
                    .lock()
                    .expect("aqua-reactive: worker result queue poisoned")
                    .push_back(completed);
                wake();
            }
        });
        Self {
            tx: Some(tx),
            done,
            thread: Some(thread),
        }
    }

    /// Run `work(params)` on the worker thread, then queue a [`Completed`]
    /// carrying `ticket` and the boxed output. The ticket travels with the
    /// work, so completion routing needs no shared mutable host state.
    pub fn dispatch<P, R>(
        &self,
        ticket: FetchTicket,
        params: P,
        work: impl FnOnce(P) -> R + Send + 'static,
    ) where
        P: Send + 'static,
        R: Send + 'static,
    {
        let job: Job = Box::new(move || Completed {
            ticket,
            payload: Box::new(work(params)),
        });
        self.tx
            .as_ref()
            .expect("aqua-reactive: dispatch on a shut-down Worker")
            .send(job)
            .expect("aqua-reactive: worker thread gone");
    }

    /// Drain finished completions (call on the owner thread).
    pub fn take_completed(&self) -> Vec<Completed> {
        self.done
            .lock()
            .expect("aqua-reactive: worker result queue poisoned")
            .drain(..)
            .collect()
    }

    /// Finished-but-undrained completions.
    pub fn pending(&self) -> usize {
        self.done
            .lock()
            .expect("aqua-reactive: worker result queue poisoned")
            .len()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Dropping the sender lets the thread's `recv` fail once its queue
        // drains; then join it. In-flight work runs to completion first.
        self.tx.take();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}
