use std::sync::{Arc, Mutex};

use aqua_platform::AquaWindowId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AquaUserEvent {
    ReactiveWake(AquaWindowId),
    Messages(AquaWindowId),
    WindowRequests,
    A11yActions(AquaWindowId),
    RenderEvents(AquaWindowId),
    Tsf(AquaWindowId),
}

pub type Waker = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Shared {
    events: Mutex<Vec<AquaUserEvent>>,
    waker: Mutex<Option<Waker>>,
}

#[derive(Clone, Default)]
pub struct Inbox {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Inbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inbox").finish_non_exhaustive()
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Inbox {
    pub fn post(&self, event: AquaUserEvent) {
        {
            let mut events = locked(&self.shared.events);
            if !events.contains(&event) {
                events.push(event);
            }
        }
        let waker = locked(&self.shared.waker).clone();
        if let Some(waker) = waker {
            waker();
        }
    }

    pub fn set_waker(&self, waker: Option<Waker>) {
        *locked(&self.shared.waker) = waker;
    }

    pub fn drain(&self) -> Vec<AquaUserEvent> {
        std::mem::take(&mut *locked(&self.shared.events))
    }

    pub fn is_empty(&self) -> bool {
        locked(&self.shared.events).is_empty()
    }
}
