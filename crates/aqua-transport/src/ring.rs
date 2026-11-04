use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, PartialEq, Eq)]
pub enum SendError<T> {
    Full(T),
    Disconnected(T),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecvError {
    Empty,
    Disconnected,
    Timeout,
}

struct Shared<T> {
    slots: Box<[Mutex<Option<T>>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    sender_alive: AtomicBool,
    receiver_alive: AtomicBool,
    receiver_thread: Mutex<Option<Thread>>,
    sender_thread: Mutex<Option<Thread>>,
    notify: Option<Notify>,
}

impl<T> Shared<T> {
    fn capacity(&self) -> usize {
        self.slots.len()
    }

    fn len(&self) -> usize {
        self.tail
            .load(Ordering::Acquire)
            .wrapping_sub(self.head.load(Ordering::Acquire))
    }
}

fn wake(slot: &Mutex<Option<Thread>>) {
    if let Some(thread) = slot.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        thread.unpark();
    }
}

fn register(slot: &Mutex<Option<Thread>>) {
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(thread::current());
}

pub struct Sender<T> {
    shared: Arc<Shared<T>>,
}

pub struct Receiver<T> {
    shared: Arc<Shared<T>>,
}

#[must_use]
pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    build(capacity, None)
}

#[must_use]
pub fn channel_with_notify<T>(capacity: usize, notify: Notify) -> (Sender<T>, Receiver<T>) {
    build(capacity, Some(notify))
}

fn build<T>(capacity: usize, notify: Option<Notify>) -> (Sender<T>, Receiver<T>) {
    let capacity = capacity.max(1);
    let shared = Arc::new(Shared {
        slots: (0..capacity).map(|_| Mutex::new(None)).collect(),
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        sender_alive: AtomicBool::new(true),
        receiver_alive: AtomicBool::new(true),
        receiver_thread: Mutex::new(None),
        sender_thread: Mutex::new(None),
        notify,
    });
    (
        Sender {
            shared: Arc::clone(&shared),
        },
        Receiver { shared },
    )
}

impl<T> Sender<T> {
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shared.capacity()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn try_send(&mut self, value: T) -> Result<(), SendError<T>> {
        let shared = &self.shared;
        if !shared.receiver_alive.load(Ordering::Acquire) {
            return Err(SendError::Disconnected(value));
        }
        let tail = shared.tail.load(Ordering::Relaxed);
        let head = shared.head.load(Ordering::Acquire);
        if tail.wrapping_sub(head) >= shared.capacity() {
            return Err(SendError::Full(value));
        }
        *shared.slots[tail % shared.capacity()]
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(value);
        shared.tail.store(tail.wrapping_add(1), Ordering::Release);
        wake(&shared.receiver_thread);
        if let Some(notify) = &shared.notify {
            notify();
        }
        Ok(())
    }

    pub fn send(&mut self, mut value: T) -> Result<(), SendError<T>> {
        loop {
            match self.try_send(value) {
                Err(SendError::Full(back)) => {
                    value = back;
                    register(&self.shared.sender_thread);
                    if self.shared.len() >= self.shared.capacity() {
                        thread::park_timeout(Duration::from_millis(1));
                    }
                }
                other => return other,
            }
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.shared.sender_alive.store(false, Ordering::Release);
        wake(&self.shared.receiver_thread);
        if let Some(notify) = &self.shared.notify {
            notify();
        }
    }
}

impl<T> Receiver<T> {
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn try_recv(&mut self) -> Result<T, RecvError> {
        let shared = &self.shared;
        let head = shared.head.load(Ordering::Relaxed);
        let tail = shared.tail.load(Ordering::Acquire);
        if head == tail {
            return match shared.sender_alive.load(Ordering::Acquire) {
                true => Err(RecvError::Empty),
                false => match shared.tail.load(Ordering::Acquire) == head {
                    true => Err(RecvError::Disconnected),
                    false => Err(RecvError::Empty),
                },
            };
        }
        let value = shared.slots[head % shared.capacity()]
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        shared.head.store(head.wrapping_add(1), Ordering::Release);
        wake(&shared.sender_thread);
        value.ok_or(RecvError::Empty)
    }

    pub fn drain(&mut self, limit: usize) -> Vec<T> {
        let mut out = Vec::new();
        while out.len() < limit {
            match self.try_recv() {
                Ok(value) => out.push(value),
                Err(_) => break,
            }
        }
        out
    }

    pub fn recv_timeout(&mut self, timeout: Option<Duration>) -> Result<T, RecvError> {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            match self.try_recv() {
                Err(RecvError::Empty) => {}
                other => return other,
            }
            register(&self.shared.receiver_thread);
            match self.try_recv() {
                Err(RecvError::Empty) => {}
                other => return other,
            }
            match deadline {
                None => thread::park(),
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(RecvError::Timeout);
                    }
                    thread::park_timeout(deadline - now);
                }
            }
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.shared.receiver_alive.store(false, Ordering::Release);
        wake(&self.shared.sender_thread);
    }
}
