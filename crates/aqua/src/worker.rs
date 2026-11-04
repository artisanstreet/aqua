use std::any::Any;
use std::collections::VecDeque;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use aqua_reactive::FetchTicket;
pub struct Completed {
    pub ticket: FetchTicket,
    pub payload: Box<dyn Any + Send>,
}

type Job = Box<dyn FnOnce() -> Completed + Send>;
pub struct Worker {
    tx: Option<mpsc::Sender<Job>>,
    done: Arc<Mutex<VecDeque<Completed>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Worker {
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
    pub fn take_completed(&self) -> Vec<Completed> {
        self.done
            .lock()
            .expect("aqua-reactive: worker result queue poisoned")
            .drain(..)
            .collect()
    }
    pub fn pending(&self) -> usize {
        self.done
            .lock()
            .expect("aqua-reactive: worker result queue poisoned")
            .len()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}
