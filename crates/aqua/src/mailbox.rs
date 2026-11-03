use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

pub type Wake = Arc<dyn Fn() + Send + Sync>;

struct State<T> {
    queue: VecDeque<T>,
    capacity: usize,
    wake: Option<Wake>,
    closed: bool,
}

impl<T> State<T> {
    fn lock(state: &Mutex<Self>) -> MutexGuard<'_, Self> {
        state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

pub struct Mailbox<T> {
    state: Arc<Mutex<State<T>>>,
}

pub struct Sender<T> {
    state: Weak<Mutex<State<T>>>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendError<T> {
    Full(T),
    Closed(T),
}

impl<T> Mailbox<T> {
    pub fn new(capacity: std::num::NonZeroUsize) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                queue: VecDeque::new(),
                capacity: capacity.get(),
                wake: None,
                closed: false,
            })),
        }
    }

    pub fn sender(&self) -> Sender<T> {
        Sender {
            state: Arc::downgrade(&self.state),
        }
    }

    pub fn set_waker(&self, wake: Wake) {
        let (previous, pending) = {
            let mut state = State::lock(&self.state);
            if state.closed {
                (None, false)
            } else {
                let previous = state.wake.replace(wake.clone());
                (previous, !state.queue.is_empty())
            }
        };
        drop(previous);
        if pending {
            wake();
        }
    }

    pub fn drain(&mut self) -> Vec<T> {
        State::lock(&self.state).queue.drain(..).collect()
    }

    pub fn close(&mut self) {
        let released = {
            let mut state = State::lock(&self.state);
            state.closed = true;
            (std::mem::take(&mut state.queue), state.wake.take())
        };
        drop(released);
    }
}

impl<T> Drop for Mailbox<T> {
    fn drop(&mut self) {
        self.close();
    }
}

impl<T> Sender<T> {
    pub fn send(&self, message: T) -> Result<(), SendError<T>> {
        let Some(shared) = self.state.upgrade() else {
            return Err(SendError::Closed(message));
        };
        let wake = {
            let mut state = State::lock(&shared);
            if state.closed {
                return Err(SendError::Closed(message));
            }
            if state.queue.len() == state.capacity {
                return Err(SendError::Full(message));
            }
            let was_empty = state.queue.is_empty();
            state.queue.push_back(message);
            was_empty.then(|| state.wake.clone()).flatten()
        };
        if let Some(wake) = wake {
            wake();
        }
        Ok(())
    }
}
